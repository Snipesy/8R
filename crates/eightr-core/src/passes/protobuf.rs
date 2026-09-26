//! `r8/protobuf-message-name` (S): protobuf-lite message classes named from the package's bundled
//! `.proto` files. protoc's lite codegen keeps a `<FIELD>_FIELD_NUMBER` constant per field (the
//! consumer rules keep their names and values); a class whose (stem, number) set equals exactly
//! one bundled message's (upper-cased field name, number) set, with at least three fields, is
//! that message's class: its simple name is the message name. Smaller or ambiguous matches only
//! hint the structural name (messages not bundled may coincide).

use std::collections::{BTreeMap, BTreeSet};

use eightr_rules::{Attribute, Source, PROTOBUF_MESSAGE_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub struct Protobuf;

impl Pass for Protobuf {
    fn name(&self) -> &'static str {
        "protobuf"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let messages: Vec<crate::protobuf::Message> =
            cx.evidence.resources.iter().filter(|(n, _)| n.ends_with(".proto")).flat_map(|(_, t)| crate::protobuf::parse(t)).collect();
        if messages.is_empty() {
            return Ok(());
        }
        // Field set → messages.
        let mut by_set: BTreeMap<BTreeSet<(String, i64)>, Vec<&crate::protobuf::Message>> = BTreeMap::new();
        for m in &messages {
            let set: BTreeSet<(String, i64)> = m.fields.iter().map(|(n, k)| (crate::protobuf::constant_stem(n), *k)).collect();
            if !set.is_empty() && set.len() == m.fields.len() {
                by_set.entry(set).or_default().push(m);
            }
        }
        let p = &cx.program.model;
        let s = &p.syms;
        let pins = eightr_ir::reflect::pins(p);
        let mut existing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for c in &p.classes {
            let d = s.get(c.ty);
            existing.entry(crate::program::package_of(d).to_string()).or_default().insert(crate::program::simple_name_of(d).to_string());
        }
        let mut found: Vec<(usize, String, bool)> = Vec::new();
        for (ci, c) in p.classes.iter().enumerate() {
            let set: BTreeSet<(String, i64)> = c
                .fields
                .iter()
                .filter(|f| f.access & eightr_dex::class::access::STATIC != 0 && s.get(f.ty) == "I")
                .filter_map(|f| {
                    let stem = s.get(f.name).strip_suffix("_FIELD_NUMBER")?;
                    match f.static_value {
                        Some(eightr_ir::value::Value::Int(k)) => Some((stem.to_string(), i64::from(k))),
                        _ => None,
                    }
                })
                .collect();
            if set.is_empty() {
                continue;
            }
            let Some(ms) = by_set.get(&set) else { continue };
            let names: BTreeSet<&str> = ms.iter().map(|m| m.name.as_str()).collect();
            if let [name] = names.into_iter().collect::<Vec<_>>()[..] {
                found.push((ci, name.to_string(), ms.len() == 1 && set.len() >= 3));
            }
        }
        let mut claims: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (ci, name, _) in &found {
            *claims.entry((crate::program::package_of(s.get(p.classes[*ci].ty)).to_string(), name.clone())).or_default() += 1;
        }
        let mut labels = Vec::new();
        let mut hints = Vec::new();
        for (ci, name, proven) in found {
            let d = s.get(p.classes[ci].ty);
            let pkg = crate::program::package_of(d).to_string();
            let item = ItemId::Class { class: ClassId(ci as u32) };
            let own = crate::program::simple_name_of(d) == name;
            let free = own || (!existing.get(&pkg).is_some_and(|e| e.contains(&name)) && claims.get(&(pkg, name.clone())) == Some(&1));
            if proven && free && cx.labels.get(item, Attribute::ClassName).is_none() && !pins.class(d) {
                labels.push((item, name));
            } else {
                hints.push((ClassId(ci as u32), name));
            }
        }
        for (item, name) in labels {
            cx.labels.record_value(item, Attribute::ClassName, PROTOBUF_MESSAGE_NAME, None, Some(name))?;
        }
        for (id, name) in hints {
            cx.program.class_hints.entry(id).or_insert(name);
        }
        Ok(())
    }
}
