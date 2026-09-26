//! IR tests over every method of every fixture dex (D8 and R8 builds).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use eightr_dex::class::access;
use eightr_dex::Dex;
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::types::{MethodSig, RefType, RegType, TypeInference};
use eightr_ir::{lift, print, Body, Cfg, Interner};

fn fixture_dexes() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut v: Vec<PathBuf> = fs::read_dir(root)
        .unwrap()
        .flat_map(|f| ["d8", "r8"].map(|x| f.as_ref().unwrap().path().join(x).join("classes.dex")))
        .filter(|p| p.exists())
        .collect();
    v.sort();
    v
}

struct Method {
    ctx: String,
    body: Body,
    sig: MethodSig,
}

/// Lifts every method with code in `path`.
fn methods(path: &Path, syms: &mut Interner) -> Vec<Method> {
    let bytes = fs::read(path).unwrap();
    let dex = Dex::parse(&bytes).unwrap();
    let mut out = Vec::new();
    for def in dex.class_defs() {
        let def = def.unwrap();
        let class = dex.type_descriptor(def.class_idx).unwrap().into_owned();
        for m in dex.class_data(&def).unwrap().methods() {
            let Some(code) = dex.code_item(m.code_off).unwrap() else { continue };
            let id = dex.method_id(m.method_idx).unwrap();
            let name = dex.string(id.name_idx).unwrap().into_owned();
            let proto = dex.proto_descriptor(id.proto_idx.into()).unwrap();
            let ctx = format!("{}: {class}->{name}{proto}", path.display());
            let body = lift(&dex, &code, syms).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let sig = MethodSig { class: syms.intern(&class), name, proto, is_static: m.access_flags & access::STATIC != 0 };
            out.push(Method { ctx, body, sig });
        }
    }
    out
}

fn all_methods(syms: &mut Interner) -> Vec<Method> {
    let v: Vec<Method> = fixture_dexes().iter().flat_map(|p| methods(p, syms)).collect();
    assert!(v.len() > 20, "expected many methods, got {}", v.len());
    v
}

#[test]
fn cfg_invariants() {
    let mut syms = Interner::default();
    for m in all_methods(&mut syms) {
        let cfg = Cfg::build(&m.body).unwrap_or_else(|e| panic!("{}: {e}", m.ctx));
        let n = m.body.insns.len() as u32;
        // Blocks partition the instructions in order.
        let mut next = 0;
        for (b, blk) in cfg.blocks.iter().enumerate() {
            assert_eq!(blk.start, next, "{}", m.ctx);
            assert!(blk.end > blk.start, "{}", m.ctx);
            for i in blk.start..blk.end {
                assert_eq!(cfg.block_of[i as usize], b as u32, "{}", m.ctx);
            }
            next = blk.end;
        }
        assert_eq!(next, n, "{}", m.ctx);
        for (b, blk) in cfg.blocks.iter().enumerate() {
            let last = &m.body.insns[blk.last() as usize].op;
            for e in &blk.succs {
                assert!(cfg.blocks[e.to as usize].preds.contains(&(b as u32)), "{}: pred missing", m.ctx);
                if e.is_exceptional() {
                    assert!(last.can_throw(), "{}: exceptional edge from non-throwing op", m.ctx);
                }
            }
            for &p in &blk.preds {
                assert!(cfg.blocks[p as usize].succs.iter().any(|e| e.to == b as u32), "{}: succ missing", m.ctx);
            }
            // Branch targets start blocks.
            for i in blk.start..blk.end {
                for t in m.body.insns[i as usize].op.targets() {
                    assert_eq!(cfg.blocks[cfg.block_of[t as usize] as usize].start, t, "{}", m.ctx);
                }
            }
            // A throwing instruction inside a try is always last in its block.
            for i in blk.start..blk.last() {
                let op = &m.body.insns[i as usize].op;
                assert!(!(op.can_throw() && m.body.try_covering(i).is_some()), "{}: throwing insn mid-block", m.ctx);
            }
        }
        assert_eq!(cfg.rpo[0], 0, "{}", m.ctx);
    }
}

