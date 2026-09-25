//! Differential test: our reader vs. AOSP `dexdump -d` on real D8/R8 output.
//!
//! `cargo xtask fixtures` stores dexdump's output next to each fixture dex. This test parses
//! that text and checks that every class, field, method, code header, instruction mnemonic
//! (at every pc), catch table, and position entry matches what eightr-dex reports.

use std::fs;
use std::path::{Path, PathBuf};

use eightr_dex::code::CodeItem;
use eightr_dex::insn::Decoded;
use eightr_dex::Dex;

#[derive(Debug, Default, PartialEq)]
struct DClass {
    descriptor: String,
    access: u32,
    superclass: Option<String>,
    interfaces: Vec<String>,
    source_file: Option<String>,
    static_fields: Vec<DField>,
    instance_fields: Vec<DField>,
    direct_methods: Vec<DMethod>,
    virtual_methods: Vec<DMethod>,
}

#[derive(Debug, Default, PartialEq)]
struct DField {
    name: String,
    ty: String,
    access: u32,
}

#[derive(Debug, Default, PartialEq)]
struct DMethod {
    name: String,
    ty: String,
    access: u32,
    code: Option<DCode>,
}

/// (start, end, [(type or "<any>", addr)])
type TryBlock = (u32, u32, Vec<(String, u32)>);

#[derive(Debug, Default, PartialEq)]
struct DCode {
    registers: u32,
    ins: u32,
    outs: u32,
    insns_size: u32,
    /// (pc, mnemonic)
    insns: Vec<(u32, String)>,
    catches: Vec<TryBlock>,
    positions: Vec<(u32, i64)>,
}

fn quoted(line: &str) -> String {
    let a = line.find('\'').expect("quote");
    let b = line.rfind('\'').expect("quote");
    line[a + 1..b].to_string()
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).unwrap_or_else(|_| panic!("hex: {s:?}"))
}

fn value(line: &str) -> &str {
    line.split_once(':').expect("colon").1.trim()
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Static,
    Instance,
    Direct,
    Virtual,
}

fn members(c: &mut DClass, section: Section) -> (&mut Vec<DField>, &mut Vec<DMethod>) {
    match section {
        Section::Static => (&mut c.static_fields, &mut c.direct_methods),
        Section::Instance => (&mut c.instance_fields, &mut c.direct_methods),
        Section::Direct => (&mut c.static_fields, &mut c.direct_methods),
        Section::Virtual => (&mut c.static_fields, &mut c.virtual_methods),
    }
}

