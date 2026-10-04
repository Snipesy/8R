//! Reflective name references: code that looks up a class or member **by a name string**
//! (`Class.forName`, `getDeclaredField`, `AtomicReferenceFieldUpdater.newUpdater`, ...). R8
//! rewrites these strings when it renames the target, so renaming must do the same, or pin
//! the target when the string can't be tied to exactly one declaration.

use std::collections::{BTreeMap, BTreeSet};

use crate::cfg::Cfg;
use crate::defs::{DefSite, ReachingDefs};
use crate::model::Program;
use crate::op::{InvokeKind, Op, Reg};
use crate::types::parse_proto;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `newUpdater(Class, [Class,] String)`: a field declared in the class.
    UpdaterField,
    /// `Class.getDeclaredField(String)`: a field declared in the receiver.
    DeclaredField,
    /// `Class.getField(String)`: a public field of the receiver or its supertypes.
    PublicField,
    /// `Class.getDeclaredMethod` / `getMethod`: a method (parameter types are dynamic).
    Method,
    /// `Class.forName(String, ...)`: a class by binary name.
    Class,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Indices of the method containing the lookup.
    pub class: usize,
    pub method: usize,
    /// Instruction index of the `const-string` that supplies the name, when that string is
    /// used *only* by reflective lookups (so rewriting it can't change anything else).
    pub string_insn: Option<u32>,
    pub kind: Kind,
    /// The looked-up name (dotted for classes).
    pub name: String,
    /// Owner class descriptor, when it is a known constant.
    pub owner: Option<String>,
}

/// (owner class, method name, descriptor, index of the name argument, index of the owner
/// argument or None for "the receiver")
struct Api {
    class: &'static str,
    name: &'static str,
    kind: Kind,
    name_arg: usize,
    owner_arg: Option<usize>,
}

const APIS: &[Api] = &[
    Api { class: "Ljava/util/concurrent/atomic/AtomicIntegerFieldUpdater;", name: "newUpdater", kind: Kind::UpdaterField, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/util/concurrent/atomic/AtomicLongFieldUpdater;", name: "newUpdater", kind: Kind::UpdaterField, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/util/concurrent/atomic/AtomicReferenceFieldUpdater;", name: "newUpdater", kind: Kind::UpdaterField, name_arg: 2, owner_arg: Some(0) },
    Api { class: "Ljava/lang/Class;", name: "getDeclaredField", kind: Kind::DeclaredField, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/lang/Class;", name: "getField", kind: Kind::PublicField, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/lang/Class;", name: "getDeclaredMethod", kind: Kind::Method, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/lang/Class;", name: "getMethod", kind: Kind::Method, name_arg: 1, owner_arg: Some(0) },
    Api { class: "Ljava/lang/Class;", name: "forName", kind: Kind::Class, name_arg: 0, owner_arg: None },
];

/// Argument registers of an invoke, one entry per *argument* (wide args take two registers).
fn arg_regs(proto: &str, is_static: bool, regs: &[Reg]) -> Option<Vec<Reg>> {
    let (params, _) = parse_proto(proto)?;
    let mut out = Vec::new();
    let mut i = 0;
    if !is_static {
        out.push(*regs.first()?);
        i = 1;
    }
    for p in params {
        out.push(*regs.get(i)?);
        i += if matches!(p, "J" | "D") { 2 } else { 1 };
    }
    Some(out)
}

pub fn sites(p: &Program) -> Vec<Site> {
    let s = &p.syms;
    let mut out = Vec::new();
    for (ci, c) in p.classes.iter().enumerate() {
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(body) = &m.code else { continue };
            let calls: Vec<(u32, &Api, Vec<Reg>)> = body
                .insns
                .iter()
                .enumerate()
                .filter_map(|(k, insn)| {
                    let Op::Invoke { kind, method, args } = &insn.op else { return None };
                    let api = APIS.iter().find(|a| s.get(method.class) == a.class && s.get(method.name) == a.name)?;
                    let regs = arg_regs(s.get(method.proto), *kind == InvokeKind::Static, args)?;
                    Some((k as u32, api, regs))
                })
                .collect();
            if calls.is_empty() {
                continue;
            }
            let Ok(cfg) = Cfg::build(body) else { continue };
            let rd = ReachingDefs::compute(body, &cfg);
            // Single constant definition of `reg` reaching instruction `at`.
            let single_def = |at: u32, reg: Reg| -> Option<u32> {
                let uses = rd.uses[at as usize].as_ref()?;
                let (_, defs) = uses.iter().find(|(r, _)| *r == reg)?;
                match defs.as_slice() {
                    [d] => match rd.defs[*d].site {
                        DefSite::Insn(i) => Some(i),
                        DefSite::Param => None,
                    },
                    _ => None,
                }
            };
            for (k, api, regs) in calls {
                let Some(&name_reg) = regs.get(api.name_arg) else { continue };
                let Some(def) = single_def(k, name_reg) else { continue };
                let Op::ConstString { value, .. } = &body.insns[def as usize].op else { continue };
                let name = s.get(*value).to_string();
                let owner = api.owner_arg.and_then(|o| regs.get(o)).and_then(|&r| single_def(k, r)).and_then(|d| match &body.insns[d as usize].op {
                    Op::ConstClass { ty, .. } => Some(s.get(*ty).to_string()),
                    _ => None,
                });
                out.push(Site { class: ci, method: mi, string_insn: Some(def), kind: api.kind, name, owner });
            }
            // A string is rewritable only if every read of it is a reflective name argument.
            let reflective: BTreeSet<(u32, Reg)> = body
                .insns
                .iter()
                .enumerate()
                .filter_map(|(k, insn)| {
                    let Op::Invoke { kind, method, args } = &insn.op else { return None };
                    let api = APIS.iter().find(|a| s.get(method.class) == a.class && s.get(method.name) == a.name)?;
                    let regs = arg_regs(s.get(method.proto), *kind == InvokeKind::Static, args)?;
                    Some((k as u32, *regs.get(api.name_arg)?))
                })
                .collect();
            for site in out.iter_mut().filter(|x| x.class == ci && x.method == mi) {
                let Some(def) = site.string_insn else { continue };
                let only_reflective = rd.uses.iter().enumerate().all(|(k, uses)| {
                    uses.iter().flatten().all(|(reg, defs)| {
                        let reads_it = defs.iter().any(|&d| rd.defs[d].site == DefSite::Insn(def));
                        !reads_it || reflective.contains(&(k as u32, *reg))
                    })
                });
                if !only_reflective {
                    site.string_insn = None;
                }
            }
        }
    }
    out
}