/// Naive dominator sets: Dom(entry) = {entry}; Dom(n) = {n} ∪ ⋂ Dom(p) over reachable preds.
fn naive_dominators(cfg: &Cfg) -> Vec<Option<BTreeSet<u32>>> {
    let n = cfg.blocks.len();
    let all: BTreeSet<u32> = cfg.rpo.iter().copied().collect();
    let mut dom: Vec<Option<BTreeSet<u32>>> =
        (0..n).map(|b| cfg.is_reachable(b as u32).then(|| all.clone())).collect();
    dom[0] = Some(BTreeSet::from([0]));
    let mut changed = true;
    while changed {
        changed = false;
        for &b in cfg.rpo.iter().skip(1) {
            let mut new: Option<BTreeSet<u32>> = None;
            for &p in &cfg.blocks[b as usize].preds {
                if let Some(d) = &dom[p as usize] {
                    new = Some(match new {
                        None => d.clone(),
                        Some(acc) => acc.intersection(d).copied().collect(),
                    });
                }
            }
            let mut new = new.unwrap_or_default();
            new.insert(b);
            if dom[b as usize].as_ref() != Some(&new) {
                dom[b as usize] = Some(new);
                changed = true;
            }
        }
    }
    dom
}

#[test]
fn dominators_match_naive_algorithm() {
    let mut syms = Interner::default();
    let mut nontrivial = 0;
    for m in all_methods(&mut syms) {
        let cfg = Cfg::build(&m.body).unwrap();
        let idom = cfg.dominators();
        let sets = naive_dominators(&cfg);
        for b in 0..cfg.blocks.len() {
            match (&sets[b], idom[b]) {
                (None, None) => {}
                (Some(set), Some(d)) => {
                    if b == 0 {
                        assert_eq!(d, 0);
                        continue;
                    }
                    // The immediate dominator is the strict dominator with the largest
                    // dominator set (it is dominated by all the others).
                    let expected = set
                        .iter()
                        .copied()
                        .filter(|&x| x != b as u32)
                        .max_by_key(|&x| sets[x as usize].as_ref().unwrap().len())
                        .unwrap();
                    assert_eq!(d, expected, "{}: idom(B{b})", m.ctx);
                    if d != b as u32 - 1 {
                        nontrivial += 1;
                    }
                }
                other => panic!("{}: B{b} reachability mismatch {other:?}", m.ctx),
            }
        }
    }
    assert!(nontrivial > 10, "fixtures should exercise non-trivial dominance");
}

/// Is inferred type `t` consistent with a local declared as descriptor `d` (low half for
/// wide types)?
fn compatible(t: RegType, d: &str) -> bool {
    use RegType::*;
    match d.as_bytes()[0] {
        b'Z' | b'B' | b'S' | b'C' | b'I' => matches!(t, Int | Zero | Narrow),
        b'F' => matches!(t, Float | Zero | Narrow),
        b'J' => matches!(t, LongLo | WideLo),
        b'D' => matches!(t, DoubleLo | WideLo),
        _ => matches!(t, Ref(_) | Zero | Uninit { .. } | UninitThis(_)),
    }
}

