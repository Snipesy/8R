//! Android resource classes for the scenarios. An app build generates every library's `R`
//! classes with final constant ids; R8 folds `R.id.x` reads into constants, which shrinks the
//! library code around them and changes its inlining (`ViewTreeLifecycleOwner.set` becomes a
//! `setTag(0x7f…, owner)` small enough to inline everywhere). AARs ship only `R.txt`, so the
//! forge writes the `R$<type>` classes the closure's code reads: ids assigned deterministically
//! per (type, name) across the closure, as one merged app would; styleable indices and array
//! lengths from the AARs' `R.txt`. (The ids themselves are app-specific; fingerprints normalize
//! them, `sigdb::print`.)

use std::collections::{BTreeMap, BTreeSet};

use crate::api::Api;
use crate::artifacts::Lib;
use crate::classfile::{ClassWriter, ACC_FINAL, ACC_PUBLIC, ACC_STATIC, ACC_SUPER};

/// `R.txt` facts: styleable index values and styleable array lengths, by name.
#[derive(Default)]
struct RTxt {
    styleable_index: BTreeMap<String, i32>,
    styleable_len: BTreeMap<String, usize>,
    /// Every (type, name) declared.
    declared: BTreeSet<(String, String)>,
}

fn parse_r_txt(text: &str, out: &mut RTxt) {
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w[..] {
            ["int", ty, name, value] => {
                out.declared.insert((ty.to_string(), name.to_string()));
                if ty == "styleable" {
                    let v = value.strip_prefix("0x").map_or_else(|| value.parse().ok(), |h| i32::from_str_radix(h, 16).ok());
                    out.styleable_index.insert(name.to_string(), v.unwrap_or(0));
                }
            }
            ["int[]", "styleable", name, ..] => {
                let body = line.split_once('{').map_or("", |x| x.1).trim_end_matches(['}', ' ']);
                out.styleable_len.insert(name.to_string(), body.split(',').filter(|x| !x.trim().is_empty()).count());
            }
            _ => {}
        }
    }
}

/// The resource classes the closure reads but doesn't define: (jar entry, class file), sorted.
pub fn r_classes(api: &Api, libs: &[Lib]) -> Vec<(String, Vec<u8>)> {
    let mut rt = RTxt::default();
    for l in libs {
        if let Some(t) = &l.r_txt {
            parse_r_txt(t, &mut rt);
        }
    }
    let refs: Vec<&(String, String, String)> = api.r_refs.iter().filter(|r| !api.owner.contains_key(&r.0)).collect();
    // Ids: 0x7f | type | entry, types and names in sorted order over everything referenced or declared.
    let ty_of = |class: &str| class.rsplit('/').next().and_then(|x| x.strip_prefix("R$")).unwrap_or("").trim_end_matches(';').to_string();
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (t, n) in &rt.declared {
        names.entry(t.clone()).or_default().insert(n.clone());
    }
    for r in &refs {
        names.entry(ty_of(&r.0)).or_default().insert(r.1.clone());
    }
    let id = |t: &str, n: &str| -> i32 {
        let ti = names.keys().position(|k| k == t).unwrap_or(0);
        let ni = names.get(t).and_then(|s| s.iter().position(|x| x == n)).unwrap_or(0);
        (0x7f00_0000u32 | ((ti as u32 + 1) & 0xff) << 16 | (ni as u32 & 0xffff)) as i32
    };
    let mut by_class: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for r in &refs {
        by_class.entry(r.0.as_str()).or_default().push((r.1.as_str(), r.2.as_str()));
    }
    let mut out = Vec::new();
    for (class, fields) in by_class {
        let internal = class.trim_start_matches('L').trim_end_matches(';');
        let ty = ty_of(class);
        let mut w = ClassWriter::default();
        let mut clinit: Vec<u8> = Vec::new();
        let mut max_stack = 0u16;
        for (name, desc) in fields {
            match desc {
                "I" => {
                    let v = if ty == "styleable" { rt.styleable_index.get(name).copied().unwrap_or(0) } else { id(&ty, name) };
                    w.field(ACC_PUBLIC | ACC_STATIC | ACC_FINAL, name, "I", Some(v));
                }
                "[I" => {
                    // `static final int[] name = new int[n]` (the values don't shape the code).
                    w.field(ACC_PUBLIC | ACC_STATIC | ACC_FINAL, name, "[I", None);
                    let n = rt.styleable_len.get(name).copied().unwrap_or(0);
                    let c = w.integer(n as i32);
                    let f = w.field_ref(internal, name, "[I");
                    clinit.push(0x13);
                    clinit.extend(c.to_be_bytes());
                    clinit.extend([0xbc, 10]); // newarray int
                    clinit.push(0xb3); // putstatic
                    clinit.extend(f.to_be_bytes());
                    max_stack = max_stack.max(1);
                }
                _ => {}
            }
        }
        if !clinit.is_empty() {
            clinit.push(0xb1);
            w.method(ACC_STATIC, "<clinit>", "()V", Some((max_stack, 0, clinit)));
        }
        out.push((format!("{internal}.class"), w.finish(internal, "java/lang/Object", ACC_PUBLIC | ACC_FINAL | ACC_SUPER)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_classes_for_undefined_references() {
        let mut api = Api::default();
        api.r_refs.insert(("La/b/R$id;".into(), "view_tree_owner".into(), "I".into()));
        api.r_refs.insert(("La/b/R$styleable;".into(), "Foo".into(), "[I".into()));
        api.r_refs.insert(("La/b/R$styleable;".into(), "Foo_bar".into(), "I".into()));
        let lib = Lib { coord: "a:b:1".into(), jars: vec![], rules: vec![], r_txt: Some("int id view_tree_owner 0x0\nint[] styleable Foo { 0x7f010000, 0x0101 }\nint styleable Foo_bar 1\n".into()) };
        let classes = r_classes(&api, &[lib]);
        let names: Vec<&str> = classes.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["a/b/R$id.class", "a/b/R$styleable.class"]);
        let id = crate::api::parse_class(&classes[0].1).unwrap();
        assert_eq!(id.fields, vec![("view_tree_owner".to_string(), "I".to_string())]);
        // Defined in the closure: nothing generated.
        api.owner.insert("La/b/R$id;".into(), 0);
        assert_eq!(r_classes(&api, &[]).len(), 1);
    }
}
