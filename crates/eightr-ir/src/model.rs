//! The whole-program model: every class with its members, code, annotations, and static
//! values, fully resolved to interned references. It can be loaded from dex files, renamed
//! ([`crate::rename`]), and written back out (the `eightr-dexwrite` crate).

use std::collections::BTreeMap;

use eightr_dex::class::access;
use eightr_dex::{Dex, Result};

use crate::lift::{lift, Body};
use crate::resolve::Resolver;
use crate::sym::{Interner, Sym};
use crate::value::{Annotation, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: Sym,
    pub ty: Sym,
    pub access: u32,
    /// Explicit initial value (static fields only), as stored in the dex.
    pub static_value: Option<Value>,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub name: Sym,
    /// Method descriptor.
    pub proto: Sym,
    pub access: u32,
    pub code: Option<Body>,
    pub annotations: Vec<Annotation>,
    /// One set per parameter, when the method has parameter annotations at all.
    pub parameter_annotations: Option<Vec<Vec<Annotation>>>,
}

impl Method {
    pub fn is_static(&self) -> bool {
        self.access & access::STATIC != 0
    }

    /// Dex puts static, private, and constructor methods in `direct_methods`.
    pub fn is_direct(&self) -> bool {
        self.access & (access::STATIC | access::PRIVATE | access::CONSTRUCTOR) != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub ty: Sym,
    pub access: u32,
    pub superclass: Option<Sym>,
    pub interfaces: Vec<Sym>,
    pub source_file: Option<Sym>,
    pub annotations: Vec<Annotation>,
    /// Static fields first, then instance fields (dex order within each group).
    pub fields: Vec<Field>,
    /// Direct methods first, then virtual methods (dex order within each group).
    pub methods: Vec<Method>,
    /// Index of the input this class was loaded from (for diagnostics only).
    pub origin: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    /// Sorted by descriptor string.
    pub classes: Vec<Class>,
    pub syms: Interner,
    /// Strings no code references but that must survive a rewrite: build-tool markers
    /// (`~~R8{...}`, `~~D8{...}`). Sorted, deduplicated.
    pub retained_strings: Vec<String>,
}

/// Is `s` a D8/R8/L8-style marker (`~~Tool{json}`)?
pub fn is_marker(s: &str) -> bool {
    s.strip_prefix("~~").and_then(|r| r.find('{').map(|b| b > 0 && r[..b].bytes().all(|c| c.is_ascii_alphanumeric()))).unwrap_or(false)
}

#[derive(Debug)]
pub enum LoadError {
    Dex { input: usize, error: eightr_dex::DexError },
    DuplicateClass { descriptor: String, inputs: [usize; 2] },
}

fn load_class(r: &mut Resolver, dex: &Dex, def: &eightr_dex::class::ClassDef, origin: usize) -> Result<Class> {
    let ty = r.ty(def.class_idx)?;
    let superclass = def.superclass_idx.map(|s| r.ty(s)).transpose()?;
    let interfaces = dex.type_list(def.interfaces_off)?.into_iter().map(|t| r.ty(t)).collect::<Result<_>>()?;
    let source_file = def.source_file_idx.map(|s| r.string(s)).transpose()?;
    let dir = dex.annotations_directory(def.annotations_off)?.unwrap_or_default();
    let annotations = r.annotation_set(dir.class_annotations_off)?;
    let data = dex.class_data(def)?;
    let statics = dex.static_values(def)?;

    let mut fields = Vec::new();
    for (i, f) in data.fields().enumerate() {
        let fref = r.field(f.field_idx)?;
        let static_value = if i < data.static_fields.len() { statics.get(i).map(|v| r.value(v)).transpose()? } else { None };
        let annotations = match dir.fields.iter().find(|(idx, _)| *idx == f.field_idx) {
            Some(&(_, off)) => r.annotation_set(off)?,
            None => Vec::new(),
        };
        fields.push(Field { name: fref.name, ty: fref.ty, access: f.access_flags, static_value, annotations });
    }

    let mut methods = Vec::new();
    for m in data.methods() {
        let mref = r.method(m.method_idx)?;
        let code = match dex.code_item(m.code_off)? {
            Some(c) => Some(lift(dex, &c, r.syms)?),
            None => None,
        };
        let annotations = match dir.methods.iter().find(|(idx, _)| *idx == m.method_idx) {
            Some(&(_, off)) => r.annotation_set(off)?,
            None => Vec::new(),
        };
        let parameter_annotations = match dir.parameters.iter().find(|(idx, _)| *idx == m.method_idx) {
            Some(&(_, off)) => {
                let sets = dex.annotation_set_ref_list(off)?;
                Some(sets.into_iter().map(|s| r.annotation_set(s)).collect::<Result<_>>()?)
            }
            None => None,
        };
        methods.push(Method { name: mref.name, proto: mref.proto, access: m.access_flags, code, annotations, parameter_annotations });
    }
    Ok(Class { ty, access: def.access_flags, superclass, interfaces, source_file, annotations, fields, methods, origin })
}

impl Program {
    /// Loads every class from `dexes` (in any order; the result is sorted by descriptor).
    pub fn load(dexes: &[&Dex]) -> std::result::Result<Program, LoadError> {
        let mut syms = Interner::default();
        let mut by_desc: BTreeMap<String, Class> = BTreeMap::new();
        let mut retained = std::collections::BTreeSet::new();
        for (input, dex) in dexes.iter().enumerate() {
            for st in dex.strings() {
                let st = st.map_err(|error| LoadError::Dex { input, error })?;
                if is_marker(&st) {
                    retained.insert(st.into_owned());
                }
            }
            let mut r = Resolver { dex, syms: &mut syms };
            for def in dex.class_defs() {
                let class = def.and_then(|d| load_class(&mut r, dex, &d, input)).map_err(|error| LoadError::Dex { input, error })?;
                let desc = r.syms.get(class.ty).to_string();
                if let Some(prev) = by_desc.get(&desc) {
                    return Err(LoadError::DuplicateClass { descriptor: desc, inputs: [prev.origin, input] });
                }
                by_desc.insert(desc, class);
            }
        }
        Ok(Program { classes: by_desc.into_values().collect(), syms, retained_strings: retained.into_iter().collect() })
    }

    pub fn descriptor(&self, class: &Class) -> &str {
        self.syms.get(class.ty)
    }

    /// Index of the class with this descriptor.
    pub fn find(&self, descriptor: &str) -> Option<usize> {
        self.classes.binary_search_by(|c| self.syms.get(c.ty).cmp(descriptor)).ok()
    }

    /// Re-sorts classes by descriptor (after renaming).
    pub fn sort(&mut self) {
        let syms = &self.syms;
        self.classes.sort_by(|a, b| syms.get(a.ty).cmp(syms.get(b.ty)));
    }
}