/// Names that must not change: reflective lookups whose target can't be tied to a single
/// declaration, or whose name string is also used for something else.
#[derive(Debug, Default, Clone)]
pub struct Pins {
    /// (owner descriptor or None = any class, field name)
    pub fields: BTreeSet<(Option<String>, String)>,
    /// (owner descriptor or None = any class, method name). Method lookups always pin: their
    /// parameter types are built at run time.
    pub methods: BTreeSet<(Option<String>, String)>,
    /// Class descriptors looked up by a name string that can't be rewritten.
    pub classes: BTreeSet<String>,
}

impl Pins {
    pub fn of(sites: &[Site]) -> Pins {
        let mut p = Pins::default();
        for x in sites {
            match x.kind {
                Kind::Method => {
                    p.methods.insert((x.owner.clone(), x.name.clone()));
                }
                Kind::Class if x.string_insn.is_none() => {
                    p.classes.insert(format!("L{};", x.name.replace('.', "/")));
                }
                Kind::Class => {}
                Kind::UpdaterField | Kind::DeclaredField | Kind::PublicField => {
                    // getField searches supertypes; resolving that precisely isn't worth it.
                    let rewritable = x.string_insn.is_some() && x.owner.is_some() && x.kind != Kind::PublicField;
                    if !rewritable {
                        p.fields.insert((if x.kind == Kind::PublicField { None } else { x.owner.clone() }, x.name.clone()));
                    }
                }
            }
        }
        p
    }

    pub fn field(&self, owner: &str, name: &str) -> bool {
        self.fields.contains(&(Some(owner.to_string()), name.to_string())) || self.fields.contains(&(None, name.to_string()))
    }

    pub fn method(&self, owner: &str, name: &str) -> bool {
        self.methods.contains(&(Some(owner.to_string()), name.to_string())) || self.methods.contains(&(None, name.to_string()))
    }