/// Differential test against the compiler's own knowledge: D8 --debug builds carry every
/// local variable's declared type and live range. The inferred register type must agree with
/// the declaration wherever the value is observed: at the start of the range, and at every
/// instruction in the range that reads the register. (Ranges can be imprecise at register
/// shuffles, e.g. kotlinc's Compose output ends a range one instruction after the register
/// was overwritten, so unread positions aren't compared.)
#[test]
fn inferred_types_agree_with_debug_locals() {
    let mut syms = Interner::default();
    let mut checked = 0;
    let mut exact_refs = 0;
    let paths: Vec<PathBuf> = fixture_dexes().into_iter().filter(|p| p.to_string_lossy().contains("/d8/")).collect();
    let mut methods_all = Vec::new();
    for p in &paths {
        methods_all.extend(methods(p, &mut syms));
    }
    for m in &methods_all {
        TypeInference::prepare(&m.body, &m.sig, &mut syms);
    }
    for m in &methods_all {
        let cfg = Cfg::build(&m.body).unwrap();
        let states = TypeInference::new(&m.body, &cfg, &m.sig, &syms).run();
        for local in &m.body.locals {
            let Some(ty) = local.ty else { continue };
            let desc = syms.get(ty);
            for (i, insn) in m.body.insns.iter().enumerate() {
                if (i as u32) < local.start || (i as u32) >= local.end {
                    continue;
                }
                if i as u32 != local.start && !insn.op.uses().contains(&local.reg) {
                    continue;
                }
                let Some(state) = &states[i] else { continue };
                let t = state.regs[local.reg as usize];
                let name = local.name.map(|n| syms.get(n)).unwrap_or("?");
                assert!(
                    compatible(t, desc),
                    "{} @{i} pc {:04x}: local {name} v{} declared {desc} but inferred {}",
                    m.ctx,
                    insn.pc,
                    local.reg,
                    t.display(&syms)
                );
                if let RegType::Ref(RefType::Exact(s)) = t {
                    if syms.get(s) == desc {
                        exact_refs += 1;
                    }
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 300, "only {checked} local observations checked");
    assert!(exact_refs > 50, "reference inference is too imprecise: {exact_refs}");
}

#[test]
fn every_use_has_a_reaching_def() {
    let mut syms = Interner::default();
    for m in all_methods(&mut syms) {
        let cfg = Cfg::build(&m.body).unwrap();
        let rd = ReachingDefs::compute(&m.body, &cfg);
        for (i, uses) in rd.uses.iter().enumerate() {
            let Some(uses) = uses else { continue };
            for (reg, defs) in uses {
                assert!(!defs.is_empty(), "{} @{i}: v{reg} read with no reaching def", m.ctx);
                for &d in defs {
                    assert_eq!(rd.defs[d].reg, *reg);
                    if let DefSite::Insn(site) = rd.defs[d].site {
                        assert!(site as usize != i || m.body.insns[i].op.uses().contains(reg));
                    }
                }
            }
        }
    }
}

/// Sym ids depend on interning order; nothing observable may. Lift everything with a fresh
/// interner and with one pre-seeded (in reverse order) with every string, and compare output.
#[test]
fn output_independent_of_interner_order() {
    let render = |syms: &mut Interner| -> String {
        let ms = all_methods(syms);
        ms.iter()
            .map(|m| {
                let cfg = Cfg::build(&m.body).unwrap();
                format!("{}\n{}", m.ctx, print::body(&m.body, &cfg, syms))
            })
            .collect()
    };
    let mut fresh = Interner::default();
    let a = render(&mut fresh);
    let mut seeded = Interner::default();
    let mut words: Vec<String> = Vec::new();
    for p in fixture_dexes() {
        let bytes = fs::read(&p).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        words.extend(dex.strings().map(|s| s.unwrap().into_owned()));
    }
    words.sort();
    for w in words.iter().rev() {
        seeded.intern(w);
    }
    let b = render(&mut seeded);
    assert_eq!(a, b);
}

/// Golden snapshots of a few methods. Regenerate with EIGHTR_UPDATE_SNAPSHOTS=1.
#[test]
fn snapshots() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let wanted = [
        ("hello/d8", "Lcom/example/Hello;->main([Ljava/lang/String;)V"),
        ("opcodes/d8", "Lcom/example/Opcodes;->packedSwitch(I)I"),
        ("opcodes/d8", "Lcom/example/Opcodes;->exceptions(Ljava/lang/Object;)Ljava/lang/String;"),
        ("shapes/r8", "Lcom/example/Main;->main([Ljava/lang/String;)V"),
    ];
    let update = std::env::var_os("EIGHTR_UPDATE_SNAPSHOTS").is_some();
    for (fixture, method) in wanted {
        let mut syms = Interner::default();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out").join(fixture).join("classes.dex");
        let m = methods(&path, &mut syms).into_iter().find(|m| m.ctx.ends_with(method)).expect(method);
        let cfg = Cfg::build(&m.body).unwrap();
        let text = format!("# {fixture} {method}\n{}", print::body(&m.body, &cfg, &syms));
        let file = dir.join(format!("{}__{}.txt", fixture.replace('/', "_"), method.split("->").nth(1).unwrap().split('(').next().unwrap()));
        if update {
            fs::create_dir_all(&dir).unwrap();
            fs::write(&file, &text).unwrap();
        } else {
            let expected = fs::read_to_string(&file).unwrap_or_else(|_| panic!("missing {}; run with EIGHTR_UPDATE_SNAPSHOTS=1", file.display()));
            assert_eq!(text, expected, "snapshot {}", file.display());
        }
    }
}
