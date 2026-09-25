//! The program model un-passes operate on. For now this is a structural view of the input
//! (classes, members, and where their code lives); an instruction-level IR comes later.

use std::collections::BTreeMap;

use eightr_dex::{class::access, Dex};
use serde::Serialize;

use crate::error::{Error, Result};

/// Stable handle to a class. Assigned once, in canonical order, when the program is built;
/// renaming an item never changes its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ClassId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemId {
    Class { class: ClassId },
    Field { class: ClassId, index: u32 },
    Method { class: ClassId, index: u32 },
}

#[derive(Debug, Clone)]
pub struct Program {
    pub classes: Vec<Class>,
    /// Descriptor → id.
    pub by_descriptor: BTreeMap<String, ClassId>,
}

#[derive(Debug, Clone)]
pub struct Class {
    pub descriptor: String,
    pub access: u32,
    pub superclass: Option<String>,
    pub interfaces: Vec<String>,
    pub source_file: Option<String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    /// Index into the pipeline's input list.
    pub input: usize,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub access: u32,
}

#[derive(Debug, Clone)]
pub struct Method {
    pub name: String,
    /// `(I)V`
    pub proto: String,
    pub access: u32,
    /// File offset of the code_item in the class's input dex; `None` for abstract/native.
    pub code_off: Option<u32>,
}

impl Class {
    /// `Lcom/example/Main$Inner;` → `com/example`
    pub fn package(&self) -> &str {
        let inner = self.descriptor.trim_start_matches('L').trim_end_matches(';');
        inner.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
    }

    /// `Lcom/example/Main$Inner;` → `Main$Inner`
    pub fn simple_name(&self) -> &str {
        let inner = self.descriptor.trim_start_matches('L').trim_end_matches(';');
        inner.rsplit_once('/').map(|(_, n)| n).unwrap_or(inner)
    }

    pub fn is_interface(&self) -> bool {
        self.access & access::INTERFACE != 0
    }
}

impl Program {
    /// Builds the program from parsed dex files (in canonical input order).
    pub fn build(dexes: &[(String, Dex)]) -> Result<Program> {
        let dex_err = |input: &str| {
            let input = input.to_string();
            move |error| Error::Dex { input: input.clone(), error }
        };
        let mut classes: BTreeMap<String, Class> = BTreeMap::new();
        for (input, (name, dex)) in dexes.iter().enumerate() {
            let e = dex_err(name);
            for def in dex.class_defs() {
                let def = def.map_err(&e)?;
                let descriptor = dex.type_descriptor(def.class_idx).map_err(&e)?.into_owned();
                if let Some(existing) = classes.get(&descriptor) {
                    return Err(Error::DuplicateClass {
                        descriptor,
                        inputs: [dexes[existing.input].0.clone(), name.clone()],
                    });
                }
                let data = dex.class_data(&def).map_err(&e)?;
                let mut fields = Vec::new();
                for f in data.fields() {
                    let id = dex.field_id(f.field_idx).map_err(&e)?;
                    fields.push(Field {
                        name: dex.string(id.name_idx).map_err(&e)?.into_owned(),
                        ty: dex.type_descriptor(id.type_idx.into()).map_err(&e)?.into_owned(),
                        access: f.access_flags,
                    });
                }
                let mut methods = Vec::new();
                for m in data.methods() {
                    let id = dex.method_id(m.method_idx).map_err(&e)?;
                    methods.push(Method {
                        name: dex.string(id.name_idx).map_err(&e)?.into_owned(),
                        proto: dex.proto_descriptor(id.proto_idx.into()).map_err(&e)?,
                        access: m.access_flags,
                        code_off: (m.code_off != 0).then_some(m.code_off),
                    });
                }
                let class = Class {
                    superclass: def.superclass_idx.map(|s| dex.type_descriptor(s).map(|d| d.into_owned())).transpose().map_err(&e)?,
                    interfaces: dex
                        .type_list(def.interfaces_off)
                        .map_err(&e)?
                        .into_iter()
                        .map(|t| dex.type_descriptor(t).map(|d| d.into_owned()))
                        .collect::<eightr_dex::Result<_>>()
                        .map_err(&e)?,
                    source_file: dex.opt_string(def.source_file_idx).map_err(&e)?.map(|s| s.into_owned()),
                    access: def.access_flags,
                    fields,
                    methods,
                    input,
                    descriptor: descriptor.clone(),
                };
                classes.insert(descriptor, class);
            }
        }
        // Ids follow descriptor order: independent of dex file order and class_def order.
        let classes: Vec<Class> = classes.into_values().collect();
        let by_descriptor = classes.iter().enumerate().map(|(i, c)| (c.descriptor.clone(), ClassId(i as u32))).collect();
        Ok(Program { classes, by_descriptor })
    }

    pub fn class(&self, id: ClassId) -> &Class {
        &self.classes[id.0 as usize]
    }

    pub fn class_ids(&self) -> impl Iterator<Item = ClassId> {
        (0..self.classes.len() as u32).map(ClassId)
    }

    /// Human-readable item name for reports: `Lcom/Foo;`, `Lcom/Foo;->bar:I`, `Lcom/Foo;->baz()V`.
    pub fn describe(&self, item: ItemId) -> String {
        match item {
            ItemId::Class { class } => self.class(class).descriptor.clone(),
            ItemId::Field { class, index } => {
                let c = self.class(class);
                let f = &c.fields[index as usize];
                format!("{}->{}:{}", c.descriptor, f.name, f.ty)
            }
            ItemId::Method { class, index } => {
                let c = self.class(class);
                let m = &c.methods[index as usize];
                format!("{}->{}{}", c.descriptor, m.name, m.proto)
            }
        }
    }
}
