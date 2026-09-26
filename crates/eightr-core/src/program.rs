//! The pipeline's view of the program: the shared [`eightr_ir::model::Program`] plus stable
//! item ids for labels and reports.

use eightr_ir::model;
use serde::Serialize;

pub use eightr_ir::model::{Class, Field, Method};

/// Stable handle to a class: its index in the (descriptor-sorted) model. Passes that rename
/// must not re-sort until labels are finalized.
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
    pub model: model::Program,
}

/// `Lcom/example/Main$Inner;` → `com/example`
pub fn package_of(descriptor: &str) -> &str {
    let inner = descriptor.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(descriptor);
    inner.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// `Lcom/example/Main$Inner;` → `Main$Inner`
pub fn simple_name_of(descriptor: &str) -> &str {
    let inner = descriptor.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(descriptor);
    inner.rsplit_once('/').map(|(_, n)| n).unwrap_or(inner)
}

impl Program {
    pub fn class(&self, id: ClassId) -> &Class {
        &self.model.classes[id.0 as usize]
    }

    pub fn class_ids(&self) -> impl Iterator<Item = ClassId> {
        (0..self.model.classes.len() as u32).map(ClassId)
    }

    pub fn str(&self, s: eightr_ir::Sym) -> &str {
        self.model.syms.get(s)
    }

    pub fn descriptor(&self, id: ClassId) -> &str {
        self.str(self.class(id).ty)
    }

    pub fn find(&self, descriptor: &str) -> Option<ClassId> {
        self.model.find(descriptor).map(|i| ClassId(i as u32))
    }

    /// Human-readable item name for reports: `Lcom/Foo;`, `Lcom/Foo;->bar:I`, `Lcom/Foo;->baz()V`.
    pub fn describe(&self, item: ItemId) -> String {
        match item {
            ItemId::Class { class } => self.descriptor(class).to_string(),
            ItemId::Field { class, index } => {
                let f = &self.class(class).fields[index as usize];
                format!("{}->{}:{}", self.descriptor(class), self.str(f.name), self.str(f.ty))
            }
            ItemId::Method { class, index } => {
                let m = &self.class(class).methods[index as usize];
                format!("{}->{}{}", self.descriptor(class), self.str(m.name), self.str(m.proto))
            }
        }
    }
}
