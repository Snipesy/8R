use std::fs;
use std::path::{Path, PathBuf};

use eightr_mapping::{Frame, Mapping, MemberKind, Metadata, OriginalRange};

fn fixture_mappings() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut v: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|f| f.unwrap().path().join("r8/mapping.txt"))
        .filter(|p| p.exists())
        .collect();
    v.sort();
    assert!(!v.is_empty());
    v
}

#[test]
fn round_trips_r8_output_exactly() {
    for p in fixture_mappings() {
        let text = fs::read_to_string(&p).unwrap();
        let m = Mapping::parse(&text).unwrap();
        assert_eq!(m.to_string(), text, "{}", p.display());
        assert!(m.warnings.is_empty(), "{}: {:?}", p.display(), m.warnings);
    }
}

const SHAPES: &str = include_str!("../../../fixtures/out/shapes/r8/mapping.txt");

#[test]
fn header() {
    let h = Mapping::parse(SHAPES).unwrap().header();
    assert_eq!(h.compiler.as_deref(), Some("R8"));
    assert_eq!(h.version.as_deref(), Some("2.2"));
    assert_eq!(h.min_api, Some(21));
    assert!(h.pg_map_id.is_some_and(|id| id.len() == 7));
}

#[test]
fn inline_frames_innermost_first() {
    let m = Mapping::parse(SHAPES).unwrap();
    let main = &m.classes[m.by_obfuscated()["com.example.Main"]];
    let frames = main.frames("main", 2);
    assert_eq!(
        frames,
        vec![
            Frame {
                class: "com.example.Main$Circle".into(),
                method: "<init>".into(),
                signature: "void <init>(double)".into(),
                line: Some(10),
            },
            Frame {
                class: "com.example.Main".into(),
                method: "main".into(),
                signature: "void main(java.lang.String[])".into(),
                line: Some(29),
            },
        ]
    );
    // Line 1 is not inlined code: one frame.
    assert_eq!(main.frames("main", 1).len(), 1);
    // Nothing covers line 999.
    assert!(main.frames("main", 999).is_empty());
}

#[test]
fn metadata_attached_to_right_owner() {
    let m = Mapping::parse(SHAPES).unwrap();
    let main = &m.classes[m.by_obfuscated()["com.example.Main"]];
    assert_eq!(main.source_file(), Some("Main.java"));
    assert!(!main.is_synthesized());
    // The rewriteFrame comment follows a member line, so it belongs to that member.
    let with_rewrite = main
        .members
        .iter()
        .find(|mm| mm.metadata.iter().any(|md| matches!(md.parsed, Metadata::RewriteFrame { .. })))
        .expect("rewriteFrame member");
    let MemberKind::Method(meth) = &with_rewrite.kind else { panic!() };
    assert_eq!(meth.original_name, "main");
    // Synthesized enum-unboxing utility class.
    let util = m.classes.iter().find(|c| c.original.ends_with("EnumUnboxingSharedUtility")).unwrap();
    assert!(util.is_synthesized());
}

#[test]
fn line_mapping_arithmetic() {
    let m = Mapping::parse(
        "a.B -> a.B:\n    5:7:void f():20:22 -> f\n    8:9:void g():30:40 -> g\n    0:65535:void h():33:33 -> h\n    void i() -> i\n",
    )
    .unwrap();
    let methods: Vec<_> = m.classes[0].methods().map(|(m, _)| m.clone()).collect();
    assert_eq!(methods[0].original_line(6), Some(21)); // linear
    assert_eq!(methods[0].original_line(8), None);
    assert_eq!(methods[1].original_line(9), Some(30)); // spans differ: collapse to start
    assert_eq!(methods[2].original_line(12345), Some(33));
    assert_eq!(methods[3].original_line(1), None); // no range: covers nothing
    assert_eq!(methods[0].original_range, Some(OriginalRange::Range(20, 22)));
}

#[test]
fn members_with_owners_and_fields() {
    let m = Mapping::parse(
        "x.Y -> a:\n    int x.Z.count -> a\n    java.lang.String name -> b\n    1:1:void x.Z.<init>(int,long[]):0:0 -> <init>\n",
    )
    .unwrap();
    let c = &m.classes[0];
    let fields: Vec<_> = c.fields().collect();
    assert_eq!(fields[0].original_owner.as_deref(), Some("x.Z"));
    assert_eq!(fields[0].original_name, "count");
    assert_eq!(fields[1].original_owner, None);
    let (init, _) = c.methods().next().unwrap();
    assert_eq!(init.original_owner.as_deref(), Some("x.Z"));
    assert_eq!(init.params, vec!["int", "long[]"]);
    assert_eq!(m.to_string(), "x.Y -> a:\n    int x.Z.count -> a\n    java.lang.String name -> b\n    1:1:void x.Z.<init>(int,long[]):0:0 -> <init>\n");
}

#[test]
fn errors_have_line_numbers() {
    let e = Mapping::parse("a -> b:\n    garbage\n").unwrap_err();
    assert_eq!(e.line, 2);
    let e = Mapping::parse("    int x -> a\n").unwrap_err();
    assert_eq!(e.line, 1);
    let e = Mapping::parse("a -> b\n").unwrap_err();
    assert_eq!(e.line, 1);
    let e = Mapping::parse("a -> b:\n    1:x:void f() -> f\n").unwrap_err();
    assert_eq!(e.line, 2);
    let e = Mapping::parse("a -> b:\n    void f(:1 -> f\n").unwrap_err();
    assert_eq!(e.line, 2);
}

#[test]
fn mutated_mappings_never_panic() {
    // Truncate the real mapping at every byte offset and delete every line in turn.
    for cut in 0..SHAPES.len() {
        if SHAPES.is_char_boundary(cut) {
            let _ = Mapping::parse(&SHAPES[..cut]);
        }
    }
    let lines: Vec<&str> = SHAPES.lines().collect();
    for skip in 0..lines.len() {
        let text: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| *i != skip).map(|(_, l)| *l).collect();
        let _ = Mapping::parse(&text.join("\n"));
    }
}

#[test]
fn rejects_backwards_range() {
    assert_eq!(Mapping::parse("a -> b:\n    5:1:void f() -> f\n").unwrap_err().line, 2);
}

#[test]
fn outermost_frames_only() {
    let m = Mapping::parse(SHAPES).unwrap();
    let main = &m.classes[m.by_obfuscated()["com.example.Main"]];
    let names: std::collections::BTreeSet<_> = main.outermost_methods().iter().map(|(m, _)| m.original_name.clone()).collect();
    assert_eq!(names.into_iter().collect::<Vec<_>>(), vec!["main".to_string()]);
    assert!(main.methods().any(|(m, _)| m.original_name == "lambda$main$0"));
}