/// Parses the subset of `dexdump -d` output we compare against.
fn parse_dexdump(text: &str) -> Vec<DClass> {
    let mut classes: Vec<DClass> = Vec::new();
    let mut section = Section::Static;
    let mut in_interfaces = false;
    let mut in_catches = false;
    let mut in_positions = false;

    for line in text.lines() {
        let t = line.trim();
        if line.starts_with("Class #") {
            classes.push(DClass::default());
            continue;
        }
        let Some(c) = classes.last_mut() else { continue };
        let is_method_section = matches!(section, Section::Direct | Section::Virtual);

        if t.starts_with("Class descriptor") {
            c.descriptor = quoted(t);
        } else if t.starts_with("Access flags") {
            c.access = hex(value(t).split_whitespace().next().unwrap());
        } else if t.starts_with("Superclass") {
            c.superclass = Some(quoted(t));
        } else if t.starts_with("Interfaces") {
            in_interfaces = true;
        } else if t.starts_with("Static fields") {
            in_interfaces = false;
            section = Section::Static;
        } else if t.starts_with("Instance fields") {
            section = Section::Instance;
        } else if t.starts_with("Direct methods") {
            section = Section::Direct;
        } else if t.starts_with("Virtual methods") {
            section = Section::Virtual;
        } else if t.starts_with("source_file_idx") {
            // "source_file_idx   : 58 (Opcodes.java)" or "-1 (unknown)"
            let v = value(t);
            c.source_file = if v.starts_with("-1") || v.contains("(unknown)") {
                None
            } else {
                Some(v[v.find('(').unwrap() + 1..v.rfind(')').unwrap()].to_string())
            };
        } else if t.starts_with('#') && t.contains("(in ") {
            in_catches = false;
            in_positions = false;
            if is_method_section {
                members(c, section).1.push(DMethod::default());
            } else {
                members(c, section).0.push(DField::default());
            }
        } else if t.starts_with('#') && in_interfaces {
            c.interfaces.push(quoted(t));
        } else if t.starts_with("name ") {
            let n = quoted(t);
            if is_method_section { members(c, section).1.last_mut().unwrap().name = n } else { members(c, section).0.last_mut().unwrap().name = n }
        } else if t.starts_with("type ") {
            let n = quoted(t);
            if is_method_section { members(c, section).1.last_mut().unwrap().ty = n } else { members(c, section).0.last_mut().unwrap().ty = n }
        } else if t.starts_with("access ") {
            let a = hex(value(t).split_whitespace().next().unwrap());
            if is_method_section { members(c, section).1.last_mut().unwrap().access = a } else { members(c, section).0.last_mut().unwrap().access = a }
        } else if t.starts_with("code          -") {
            members(c, section).1.last_mut().unwrap().code = Some(DCode::default());
        } else if let Some(code) = is_method_section.then(|| members(c, section).1.last_mut()).flatten().and_then(|m| m.code.as_mut()) {
            if t.starts_with("registers") {
                code.registers = value(t).parse().unwrap();
            } else if t.starts_with("ins ") {
                code.ins = value(t).parse().unwrap();
            } else if t.starts_with("outs") {
                code.outs = value(t).parse().unwrap();
            } else if t.starts_with("insns size") {
                code.insns_size = value(t).split_whitespace().next().unwrap().parse().unwrap();
            } else if t.starts_with("catches") {
                in_catches = true;
            } else if t.starts_with("positions") {
                in_catches = false;
                in_positions = true;
            } else if t.starts_with("locals") {
                in_positions = false;
            } else if let Some((_, rest)) = line.split_once('|') {
                // "000918: 2b02 ...   |0000: packed-switch v2, ..."
                if rest.starts_with('[') {
                    continue;
                }
                let (pc, insn) = rest.split_once(": ").unwrap();
                code.insns.push((hex(pc), insn.split_whitespace().next().unwrap().to_string()));
            } else if in_catches && t.starts_with("0x") && t.contains(" - ") {
                let (a, b) = t.split_once(" - ").unwrap();
                code.catches.push((hex(a), hex(b), Vec::new()));
            } else if in_catches && t.contains(" -> ") {
                let (ty, addr) = t.split_once(" -> ").unwrap();
                code.catches.last_mut().unwrap().2.push((ty.to_string(), hex(addr)));
            } else if in_positions && t.starts_with("0x") {
                let (addr, line) = t.split_once(" line=").unwrap();
                code.positions.push((hex(addr), line.parse().unwrap()));
            }
        }
    }
    classes
}

fn payload_name(d: &Decoded) -> &'static str {
    use eightr_dex::insn::Payload::*;
    match d {
        Decoded::Insn(i) => i.name(),
        Decoded::Payload { payload: PackedSwitch { .. }, .. } => "packed-switch-data",
        Decoded::Payload { payload: SparseSwitch { .. }, .. } => "sparse-switch-data",
        Decoded::Payload { payload: FillArrayData { .. }, .. } => "array-data",
    }
}

fn our_code(dex: &Dex, code: &CodeItem) -> DCode {
    let insns = code.decode().unwrap().iter().map(|d| (d.pc(), payload_name(d).to_string())).collect();
    let catches = code
        .tries
        .iter()
        .map(|t| {
            let h = code.handler(t.handler_off).unwrap();
            let mut v: Vec<(String, u32)> =
                h.catches.iter().map(|&(ty, addr)| (dex.type_descriptor(ty).unwrap().into_owned(), addr)).collect();
            if let Some(addr) = h.catch_all {
                v.push(("<any>".to_string(), addr));
            }
            (t.start_addr, t.start_addr + u32::from(t.insn_count), v)
        })
        .collect();
    let positions = dex
        .debug_info(code.debug_info_off)
        .unwrap()
        // R8 shares one debug_info_item (a pc→line table) across methods; entries past this
        // method's code are irrelevant to it, and dexdump omits them.
        .map(|d| d.positions.iter().filter(|p| (p.addr as usize) < code.insns.len()).map(|p| (p.addr, p.line)).collect())
        .unwrap_or_default();
    DCode {
        registers: code.registers_size.into(),
        ins: code.ins_size.into(),
        outs: code.outs_size.into(),
        insns_size: code.insns.len() as u32,
        insns,
        catches,
        positions,
    }
}

