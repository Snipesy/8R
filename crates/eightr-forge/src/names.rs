//! Residual → original keys from a scenario build's R8 mapping. The forge is the only place a
//! mapping is read (8R never takes one as input).

use std::collections::{BTreeMap, BTreeSet};

use eightr_mapping::{Mapping, Metadata};

pub fn descriptor(dotted: &str) -> String {
    match dotted {
        "void" => "V".into(),
        "boolean" => "Z".into(),
        "byte" => "B".into(),
        "char" => "C".into(),
        "short" => "S".into(),
        "int" => "I".into(),
        "long" => "J".into(),
        "float" => "F".into(),
        "double" => "D".into(),
        t if t.ends_with("[]") => format!("[{}", descriptor(&t[..t.len() - 2])),
        t => format!("L{};", t.replace('.', "/")),
    }
}

/// Residual → original names from an R8 mapping.
pub struct Names {
    /// Residual class descriptor → original class descriptor.
    classes: BTreeMap<String, String>,
    /// (residual class, residual name, residual proto) → (original owner, original name, original proto).
    methods: BTreeMap<(String, String, String), (String, String, String)>,
    /// Residual classes and methods R8 synthesized: their names are R8's, not the library's.
    pub synthesized_classes: BTreeSet<String>,
    synthesized_methods: BTreeSet<(String, String, String)>,
}

impl Names {
    pub fn new(mapping: &Mapping) -> Names {
        let classes: BTreeMap<String, String> = mapping.classes.iter().map(|c| (descriptor(&c.obfuscated), descriptor(&c.original))).collect();
        let inverse: BTreeMap<&str, &str> = classes.iter().map(|(r, o)| (o.as_str(), r.as_str())).collect();
        let residual_type = |t: &str| -> String {
            let dims = t.bytes().take_while(|&b| b == b'[').count();
            match inverse.get(&t[dims..]) {
                Some(r) => format!("{}{}", &t[..dims], r),
                None => t.to_string(),
            }
        };
        let mut methods = BTreeMap::new();
        let mut synthesized_classes = BTreeSet::new();
        let mut synthesized_methods = BTreeSet::new();
        for c in &mapping.classes {
            let rc = descriptor(&c.obfuscated);
            if c.metadata.iter().any(|x| x.parsed == Metadata::Synthesized) {
                synthesized_classes.insert(rc.clone());
            }
            for (m, md) in c.outermost_methods() {
                let ps: Vec<String> = m.params.iter().map(|t| descriptor(t)).collect();
                let original = format!("({}){}", ps.concat(), descriptor(&m.return_type));
                let residual = md
                    .iter()
                    .find_map(|x| match &x.parsed {
                        Metadata::ResidualSignature(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| format!("({}){}", ps.iter().map(|t| residual_type(t)).collect::<String>(), residual_type(&descriptor(&m.return_type))));
                let key = (rc.clone(), m.obfuscated.clone(), residual);
                if md.iter().any(|x| x.parsed == Metadata::Synthesized) {
                    synthesized_methods.insert(key);
                    continue;
                }
                // A method R8 moved here (or merged into it) keeps its original owner.
                let owner = m.original_owner.as_deref().map_or_else(|| descriptor(&c.original), descriptor);
                methods.insert(key, (owner, m.original_name.clone(), original));
            }
        }
        Names { classes, methods, synthesized_classes, synthesized_methods }
    }

    pub fn class(&self, residual: &str) -> String {
        self.classes.get(residual).cloned().unwrap_or_else(|| residual.to_string())
    }

    /// The original (owner, name, proto) of a residual method; `None` for R8's own methods and
    /// classes. Unmapped methods kept their names, and their proto maps type by type.
    pub fn method(&self, class: &str, name: &str, proto: &str) -> Option<(String, String, String)> {
        let key = (class.to_string(), name.to_string(), proto.to_string());
        if self.synthesized_methods.contains(&key) || self.synthesized_classes.contains(class) {
            return None;
        }
        if let Some(x) = self.methods.get(&key) {
            return Some(x.clone());
        }
        let map = |t: &str| {
            let dims = t.bytes().take_while(|&b| b == b'[').count();
            format!("{}{}", &t[..dims], self.class(&t[dims..]))
        };
        let proto = match eightr_ir::types::parse_proto(proto) {
            Some((ps, r)) => format!("({}){}", ps.iter().map(|t| map(t)).collect::<String>(), map(r)),
            None => proto.to_string(),
        };
        Some((self.class(class), name.to_string(), proto))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Residual → original keys: residualsignature, a method R8 moved (keyed by its original
    /// owner), R8's synthesized methods and classes (no key), and an unmapped kept method.
    #[test]
    fn names_key_by_original_owner_and_skip_synthesized() {
        let text = "\
com.lib.Foo -> a:
    com.lib.Bar field -> a
    1:2:com.lib.Bar make(int):10:11 -> b
      # {\"id\":\"com.android.tools.r8.residualsignature\",\"signature\":\"(I)Lb;\"}
    3:4:void com.lib.Moved.helper(java.lang.String):20:21 -> c
    5:6:void lambda$0():0:0 -> d
      # {\"id\":\"com.android.tools.r8.synthesized\"}
com.lib.Bar -> b:
com.lib.Foo$0 -> c:
# {\"id\":\"com.android.tools.r8.synthesized\"}
    1:2:void m():0:0 -> a
";
        let mapping = Mapping::parse_normalized(text).unwrap();
        let n = Names::new(&mapping);
        assert_eq!(n.method("La;", "b", "(I)Lb;"), Some(("Lcom/lib/Foo;".into(), "make".into(), "(I)Lcom/lib/Bar;".into())));
        assert_eq!(n.method("La;", "c", "(Ljava/lang/String;)V"), Some(("Lcom/lib/Moved;".into(), "helper".into(), "(Ljava/lang/String;)V".into())));
        assert_eq!(n.method("La;", "d", "()V"), None);
        assert_eq!(n.method("Lc;", "a", "()V"), None);
        // Not in the mapping: kept name, types mapped one by one.
        assert_eq!(n.method("La;", "toString", "(Lb;)Ljava/lang/String;"), Some(("Lcom/lib/Foo;".into(), "toString".into(), "(Lcom/lib/Bar;)Ljava/lang/String;".into())));
    }
}
