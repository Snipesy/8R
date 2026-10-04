//! Class and field names from LibDB packs (docs/research/libdb.md §11), after the signature
//! pass named methods (`r8/libdb-method-name` S, `r8/sigdb-method-name` D):
//!
//! * `r8/libdb-class-name` (S): an app class with at least two S-named methods matching one pack
//!   class, every exact match of the class agreeing, and the class's shape among the pack's shapes
//!   of that class: the original simple name and package. The `Package` label states the original
//!   package (a fact, S) whether or not the class could go back there: the pipeline moves it only
//!   where access allows and the name collides with nothing (`crate::repackage`), and records
//!   refusals as a finding.
//! * `r8/libdb-field-name` (S) / `r8/libdb-field-hint` (D): matched bodies have the same token
//!   sequence, so the n-th program field access of the app method is the n-th of its pack twin.
//!   Every aligned access of a field must name the same pack field, of a compatible type.
//!
//! Returns the items named, with their library (`group:artifact:version`) and the pack's app id,
//! for the `@eightr.Library` tag.

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::model::Program as Model;
use eightr_rules::{Attribute, LIBDB_CLASS_NAME, LIBDB_FIELD_HINT, LIBDB_FIELD_NAME};

use crate::error::Result;
use crate::labels::Labels;
use crate::pipeline::Evidence;
use crate::program::{ClassId, ItemId};
use crate::sigdb::db::SigDb;
use crate::sigdb::matcher::Matches;
use crate::sigdb::print::{class_print, erase_type, platform_stable, MethodPrint};

/// `@eightr.Library(value = "group:artifact:version", app = "<pack app id>")`.
pub const LIBRARY: &str = "Leightr/Library;";

fn simple(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map_or(inner, |x| x.1)
}

fn package(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map_or("", |x| x.0)
}

/// The program field a reference resolves to (the class, then its superclasses).
fn resolve_field(p: &Model, class: &str, name: &str, ty: &str) -> Option<(usize, usize)> {
    let mut cur = p.find(class);
    let mut guard = 0;
    while let Some(ci) = cur {
        let c = &p.classes[ci];
        if let Some(fi) = c.fields.iter().position(|f| p.syms.get(f.name) == name && p.syms.get(f.ty) == ty) {
            return Some((ci, fi));
        }
        guard += 1;
        if guard > 64 {
            return None;
        }
        cur = c.superclass.and_then(|t| p.find(p.syms.get(t)));
    }
    None
}

