//! `r8/kept-name`: names that R8's default minifier cannot have produced are original.
//!
//! This asserts S, so it must be sound. The test suite grades every S label this pass emits
//! against R8's held-back mapping file.

use eightr_rules::{Attribute, Source, KEPT_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::ItemId;

pub struct KeptName;

/// Could R8's default naming strategy have produced this identifier? R8's minified names are
/// short runs of ASCII letters; three letters already allows ~140k names per namespace.
pub fn could_be_minified(name: &str) -> bool {
    (1..=3).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphabetic())
}

/// The simple name's last `$`-separated segment, which is what R8 renames for inner classes
/// (`Outer$a`). An empty last segment (name ends in `$`) is treated as the whole name.
fn last_segment(simple: &str) -> &str {
    match simple.rsplit('$').next() {
        Some(s) if !s.is_empty() => s,
        _ => simple,
    }
}

impl Pass for KeptName {
    fn name(&self) -> &'static str {
        "kept-name"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        for id in cx.program.class_ids() {
            let class = cx.program.class(id);
            let item = ItemId::Class { class: id };
            let simple = class.simple_name();
            let class_kept = !could_be_minified(simple) && !could_be_minified(last_segment(simple));
            if class_kept {
                cx.labels.record(item, Attribute::ClassName, KEPT_NAME, None)?;
                // R8 renames a class descriptor as a unit, so a kept simple name implies the
                // package was kept too (a precondition the audit tests check).
                cx.labels.record(item, Attribute::Package, KEPT_NAME, None)?;
            }
            let field_kept: Vec<bool> = class.fields.iter().map(|f| !could_be_minified(&f.name)).collect();
            let method_kept: Vec<bool> = class
                .methods
                .iter()
                .map(|m| m.name == "<init>" || m.name == "<clinit>" || !could_be_minified(&m.name))
                .collect();
            for (index, kept) in field_kept.into_iter().enumerate() {
                if kept {
                    cx.labels.record(ItemId::Field { class: id, index: index as u32 }, Attribute::MemberName, KEPT_NAME, None)?;
                }
            }
            for (index, kept) in method_kept.into_iter().enumerate() {
                if kept {
                    cx.labels.record(ItemId::Method { class: id, index: index as u32 }, Attribute::MemberName, KEPT_NAME, None)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_shapes() {
        for n in ["a", "b", "zz", "Abc", "aB"] {
            assert!(could_be_minified(n), "{n}");
        }
        for n in ["Main", "main", "<init>", "a1", "$r8$classId", "lambda$main$0", "abcd", ""] {
            assert!(!could_be_minified(n), "{n}");
        }
        assert_eq!(last_segment("Main$a"), "a");
        assert_eq!(last_segment("Main$1"), "1");
        assert_eq!(last_segment("Main"), "Main");
        assert_eq!(last_segment("Weird$"), "Weird$");
    }
}