    pub fn class(&self, desc: &str) -> bool {
        self.classes.contains(desc)
    }
}

/// Every name that must not change, from all name-observation evidence in the program:
/// * reflective lookup sites that can't be rewritten (see [`Pins::of`]);
/// * members whose name appears as a string constant in their declaring class's code or
///   static values: such tables are how serializers and protobuf-style runtimes look fields
///   up by name (library-agnostic; costs only a rename, never behavior);
/// * `native` methods and their classes: native code binds to them by name (JNI).
pub fn pins(p: &Program) -> Pins {
    let all = sites(p);
    let mut pins = Pins::of(&all);
    // One name string feeding several lookups (Guava's AbstractFuture: `"a"` for the updaters of
    // two classes): the string can only follow one rename, so every target keeps its name.
    let mut by_string: BTreeMap<(usize, usize, u32), Vec<&Site>> = BTreeMap::new();
    for x in &all {
        if let Some(insn) = x.string_insn {
            by_string.entry((x.class, x.method, insn)).or_default().push(x);
        }
    }
    for group in by_string.values().filter(|g| g.len() > 1) {
        for x in group {
            match x.kind {
                Kind::Class => {
                    pins.classes.insert(format!("L{};", x.name.replace('.', "/")));
                }
                _ => {
                    pins.fields.insert((x.owner.clone(), x.name.clone()));
                    pins.methods.insert((x.owner.clone(), x.name.clone()));
                }
            }
        }
    }
    let s = &p.syms;
    // Class binary names as strings anywhere in code or static values (R8's
    // `-adaptclassstrings` output, e.g. Hilt's `@LazyClassKey` map keys compared with
    // `Class.getName()`): R8 rewrote them to the input names, so the classes keep them.
    // Package-qualified names only: default-package names (`"r"`) are everyday strings (a
    // serial name, a default value), and pinning on them would cost recovered names.
    let binary: BTreeMap<String, String> = p
        .classes
        .iter()
        .filter_map(|c| {
            let d = s.get(c.ty);
            let inner = d.strip_prefix('L')?.strip_suffix(';')?;
            inner.contains('/').then(|| (inner.replace('/', "."), d.to_string()))
        })
        .collect();
    for c in &p.classes {
        let mut strings: BTreeSet<&str> = BTreeSet::new();
        for insn in c.methods.iter().flat_map(|m| m.code.iter().flat_map(|b| &b.insns)) {
            if let Op::ConstString { value, .. } = &insn.op {
                strings.insert(s.get(*value));
            }
        }
        for f in &c.fields {
            collect_value_strings(f.static_value.as_ref(), s, &mut strings);
        }
        for t in strings {
            if let Some(d) = binary.get(t) {
                pins.classes.insert(d.clone());
            }
        }
    }
    for c in &p.classes {
        let owner = s.get(c.ty).to_string();
        let mut strings: BTreeSet<&str> = BTreeSet::new();
        for m in &c.methods {
            for insn in m.code.iter().flat_map(|b| &b.insns) {
                if let Op::ConstString { value, .. } = &insn.op {
                    strings.insert(s.get(*value));
                }
            }
        }
        for f in &c.fields {
            collect_value_strings(f.static_value.as_ref(), s, &mut strings);
        }
        for f in &c.fields {
            let n = s.get(f.name);
            if strings.contains(n) {
                pins.fields.insert((Some(owner.clone()), n.to_string()));
            }
        }
        let mut has_native = false;
        for m in &c.methods {
            let n = s.get(m.name);
            if m.access & eightr_dex::class::access::NATIVE != 0 {
                has_native = true;
                pins.methods.insert((Some(owner.clone()), n.to_string()));
            } else if strings.contains(n) {
                pins.methods.insert((Some(owner.clone()), n.to_string()));
            }
        }
        if has_native {
            pins.classes.insert(owner);
        }
    }
    pins
}

fn collect_value_strings<'a>(v: Option<&crate::value::Value>, s: &'a crate::Interner, out: &mut BTreeSet<&'a str>) {
    match v {
        Some(crate::value::Value::String(x)) => {
            out.insert(s.get(*x));
        }
        Some(crate::value::Value::Array(a)) => {
            for x in a {
                collect_value_strings(Some(x), s, out);
            }
        }
        _ => {}
    }
}