pub fn name_classes_and_fields(
    p: &Model,
    evidence: &Evidence,
    labels: &mut Labels,
    dbs: &[SigDb],
    matches: &Matches,
    prints: &BTreeMap<usize, MethodPrint>,
    named: &BTreeMap<usize, bool>,
) -> Result<Vec<(ItemId, String, String)>> {
    let facts = &evidence.libdb.packs;
    let s = &p.syms;
    let pins = eightr_ir::reflect::pins(p);
    let stable = |d: &str| platform_stable(d);
    let pack_class = |k: usize| {
        let m = &matches.methods[k];
        (m.key.0, dbs[m.key.0 as usize].methods[m.key.1 as usize].0)
    };
    let from_pack = |k: usize| (matches.methods[k].key.0 as usize) < facts.len();
    let mut tags: Vec<(ItemId, String, String)> = Vec::new();
    let tag_of = |db: u16, pc: u32| -> Option<(String, String)> {
        let f = &facts[db as usize];
        Some((f.coord.get(pc as usize).cloned().flatten()?, f.app.clone()))
    };

    // ---- classes ----
    // App class → pack classes of its S-named methods (counts), and of all its exact matches.
    let mut s_votes: BTreeMap<usize, BTreeMap<(u16, u32), usize>> = BTreeMap::new();
    // Every pack class any method of the class matched, by any stage.
    let mut exact: BTreeMap<usize, BTreeSet<(u16, u32)>> = BTreeMap::new();
    let mut any_count: BTreeMap<(usize, (u16, u32)), usize> = BTreeMap::new();
    for (k, m) in matches.methods.iter().enumerate() {
        if !from_pack(k) {
            continue;
        }
        exact.entry(m.class).or_default().insert(pack_class(k));
        *any_count.entry((m.class, pack_class(k))).or_default() += 1;
        if named.get(&k) == Some(&true) {
            *s_votes.entry(m.class).or_default().entry(pack_class(k)).or_default() += 1;
        }
    }
    // Whether app class `ci` can be pack class `pc` as a whole: its full shape (supertypes,
    // interfaces, every field type, erased) is one the pack saw for that class and for no class of
    // its hierarchy, and every method could be one of that class's (by erased proto). A class R8
    // merged others into (horizontal, static or vertical merging) has extra fields, supertypes or
    // methods, or the shape of the merged class.
    let class_fits = |ci: usize, pc: (u16, u32)| -> bool {
        let shape = class_print(p, ci, &stable);
        let f = &facts[pc.0 as usize];
        let owners = f.shape_classes.get(&shape.c2);
        if !owners.is_some_and(|o| o.contains(&pc.1)) {
            return false;
        }
        // The shape was also seen for a class of the same hierarchy: R8 merges a class into its
        // only subclass (`FragmentFactory` into `FragmentManager$3`), which then has its shape.
        if owners.into_iter().flatten().any(|&k| k != pc.1 && (f.is_ancestor(k, pc.1) || f.is_ancestor(pc.1, k))) {
            return false;
        }
        let db = &dbs[pc.0 as usize];
        let theirs: BTreeSet<String> = db.methods.iter().filter(|x| x.0 == pc.1).map(|x| crate::sigdb::print::erase_proto(&x.2, &stable)).collect();
        p.classes[ci].methods.iter().filter(|m| m.code.is_some() && s.get(m.name) != "<clinit>").all(|m| theirs.contains(&crate::sigdb::print::erase_proto(s.get(m.proto), &stable)))
    };
    // (app class, original descriptor, pack class)
    let mut claims: Vec<(usize, String, (u16, u32))> = Vec::new();
    for (&ci, votes) in &s_votes {
        let [(&pc, &n)] = votes.iter().collect::<Vec<_>>()[..] else { continue };
        if n < 2 || exact.get(&ci).is_some_and(|e| e.len() != 1) {
            continue;
        }
        let d = s.get(p.classes[ci].ty);
        let item = ItemId::Class { class: ClassId(ci as u32) };
        if labels.get(item, Attribute::ClassName).is_some() || pins.class(d) || crate::naming::is_platform_class(d) {
            continue;
        }
        if !class_fits(ci, pc) {
            continue;
        }
        claims.push((ci, dbs[pc.0 as usize].classes[pc.1 as usize].clone(), pc));
    }
    // An original class claimed by two app classes names neither.
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &claims {
        *count.entry(c.1.as_str()).or_default() += 1;
    }
    // The package goes back only where the full original name is safe to take: not in a package of
    // the boot class path (boot classes load first; ART refuses to define `java/` classes), and
    // not named by a string of the program or its resources, in any form (a `Class.forName` probe,
    // `getName()` comparison, JNI `FindClass` or manifest entry that fails today would start to
    // match).
    let mut strings: BTreeSet<&str> = BTreeSet::new();
    for c in &p.classes {
        for insn in c.methods.iter().flat_map(|m| m.code.iter().flat_map(|b| &b.insns)) {
            if let eightr_ir::op::Op::ConstString { value, .. } = &insn.op {
                strings.insert(s.get(*value));
            }
        }
        for f in &c.fields {
            if let Some(eightr_ir::value::Value::String(v)) = &f.static_value {
                strings.insert(s.get(*v));
            }
        }
    }
    let restorable = |original: &str| {
        let slashed = original.trim_start_matches('L').trim_end_matches(';');
        let dotted = slashed.replace('/', ".");
        let named = |n: &str| strings.contains(n) || evidence.resources.iter().any(|(_, text)| text.contains(n));
        !crate::naming::is_platform_class(original)
            && !crate::naming::is_platform_package(package(original))
            && !named(&dotted)
            && !named(slashed)
            && !named(original)
    };
    for (ci, original, pc) in &claims {
        if count[original.as_str()] != 1 {
            continue;
        }
        let item = ItemId::Class { class: ClassId(*ci as u32) };
        labels.record_value(item, Attribute::ClassName, LIBDB_CLASS_NAME, None, Some(simple(original).to_string()))?;
        if restorable(original) {
            labels.record_value(item, Attribute::Package, LIBDB_CLASS_NAME, None, Some(package(original).to_string()))?;
        }
        if let Some((coord, app)) = tag_of(pc.0, pc.1) {
            tags.push((item, coord, app));
        }
    }

    // ---- fields ----
    // Which pack class each app class is: an S class name, else unanimous matches (at least two),
    // else the matcher's class vote.
    // (pack class, strong): strong identities (an S class name, or unanimous matches) can back S
    // field names; the matcher's vote only D ones. A class matching several pack classes (R8
    // merged them, sharing fields) has none.
    let mut identity: BTreeMap<usize, ((u16, u32), bool)> = BTreeMap::new();
    for (&ci, pcs) in &exact {
        if let [pc] = pcs.iter().copied().collect::<Vec<_>>()[..] {
            if any_count[&(ci, pc)] >= 2 {
                identity.insert(ci, (pc, class_fits(ci, pc)));
            }
        }
    }
    for (&ci, &(li, dc)) in &matches.classes {
        if (li as usize) < facts.len() && exact.get(&ci).is_none_or(|e| e.len() <= 1) {
            identity.entry(ci).or_insert(((li, dc), false));
        }
    }
    for (ci, original, pc) in &claims {
        if count[original.as_str()] == 1 {
            identity.insert(*ci, (*pc, true));
        }
    }
    // App field → (pack fields it aligned with, whether every supporting match is S).
    type Votes = BTreeMap<(usize, usize), (BTreeSet<(u16, u32)>, bool)>;
    let mut votes: Votes = BTreeMap::new();
    // Distinct non-constructor methods supporting each field.
    let mut support: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();
    let mut rejected: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (&k, &s_tier) in named {
        let m = &matches.methods[k];
        if !from_pack(k) || m.via != "exact:all" {
            continue;
        }
        let (Some(mp), f) = (prints.get(&k), &facts[m.key.0 as usize]) else { continue };
        let Some(theirs) = f.fields.get(&(m.key.1, mp.all)) else { continue };
        if theirs.len() != mp.fields.len() || theirs.iter().zip(&mp.fields).any(|(a, b)| a.0 != b.0) {
            continue;
        }
        for ((_, idx), (_, (fc, fname, fty))) in theirs.iter().zip(&mp.fields) {
            let Some(at) = resolve_field(p, fc, fname, fty) else { continue };
            let Some((owner, _, pty)) = f.field_keys.get(*idx as usize) else {
                rejected.insert(at);
                continue;
            };
            // The field's class must be that pack class: positions align fields of whatever
            // objects the method touches.
            let Some(&(pc, strong)) = identity.get(&at.0) else { continue };
            if pc != (m.key.0, *owner) {
                continue;
            }
            // Same type, program types erased on both sides.
            if erase_type(pty, &stable) != erase_type(fty, &stable) {
                rejected.insert(at);
                continue;
            }
            let e = votes.entry(at).or_insert((BTreeSet::new(), true));
            e.0.insert((m.key.0, *idx));
            e.1 &= s_tier && strong;
            if !s.get(p.classes[m.class].methods[m.method].name).starts_with('<') {
                support.entry(at).or_default().insert((m.class, m.method));
            }
        }
    }
    // Field namespaces: classes linked by superclass or interface edges (field resolution searches
    // a class, its interfaces, then its superclass, so a new name must collide with no field
    // reachable either way). Each namespace is keyed by its smallest member after union, a value
    // that doesn't depend on which link was seen first.
    let mut parent: Vec<usize> = (0..p.classes.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for (k, c) in p.classes.iter().enumerate() {
        for t in c.superclass.iter().chain(&c.interfaces) {
            if let Some(o) = p.find(s.get(*t)) {
                let (a, b) = (find(&mut parent, k), find(&mut parent, o));
                if a != b {
                    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                    parent[hi] = lo;
                }
            }
        }
    }
    let namespace: Vec<usize> = (0..p.classes.len()).map(|k| find(&mut parent, k)).collect();
    let mut names_in: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    for (k, c) in p.classes.iter().enumerate() {
        names_in.entry(namespace[k]).or_default().extend(c.fields.iter().map(|f| s.get(f.name)));
    }
    let mut proposals: BTreeMap<(usize, usize), (String, bool, u16, u32)> = BTreeMap::new();
    for (at, (names, all_s)) in &votes {
        if rejected.contains(at) || names.len() != 1 {
            continue;
        }
        let &(db, idx) = names.iter().next().expect("one");
        let (pc, name, _) = &facts[db as usize].field_keys[idx as usize];
        // Position is proof only where it can't confuse fields: the field's erased type is unique
        // in its class, or two different non-constructor methods agree (constructors storing
        // several same-typed fields align the same whichever overload R8 kept).
        let c = &p.classes[at.0];
        let erased = erase_type(s.get(c.fields[at.1].ty), &stable);
        let alone = c.fields.iter().filter(|f| erase_type(s.get(f.ty), &stable) == erased).count() == 1;
        let corroborated = support.get(at).is_some_and(|x| x.len() >= 2);
        proposals.insert(*at, (name.clone(), *all_s && (alone || corroborated), db, *pc));
    }
    // Two fields of one namespace proposed the same name: neither.
    let mut by_name: BTreeMap<(usize, String), usize> = BTreeMap::new();
    for ((ci, _), (name, ..)) in &proposals {
        let root = namespace[*ci];
        *by_name.entry((root, name.clone())).or_default() += 1;
    }
    for (&(ci, fi), (name, all_s, db, pc)) in &proposals {
        let item = ItemId::Field { class: ClassId(ci as u32), index: fi as u32 };
        let f = &p.classes[ci].fields[fi];
        let current = s.get(f.name);
        if labels.get(item, Attribute::MemberName).is_some() || pins.field(s.get(p.classes[ci].ty), current) {
            continue;
        }
        if by_name[&(namespace[ci], name.clone())] != 1 {
            continue;
        }
        let taken = current != name && names_in[&namespace[ci]].contains(name.as_str());
        if taken {
            continue;
        }
        labels.record_value(item, Attribute::MemberName, if *all_s { LIBDB_FIELD_NAME } else { LIBDB_FIELD_HINT }, None, Some(name.clone()))?;
        // The field's library is its pack class's (field keys index the pack's class table).
        if let Some((coord, app)) = tag_of(*db, *pc) {
            tags.push((item, coord, app));
        }
    }
    Ok(tags)
}