fn our_classes(dex: &Dex) -> Vec<DClass> {
    let field = |f: &eightr_dex::class::EncodedField| {
        let id = dex.field_id(f.field_idx).unwrap();
        DField {
            name: dex.string(id.name_idx).unwrap().into_owned(),
            ty: dex.type_descriptor(id.type_idx.into()).unwrap().into_owned(),
            access: f.access_flags,
        }
    };
    let method = |m: &eightr_dex::class::EncodedMethod| {
        let id = dex.method_id(m.method_idx).unwrap();
        DMethod {
            name: dex.string(id.name_idx).unwrap().into_owned(),
            ty: dex.proto_descriptor(id.proto_idx.into()).unwrap(),
            access: m.access_flags,
            code: dex.code_item(m.code_off).unwrap().map(|c| our_code(dex, &c)),
        }
    };
    dex.class_defs()
        .map(|def| {
            let def = def.unwrap();
            let data = dex.class_data(&def).unwrap();
            DClass {
                descriptor: dex.type_descriptor(def.class_idx).unwrap().into_owned(),
                access: def.access_flags,
                superclass: def.superclass_idx.map(|s| dex.type_descriptor(s).unwrap().into_owned()),
                interfaces: dex
                    .type_list(def.interfaces_off)
                    .unwrap()
                    .into_iter()
                    .map(|t| dex.type_descriptor(t).unwrap().into_owned())
                    .collect(),
                source_file: dex.opt_string(def.source_file_idx).unwrap().map(|s| s.into_owned()),
                static_fields: data.static_fields.iter().map(field).collect(),
                instance_fields: data.instance_fields.iter().map(field).collect(),
                direct_methods: data.direct_methods.iter().map(method).collect(),
                virtual_methods: data.virtual_methods.iter().map(method).collect(),
            }
        })
        .collect()
}

pub fn fixture_dexes() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut v = Vec::new();
    for fixture in fs::read_dir(&root).expect("fixtures/out missing; run `cargo xtask fixtures`") {
        for variant in ["d8", "r8"] {
            let p = fixture.as_ref().unwrap().path().join(variant).join("classes.dex");
            if p.exists() {
                v.push(p);
            }
        }
    }
    v.sort();
    assert!(!v.is_empty());
    v
}

#[test]
fn matches_dexdump() {
    for path in fixture_dexes() {
        let bytes = fs::read(&path).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let expected = parse_dexdump(&fs::read_to_string(path.with_file_name("dexdump.txt")).unwrap());
        let actual = our_classes(&dex);
        assert_eq!(actual.len(), expected.len(), "{}: class count", path.display());
        for (a, e) in actual.iter().zip(&expected) {
            assert_eq!(a, e, "{}: class {}", path.display(), e.descriptor);
        }
        // Guard against the dexdump parser silently parsing nothing.
        let insn_count: usize = expected
            .iter()
            .flat_map(|c| c.direct_methods.iter().chain(&c.virtual_methods))
            .filter_map(|m| m.code.as_ref())
            .map(|c| c.insns.len())
            .sum();
        assert!(insn_count > 0, "{}: no instructions parsed from dexdump", path.display());
    }
}

#[test]
fn checksums_and_validation() {
    for path in fixture_dexes() {
        let bytes = fs::read(&path).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        assert!(dex.checksum_ok(), "{}: adler32", path.display());
        assert!(dex.signature_ok(), "{}: sha1", path.display());
        dex.validate().unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}

#[test]
fn opcode_coverage() {
    // The opcodes fixture exists to exercise many formats; make sure it keeps doing so.
    let path = fixture_dexes().into_iter().find(|p| p.to_string_lossy().contains("opcodes/d8")).unwrap();
    let bytes = fs::read(&path).unwrap();
    let dex = Dex::parse(&bytes).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for c in our_classes(&dex) {
        for m in c.direct_methods.iter().chain(&c.virtual_methods) {
            for (_, name) in m.code.iter().flat_map(|c| &c.insns) {
                seen.insert(name.clone());
            }
        }
    }
    for must in [
        "packed-switch-data", "sparse-switch-data", "array-data", "invoke-polymorphic/range",
        "const-wide", "const-wide/high16", "const/high16", "monitor-enter", "filled-new-array",
        "cmp-long", "shl-long", "rem-double", "div-int/lit16", "ushr-int/2addr", "rsub-int", "invoke-static/range",
    ] {
        assert!(seen.contains(must), "opcodes fixture no longer emits {must}; seen: {seen:?}");
    }
}
