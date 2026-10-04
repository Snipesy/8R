//! The naming stage (DESIGN.md §5.5): every class and member name that no rule proved (S)
//! gets a deterministic structural name `{hint}_{hash}`.
//!
//! * **Hash:** Weisfeiler–Lehman refinement over the program with every non-S name erased:
//!   each round renders every class with non-S program classes replaced by their previous
//!   round's label and non-S member names by `#`, and hashes the rendering. The result depends
//!   only on structure, S names, strings, and library references, so it is α-invariant.
//! * **Hint:** a readable prefix from α-invariant evidence: a non-generated current name, a
//!   library/S supertype, or the member's type.
//! * **Safety:** renaming must not change behavior. Fields, static, and private methods are
//!   always safe. A virtual method is renamed only if its whole override group lives in
//!   classes whose only library supertype is `java.lang.Object` and it doesn't override an
//!   `Object` method; otherwise it could override a library method we can't see, and it keeps
//!   its name (D·id). New names are unique program-wide, so no override or field shadowing can
//!   appear.

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::model::Program as Model;
use eightr_ir::rename::Renaming;
use eightr_rules::{Attribute, Class, STRUCTURAL_NAME, STRUCTURAL_TIE};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::labels::Labels;
use crate::passes::kept_name::may_be_minified;
use crate::program::{package_of, simple_name_of, ClassId, ItemId, Program};
use crate::report::{Finding, Severity};

const WL_ROUNDS: usize = 4;

/// Platform API surface (`cargo xtask platform-api`): every android.jar class, and every
/// overridable platform method whose name is short enough to collide with an R8-generated
/// name (≤ 6 characters). Library supertypes of a shrunk app are platform classes (app
/// libraries are shrunk *into* the program), so this decides whether renaming a virtual
/// method could break an override.
const PLATFORM_API: &str = include_str!("../data/platform-api.txt");
/// The table covers names up to this length.
const PLATFORM_NAME_LIMIT: usize = 6;

struct Platform {
    classes: BTreeSet<&'static str>,
    methods: BTreeSet<(&'static str, &'static str)>,
}

/// Is `desc` a class of the Android platform (android.jar)? A program class with such a name is
/// a stub R8 synthesized for a class newer than min-api.
pub fn is_platform_class(desc: &str) -> bool {
    platform().classes.contains(desc)
}

fn platform() -> &'static Platform {
    static P: std::sync::OnceLock<Platform> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut classes = BTreeSet::new();
        let mut methods = BTreeSet::new();
        let mut section = "";
        for line in PLATFORM_API.lines().filter(|l| !l.starts_with('#')) {
            match line {
                "[methods]" | "[classes]" => section = line,
                _ if section == "[methods]" => {
                    if let Some((n, d)) = line.split_once(' ') {
                        methods.insert((n, d));
                    }
                }
                _ if section == "[classes]" => {
                    classes.insert(line);
                }
                _ => {}
            }
        }
        Platform { classes, methods }
    })
}
const MIN_HEX: usize = 4;

const OBJECT_METHODS: &[(&str, &str)] = &[
    ("equals", "(Ljava/lang/Object;)Z"),
    ("hashCode", "()I"),
    ("toString", "()Ljava/lang/String;"),
    ("clone", "()Ljava/lang/Object;"),
    ("finalize", "()V"),
    ("getClass", "()Ljava/lang/Class;"),
    ("notify", "()V"),
    ("notifyAll", "()V"),
    ("wait", "()V"),
    ("wait", "(J)V"),
    ("wait", "(JI)V"),
];

/// Is `name` already an 8R structural name (`{hint}_{hex}` with an optional `_{n}` tie
/// suffix)? Such names are canonical: re-running 8R leaves them alone.
pub fn is_structural_name(name: &str) -> bool {
    let mut parts = name.split('_');
    let (Some(hint), Some(hex)) = (parts.next(), parts.next()) else { return false };
    let tie_ok = match parts.next() {
        None => true,
        Some(t) => !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && parts.next().is_none(),
    };
    tie_ok
        && hint.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && hint.bytes().all(|b| b.is_ascii_alphanumeric())
        && (MIN_HEX..=16).contains(&hex.len())
        && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hash_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Letters and digits only, not starting with a digit; `None` if nothing usable remains.
fn sanitize(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let t = t.trim_start_matches(|c: char| c.is_ascii_digit()).to_string();
    (!t.is_empty()).then_some(t)
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_ascii_lowercase().to_string() + c.as_str()).unwrap_or_default()
}

pub struct Naming {
    pub renaming: Renaming,
    /// Number of names left as-is because renaming them couldn't be proven safe.
    pub kept_unsafe: usize,
}

struct Ctx<'a> {
    p: &'a Program,
    labels: &'a Labels,
    /// Class ids whose ClassName is S.
    s_classes: BTreeSet<ClassId>,
}

impl Ctx<'_> {
    /// How a member's name appears in normalized renderings: erased (`#`) unless proven; a
    /// proven name with a recovered value renders as that value (the current name may be
    /// R8's arbitrary choice); otherwise `None` (keep the current, original name).
    fn normalized_member_name(&self, item: ItemId) -> Option<String> {
        match self.labels.get(item, Attribute::MemberName) {
            Some(l) if l.class == Class::Solved => l.value.clone(),
            _ => Some("#".to_string()),
        }
    }
    fn s_member(&self, item: ItemId) -> bool {
        self.labels.get(item, Attribute::MemberName).is_some_and(|l| l.class == Class::Solved)
    }
    /// A name another rule recovered (S, or a D value such as a Compose singleton's `lambda$K`).
    fn recovered_member(&self, item: ItemId) -> bool {
        self.labels.get(item, Attribute::MemberName).is_some_and(|l| l.value.is_some())
    }
    fn s_class(&self, id: ClassId) -> bool {
        self.s_classes.contains(&id)
    }
    /// Is this descriptor stable under α: a library type or an S-named program class?
    fn stable_type(&self, d: &str) -> bool {
        let base = d.trim_start_matches('[');
        match self.p.find(base) {
            Some(id) => self.s_class(id) || is_platform_class(base),
            None => true,
        }
    }
}

/// Class descriptors an instruction refers to (types, member owners and signatures).
fn op_types<'p>(p: &'p Program, op: &eightr_ir::op::Op) -> Vec<&'p str> {
    use eightr_ir::op::Op;
    let proto_types = |d: &'p str| -> Vec<&'p str> {
        eightr_ir::types::parse_proto(d).map(|(ps, r)| ps.into_iter().chain(std::iter::once(r)).collect()).unwrap_or_default()
    };
    match op {
        Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. } | Op::NewInstance { ty, .. }
        | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => vec![p.str(*ty)],
        Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. } | Op::StaticPut { field, .. } => {
            vec![p.str(field.class), p.str(field.ty)]
        }
        Op::Invoke { method, .. } => {
            let mut v = vec![p.str(method.class)];
            v.extend(proto_types(p.str(method.proto)));
            v
        }
        _ => Vec::new(),
    }
}

/// Program classes each class refers to (supertypes, member types, code references).
fn references(p: &Program) -> Vec<BTreeSet<usize>> {
    use eightr_ir::op::Op;
    let mut out = vec![BTreeSet::new(); p.model.classes.len()];
    let add = |from: usize, d: &str, out: &mut Vec<BTreeSet<usize>>| {
        if let Some(id) = p.find(d.trim_start_matches('[')) {
            if id.0 as usize != from {
                out[from].insert(id.0 as usize);
            }
        }
    };
    for id in p.class_ids() {
        let i = id.0 as usize;
        let c = p.class(id);
        for t in c.superclass.iter().chain(&c.interfaces) {
            add(i, p.str(*t), &mut out);
        }
        for f in &c.fields {
            add(i, p.str(f.ty), &mut out);
        }
        for m in &c.methods {
            if let Some((params, ret)) = eightr_ir::types::parse_proto(p.str(m.proto)) {
                for t in params.iter().chain(std::iter::once(&ret)) {
                    add(i, t, &mut out);
                }
            }
            for insn in m.code.iter().flat_map(|b| &b.insns) {
                match &insn.op {
                    Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. }
                    | Op::NewInstance { ty, .. } | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => add(i, p.str(*ty), &mut out),
                    Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. }
                    | Op::StaticPut { field, .. } => add(i, p.str(field.class), &mut out),
                    Op::Invoke { method, .. } => add(i, p.str(method.class), &mut out),
                    _ => {}
                }
            }
        }
    }
    out
}

/// WL class labels (hex), indexed by class id. Each round hashes a class's normalized
/// rendering (which embeds the previous labels of everything it refers to) together with the
/// previous labels of every class that refers to *it*, so classes that differ only in how
/// they're used (e.g. empty marker interfaces) are still told apart.
fn class_labels(cx: &Ctx) -> Vec<String> {
    let p = cx.p;
    let n = p.model.classes.len();
    let outgoing = references(p);
    let mut incoming = vec![Vec::new(); n];
    for (from, tos) in outgoing.iter().enumerate() {
        for &to in tos {
            incoming[to].push(from);
        }
    }
    let mut labels = vec!["?".to_string(); n];
    // Precise "erase" maps for every non-S member (declaration keys, pre-rename).
    let mut fields = BTreeMap::new();
    let mut methods = BTreeMap::new();
    for id in p.class_ids() {
        let c = p.class(id);
        let d = p.descriptor(id).to_string();
        for (i, f) in c.fields.iter().enumerate() {
            if let Some(n) = cx.normalized_member_name(ItemId::Field { class: id, index: i as u32 }) {
                fields.insert((d.clone(), p.str(f.name).to_string(), p.str(f.ty).to_string()), n);
            }
        }
        for (i, m) in c.methods.iter().enumerate() {
            if let Some(n) = cx.normalized_member_name(ItemId::Method { class: id, index: i as u32 }) {
                methods.insert((d.clone(), p.str(m.name).to_string(), p.str(m.proto).to_string()), n);
            }
        }
    }
    // Round-invariant: (referring class, method, instruction, referred class).
    let mut sites: Vec<(usize, usize, usize, usize)> = Vec::new();
    for id in p.class_ids() {
        let ci = id.0 as usize;
        for (mi, m) in p.class(id).methods.iter().enumerate() {
            let Some(body) = &m.code else { continue };
            for (k, insn) in body.insns.iter().enumerate() {
                for d in op_types(p, &insn.op) {
                    if let Some(x) = p.find(d.trim_start_matches('[')) {
                        sites.push((ci, mi, k, x.0 as usize));
                    }
                }
            }
        }
    }
    for _ in 0..WL_ROUNDS {
        let mut classes = BTreeMap::new();
        for id in p.class_ids() {
            if !cx.s_class(id) {
                classes.insert(p.descriptor(id).to_string(), format!("L#{};", labels[id.0 as usize]));
            }
        }
        let mut model: Model = p.model.clone();
        // Keep the class order of the original so indices line up (apply() re-sorts).
        let order: Vec<String> = model.classes.iter().map(|c| model.syms.get(c.ty).to_string()).collect();
        Renaming { classes, members: BTreeMap::new(), fields: fields.clone(), methods: methods.clone() }.apply(&mut model);
        let mut next = vec![String::new(); n];
        // After apply(), classes are re-sorted; map back by applying the same class mapping.
        let mut by_new: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, c) in model.classes.iter().enumerate() {
            by_new.entry(model.syms.get(c.ty).to_string()).or_default().push(i);
        }
        let mut used: BTreeMap<String, usize> = BTreeMap::new();
        let mut norm_of = vec![0usize; n];
        for (orig_idx, d) in order.iter().enumerate() {
            let new_d = if cx.s_class(ClassId(orig_idx as u32)) { d.clone() } else { format!("L#{};", labels[orig_idx]) };
            // Several classes can share a label-descriptor; stable sorting keeps their order.
            let k = used.entry(new_d.clone()).or_default();
            norm_of[orig_idx] = by_new[&new_d][*k];
            *k += 1;
        }
        // Where each class is referenced: (referring method's normalized hash, instruction).
        // Members and instructions keep their order through renaming.
        let mut uses: Vec<Vec<String>> = vec![Vec::new(); n];
        let prints: Vec<Vec<String>> = (0..n).map(|ci| model.classes[norm_of[ci]].methods.iter().map(|m| eightr_ir::print::method(m, &model.syms)).collect()).collect();
        let mhash: Vec<Vec<Option<String>>> = (0..n)
            .map(|ci| p.class(ClassId(ci as u32)).methods.iter().zip(&prints[ci]).map(|(m, t)| m.code.as_ref().map(|_| hash_hex(t))).collect())
            .collect();
        for &(ci, mi, k, x) in &sites {
            let mh = mhash[ci][mi].as_deref().unwrap_or_default();
            uses[x].push(format!("{mh}@{k}"));
        }
        let mut prints = prints;
        let mut debug_texts: Vec<String> = Vec::new();
        for orig_idx in 0..n {
            let mut users: Vec<&str> = incoming[orig_idx].iter().map(|&u| labels[u].as_str()).collect();
            users.sort_unstable();
            uses[orig_idx].sort_unstable();
            let text = format!(
                "{}\nused-by {}\nused-at {}",
                eightr_ir::print::class_with_methods(&model.classes[norm_of[orig_idx]], &model.syms, std::mem::take(&mut prints[orig_idx])),
                users.join(","),
                uses[orig_idx].join(",")
            );
            if std::env::var_os("EIGHTR_NAMING_DEBUG").is_some() {
                debug_texts.push(text.clone());
            }
            next[orig_idx] = hash_hex(&text);
        }
        if let Some(dir) = std::env::var_os("EIGHTR_NAMING_DEBUG") {
            debug_texts.sort();
            let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("round-{n:03}.txt")), debug_texts.join("\n=====\n"));
        }
        labels = next;
    }
    labels
}

/// The program field a reference resolves to (the class, its interfaces, then superclasses).
fn resolve_field(p: &Program, class: &str, name: &str, ty: &str) -> Option<(ClassId, u32)> {
    let mut stack = vec![p.find(class)?];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let c = p.class(id);
        if let Some(i) = c.fields.iter().position(|f| p.str(f.name) == name && p.str(f.ty) == ty) {
            return Some((id, i as u32));
        }
        // Superclass last: push it first so interfaces are searched before it.
        if let Some(sid) = c.superclass.and_then(|t| p.find(p.str(t))) {
            stack.push(sid);
        }
        for t in c.interfaces.iter().rev() {
            if let Some(iid) = p.find(p.str(*t)) {
                stack.push(iid);
            }
        }
    }
    None
}

/// The program method a reference resolves to (superclass chain, then interfaces).
fn resolve_method(p: &Program, class: &str, name: &str, proto: &str) -> Option<(ClassId, u32)> {
    let find_in = |id: ClassId| p.class(id).methods.iter().position(|m| p.str(m.name) == name && p.str(m.proto) == proto);
    let mut cur = p.find(class);
    let mut chain = Vec::new();
    while let Some(id) = cur {
        if chain.contains(&id) {
            break;
        }
        chain.push(id);
        if let Some(i) = find_in(id) {
            return Some((id, i as u32));
        }
        cur = p.class(id).superclass.and_then(|t| p.find(p.str(t)));
    }
    let mut queue: Vec<ClassId> = chain.iter().flat_map(|&id| p.class(id).interfaces.iter().filter_map(|t| p.find(p.str(*t)))).collect();
    let mut seen = BTreeSet::new();
    while !queue.is_empty() {
        let id = queue.remove(0);
        if !seen.insert(id) {
            continue;
        }
        if let Some(i) = find_in(id) {
            return Some((id, i as u32));
        }
        queue.extend(p.class(id).interfaces.iter().filter_map(|t| p.find(p.str(*t))));
    }
    None
}

/// Union-find over method override groups among program classes.
struct Groups {
    parent: BTreeMap<(ClassId, u32), (ClassId, u32)>,
}

impl Groups {
    fn find(&mut self, x: (ClassId, u32)) -> (ClassId, u32) {
        let p = *self.parent.get(&x).unwrap_or(&x);
        if p == x {
            return x;
        }
        let r = self.find(p);
        self.parent.insert(x, r);
        r
    }
    fn union(&mut self, a: (ClassId, u32), b: (ClassId, u32)) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Deterministic root: the smaller id (ids are only used internally, never for output).
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(hi, lo);
        }
    }
}

fn is_virtual(m: &eightr_ir::model::Method) -> bool {
    m.access & (access::STATIC | access::PRIVATE | access::CONSTRUCTOR) == 0
}

pub fn name(p: &Program, labels: &mut Labels, findings: &mut Vec<Finding>) -> Result<Naming> {
    // S classes whose name stays (kept): stable in hashing. S classes with a recovered simple name
    // (applied by the pipeline) are hashed like the rest: their input names are R8's choice.
    let s_classes: BTreeSet<ClassId> = p
        .class_ids()
        .filter(|&id| labels.get(ItemId::Class { class: id }, Attribute::ClassName).is_some_and(|l| l.class == Class::Solved && l.value.is_none()))
        .collect();
    let recovered_classes: BTreeMap<ClassId, String> = p
        .class_ids()
        .filter_map(|id| {
            let l = labels.get(ItemId::Class { class: id }, Attribute::ClassName)?;
            (l.class == Class::Solved).then(|| l.value.clone()).flatten().map(|v| (id, v))
        })
        .collect();
    let cx = Ctx { p, labels, s_classes };
    // Members and classes looked up reflectively by a name string that can't be rewritten
    // keep their names (renaming them would break the lookup).
    let pins = eightr_ir::reflect::pins(&p.model);
    let class_label = class_labels(&cx);

    // Supertype closure within the program; library supertypes per class.
    let mut supers: BTreeMap<ClassId, BTreeSet<ClassId>> = BTreeMap::new();
    let mut lib_supers: BTreeMap<ClassId, BTreeSet<String>> = BTreeMap::new();
    for id in p.class_ids() {
        let mut stack = vec![id];
        let mut seen = BTreeSet::new();
        let mut libs = BTreeSet::new();
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            let k = p.class(c);
            for t in k.superclass.iter().chain(&k.interfaces) {
                let d = p.str(*t);
                match p.find(d) {
                    Some(sid) => stack.push(sid),
                    None => {
                        libs.insert(d.to_string());
                    }
                }
            }
        }
        seen.remove(&id);
        supers.insert(id, seen);
        lib_supers.insert(id, libs);
    }

    // Override groups: within a class and its supertypes, the virtual methods with one name and
    // proto are one dispatch slot. That includes a superclass method implementing an interface
    // method only through a subclass (`C extends B implements I`, `B.m` implements `I.m`).
    let mut groups = Groups { parent: BTreeMap::new() };
    for id in p.class_ids() {
        let mut slots: BTreeMap<(&str, &str), (ClassId, u32)> = BTreeMap::new();
        for k in std::iter::once(id).chain(supers[&id].iter().copied()) {
            for (i, m) in p.class(k).methods.iter().enumerate() {
                if !is_virtual(m) {
                    continue;
                }
                match slots.entry((p.str(m.name), p.str(m.proto))) {
                    std::collections::btree_map::Entry::Vacant(e) => {
                        e.insert((k, i as u32));
                    }
                    std::collections::btree_map::Entry::Occupied(e) => groups.union(*e.get(), (k, i as u32)),
                }
            }
        }
    }
    let mut members_of: BTreeMap<(ClassId, u32), Vec<(ClassId, u32)>> = BTreeMap::new();
    for id in p.class_ids() {
        for (i, m) in p.class(id).methods.iter().enumerate() {
            if is_virtual(m) {
                let root = groups.find((id, i as u32));
                members_of.entry(root).or_default().push((id, i as u32));
            }
        }
    }

    // ---- member hashes ----
    let member_hash = |id: ClassId, text: &str| hash_hex(&format!("{}\n{text}", class_label[id.0 as usize]));
    // Render members in a normalized model (non-S classes → labels, non-S member names → #).
    let mut classes_norm = BTreeMap::new();
    for id in p.class_ids() {
        if !cx.s_class(id) {
            classes_norm.insert(p.descriptor(id).to_string(), format!("L#{};", class_label[id.0 as usize]));
        }
    }
    let mut erase_f = BTreeMap::new();
    let mut erase_m = BTreeMap::new();
    for id in p.class_ids() {
        let c = p.class(id);
        let d = p.descriptor(id).to_string();
        for (i, f) in c.fields.iter().enumerate() {
            if let Some(n) = cx.normalized_member_name(ItemId::Field { class: id, index: i as u32 }) {
                erase_f.insert((d.clone(), p.str(f.name).to_string(), p.str(f.ty).to_string()), n);
            }
        }
        for (i, m) in c.methods.iter().enumerate() {
            if let Some(n) = cx.normalized_member_name(ItemId::Method { class: id, index: i as u32 }) {
                erase_m.insert((d.clone(), p.str(m.name).to_string(), p.str(m.proto).to_string()), n);
            }
        }
    }
    let mut norm = p.model.clone();
    let order: Vec<(String, bool)> = p.class_ids().map(|id| (p.descriptor(id).to_string(), cx.s_class(id))).collect();
    Renaming { classes: classes_norm, members: BTreeMap::new(), fields: erase_f, methods: erase_m }.apply(&mut norm);
    // Map original class index → normalized class index.
    let mut by_new: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in norm.classes.iter().enumerate() {
        by_new.entry(norm.syms.get(c.ty).to_string()).or_default().push(i);
    }
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    let mut norm_idx = vec![0usize; order.len()];
    for (orig, (d, s)) in order.iter().enumerate() {
        let key = if *s { d.clone() } else { format!("L#{};", class_label[orig]) };
        let k = used.entry(key.clone()).or_default();
        norm_idx[orig] = by_new[&key][*k];
        *k += 1;
    }
    // Members keep their index order through renaming (apply() doesn't reorder members).
    let field_text = |id: ClassId, i: usize| eightr_ir::print::field(&norm.classes[norm_idx[id.0 as usize]].fields[i], &norm.syms);
    let method_text = |id: ClassId, i: usize| eightr_ir::print::method(&norm.classes[norm_idx[id.0 as usize]].methods[i], &norm.syms);

    // ---- member usage fingerprints ----
    // Where each member is used: (user method's structural hash, instruction index, op). This
    // separates members whose declarations look alike (e.g. many fields of one type).
    let method_hash: BTreeMap<(ClassId, u32), String> = p
        .class_ids()
        .flat_map(|id| (0..p.class(id).methods.len()).map(move |i| (id, i as u32)))
        .map(|(id, i)| ((id, i), member_hash(id, &method_text(id, i as usize))))
        .collect();
    let mut field_uses: BTreeMap<(ClassId, u32), Vec<String>> = BTreeMap::new();
    let mut method_uses: BTreeMap<(ClassId, u32), Vec<String>> = BTreeMap::new();
    for id in p.class_ids() {
        for (mi, m) in p.class(id).methods.iter().enumerate() {
            let Some(body) = &m.code else { continue };
            let user = &method_hash[&(id, mi as u32)];
            for (k, insn) in body.insns.iter().enumerate() {
                use eightr_ir::op::Op;
                let (target, kind) = match &insn.op {
                    Op::InstanceGet { field, .. } | Op::StaticGet { field, .. } => {
                        (resolve_field(p, p.str(field.class), p.str(field.name), p.str(field.ty)).map(|t| (true, t)), "get")
                    }
                    Op::InstancePut { field, .. } | Op::StaticPut { field, .. } => {
                        (resolve_field(p, p.str(field.class), p.str(field.name), p.str(field.ty)).map(|t| (true, t)), "put")
                    }
                    Op::Invoke { method, .. } => {
                        (resolve_method(p, p.str(method.class), p.str(method.name), p.str(method.proto)).map(|t| (false, t)), "call")
                    }
                    _ => (None, ""),
                };
                if let Some((is_field, t)) = target {
                    let entry = format!("{user}@{k}:{kind}");
                    if is_field { field_uses.entry(t).or_default().push(entry) } else { method_uses.entry(t).or_default().push(entry) }
                }
            }
        }
    }
    let usage = |uses: Option<&Vec<String>>| -> String {
        let mut v: Vec<&str> = uses.map(|u| u.iter().map(String::as_str).collect()).unwrap_or_default();
        v.sort_unstable();
        v.join(",")
    };

    // ---- choose names ----
    let mut renaming = Renaming::default();
    let mut kept_unsafe = 0;
    // (item, attribute, name, tie candidates)
    let mut proposals: Vec<(ItemId, Attribute, String, Option<Vec<String>>)> = Vec::new();

    // Classes: unique within their package, against every other class simple name there.
    let mut taken: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in p.class_ids() {
        if cx.s_class(id) {
            let d = p.descriptor(id);
            taken.entry(package_of(d).to_string()).or_default().insert(simple_name_of(d).to_string());
        }
    }
    // Recovered simple names are reserved in their package (the pipeline applies them).
    for (&id, v) in &recovered_classes {
        taken.entry(package_of(p.descriptor(id)).to_string()).or_default().insert(v.clone());
    }
    let mut class_cands: Vec<(String, String, ClassId)> = Vec::new(); // (label, hint, id)
    for id in p.class_ids().filter(|id| !cx.s_class(*id) && !recovered_classes.contains_key(id)) {
        let d = p.descriptor(id);
        // A platform class's name on a program class: a stub R8 synthesized for APIs newer than
        // min-api. On devices that have the API the platform's class is loaded instead, so every
        // reference must keep the name.
        if is_structural_name(simple_name_of(d)) || pins.class(d) || is_platform_class(d) {
            taken.entry(package_of(d).to_string()).or_default().insert(simple_name_of(d).to_string());
            continue;
        }
        let c = p.class(id);
        let simple = simple_name_of(d);
        let tail = simple.rsplit('$').next().unwrap_or(simple);
        let from_name = (!may_be_minified(tail, 64)).then(|| sanitize(tail)).flatten();
        // The nearest stable supertype (breadth-first through program supertypes), e.g. the
        // functional interface of a lambda whose direct superclass is an app class.
        let from_super = {
            let mut queue: std::collections::VecDeque<&str> = c.superclass.iter().chain(&c.interfaces).map(|t| p.str(*t)).collect();
            let mut seen = BTreeSet::new();
            let mut found = None;
            while let Some(t) = queue.pop_front() {
                if !seen.insert(t) || seen.len() > 64 {
                    continue;
                }
                if t != "Ljava/lang/Object;" && cx.stable_type(t) {
                    found = Some(t);
                    break;
                }
                if let Some(sid) = p.find(t) {
                    let k = p.class(sid);
                    queue.extend(k.superclass.iter().chain(&k.interfaces).map(|x| p.str(*x)));
                }
            }
            found.and_then(|t| sanitize(simple_name_of(t).rsplit('$').next().unwrap_or("")))
        };
        let kind = if c.access & access::ANNOTATION != 0 {
            "Annotation"
        } else if c.access & access::INTERFACE != 0 {
            "Interface"
        } else if c.access & access::ENUM != 0 {
            "Enum"
        } else {
            "Class"
        };
        let from_hint = p.class_hints.get(&id).and_then(|h| sanitize(h));
        let hint = from_hint.or(from_name).or(from_super).unwrap_or_else(|| kind.to_string());
        class_cands.push((class_label[id.0 as usize].clone(), hint, id));
    }
    class_cands.sort();
    assign(&mut class_cands, |id| package_of(p.descriptor(*id)).to_string(), &mut taken, findings, "class", |id, name, tie| {
        let d = p.descriptor(*id);
        let pkg = package_of(d);
        let new = if pkg.is_empty() { format!("L{name};") } else { format!("L{pkg}/{name};") };
        renaming.classes.insert(d.to_string(), new);
        proposals.push((ItemId::Class { class: *id }, Attribute::ClassName, name.to_string(), tie.map(<[String]>::to_vec)));
    });

    // Members: names unique program-wide per kind (prevents shadowing and accidental overrides).
    let mut member_taken: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in p.class_ids() {
        let c = p.class(id);
        for f in &c.fields {
            member_taken.entry("field".into()).or_default().insert(p.str(f.name).to_string());
        }
        for m in &c.methods {
            member_taken.entry("method".into()).or_default().insert(p.str(m.name).to_string());
        }
    }
    let mut field_cands: Vec<(String, String, (ClassId, u32))> = Vec::new();
    for id in p.class_ids() {
        for (i, f) in p.class(id).fields.iter().enumerate() {
            if cx.s_member(ItemId::Field { class: id, index: i as u32 })
                || is_platform_class(p.descriptor(id))
                || cx.recovered_member(ItemId::Field { class: id, index: i as u32 })
                || is_structural_name(p.str(f.name))
                || pins.field(p.descriptor(id), p.str(f.name))
            {
                continue;
            }
            let ty = p.str(f.ty);
            let base = ty.trim_start_matches('[');
            let arr = if base.len() < ty.len() { "Arr" } else { "" };
            let word = match base {
                "Z" => "bool".into(),
                "B" => "byte".into(),
                "S" => "short".into(),
                "C" => "char".into(),
                "I" => "int".into(),
                "J" => "long".into(),
                "F" => "float".into(),
                "D" => "double".into(),
                "Ljava/lang/String;" => "str".into(),
                d if cx.stable_type(d) => sanitize(simple_name_of(d).rsplit('$').next().unwrap_or("")).map(|s| lower_first(&s)).unwrap_or("obj".into()),
                _ => "obj".into(),
            };
            let h = member_hash(id, &format!("{}\nused {}", field_text(id, i), usage(field_uses.get(&(id, i as u32)))));
            field_cands.push((h, format!("{word}{arr}"), (id, i as u32)));
        }
    }
    field_cands.sort();
    assign(&mut field_cands, |_| "field".to_string(), &mut member_taken, findings, "field", |&(id, i), name, tie| {
        let c = p.class(id);
        let f = &c.fields[i as usize];
        renaming.fields.insert((p.descriptor(id).to_string(), p.str(f.name).to_string(), p.str(f.ty).to_string()), name.to_string());
        proposals.push((ItemId::Field { class: id, index: i }, Attribute::MemberName, name.to_string(), tie.map(<[String]>::to_vec)));
    });

    // Methods: static/private individually; virtual by override group, if safe.
    let object_method = |m: &eightr_ir::model::Method| OBJECT_METHODS.iter().any(|&(n, d)| p.str(m.name) == n && p.str(m.proto) == d);
    // (group hash, hint, members of the override group)
    type MethodCand = (String, String, Vec<(ClassId, u32)>);
    let mut method_cands: Vec<MethodCand> = Vec::new();
    let mut done_groups = BTreeSet::new();
    for id in p.class_ids() {
        for (i, m) in p.class(id).methods.iter().enumerate() {
            let item = ItemId::Method { class: id, index: i as u32 };
            if m.access & access::CONSTRUCTOR != 0
                || cx.s_member(item)
                || cx.recovered_member(item)
                || is_platform_class(p.descriptor(id))
                || is_structural_name(p.str(m.name))
            {
                continue;
            }
            let members = if is_virtual(m) {
                let root = groups.find((id, i as u32));
                if !done_groups.insert(root) {
                    continue;
                }
                members_of[&root].clone()
            } else {
                vec![(id, i as u32)]
            };
            // A virtual method may be renamed only if no member of its override group could
            // override a library method: every library supertype is a known platform class,
            // and no platform method with the same (short) name and descriptor exists.
            let plat = platform();
            let safe = members.iter().all(|&(cid, mi)| {
                let mm = &p.class(cid).methods[mi as usize];
                let (name, proto) = (p.str(mm.name), p.str(mm.proto));
                !cx.s_member(ItemId::Method { class: cid, index: mi })
                    && !pins.method(p.descriptor(cid), name)
                    && (!is_virtual(mm)
                        || (!object_method(mm)
                            && name.len() <= PLATFORM_NAME_LIMIT
                            && lib_supers[&cid].iter().all(|l| plat.classes.contains(l.as_str()))
                            && !plat.methods.contains(&(name, proto))))
            });
            if !safe {
                kept_unsafe += members.len();
                continue;
            }
            let mut hs: Vec<String> = members
                .iter()
                .map(|&(cid, mi)| hash_hex(&format!("{}\nused {}", method_hash[&(cid, mi)], usage(method_uses.get(&(cid, mi))))))
                .collect();
            hs.sort();
            method_cands.push((hash_hex(&hs.join("\n")), "m".to_string(), members));
        }
    }
    method_cands.sort();
    assign(&mut method_cands, |_| "method".to_string(), &mut member_taken, findings, "method", |members, name, tie| {
        for &(id, i) in members {
            let m = &p.class(id).methods[i as usize];
            renaming.methods.insert((p.descriptor(id).to_string(), p.str(m.name).to_string(), p.str(m.proto).to_string()), name.to_string());
            proposals.push((ItemId::Method { class: id, index: i }, Attribute::MemberName, name.to_string(), tie.map(<[String]>::to_vec)));
        }
    });

    for (item, attr, value, tie) in proposals {
        labels.record_value(item, attr, STRUCTURAL_NAME, None, Some(value))?;
        if let Some(candidates) = tie {
            labels.record(item, attr, STRUCTURAL_TIE, Some(candidates))?;
        }
    }
    if kept_unsafe > 0 {
        findings.push(Finding {
            severity: Severity::Info,
            message: format!(
                "{STRUCTURAL_NAME}: {kept_unsafe} virtual method(s) keep their names: they may override library methods this app doesn't reference"
            ),
        });
    }
    Ok(Naming { renaming, kept_unsafe })
}

/// Assigns `{hint}_{hash prefix}` names in candidate order (sorted by hash, then hint), using
/// the shortest prefix (≥ MIN_HEX) not already taken in the candidate's namespace. Identical
/// (hash, hint) pairs are structurally indistinguishable: they get numeric suffixes in input
/// order, which is reported, since only truly automorphic ties are α-invariant.
fn assign<T>(
    cands: &mut [(String, String, T)],
    namespace: impl Fn(&T) -> String,
    taken: &mut BTreeMap<String, BTreeSet<String>>,
    findings: &mut Vec<Finding>,
    what: &str,
    mut apply: impl FnMut(&T, &str, Option<&[String]>),
) {
    let mut k = 0;
    while k < cands.len() {
        // A run of identical (hash, hint) candidates.
        let mut end = k + 1;
        while end < cands.len() && cands[end].0 == cands[k].0 && cands[end].1 == cands[k].1 {
            end += 1;
        }
        if end - k > 1 {
            findings.push(Finding {
                severity: Severity::Info,
                message: format!("{STRUCTURAL_TIE}: {} {what}s are structurally indistinguishable (hash {}); reported as N", end - k, &cands[k].0[..MIN_HEX]),
            });
        }
        let mut names = Vec::with_capacity(end - k);
        for (n, c) in cands[k..end].iter().enumerate() {
            let ns = namespace(&c.2);
            let set = taken.entry(ns).or_default();
            let suffix = if end - k > 1 { format!("_{}", n + 1) } else { String::new() };
            let mut len = MIN_HEX;
            let name = loop {
                let candidate = format!("{}_{}{suffix}", c.1, &c.0[..len.min(c.0.len())]);
                if !set.contains(&candidate) || len >= c.0.len() {
                    break candidate;
                }
                len += 1;
            };
            set.insert(name.clone());
            names.push(name);
        }
        let tie = (end - k > 1).then_some(names.as_slice());
        for (c, name) in cands[k..end].iter().zip(&names) {
            apply(&c.2, name, tie);
        }
        k = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_hint() {
        assert_eq!(sanitize("ExternalSyntheticLambda0").as_deref(), Some("ExternalSyntheticLambda0"));
        assert_eq!(sanitize("1abc").as_deref(), Some("abc"));
        assert_eq!(sanitize("$$"), None);
        assert_eq!(lower_first("Activity"), "activity");
    }

    #[test]
    fn structural_names() {
        for n in ["Class_9bb9", "m_1a2b3c", "Interface_e2cb_24", "int_fab9"] {
            assert!(is_structural_name(n), "{n}");
        }
        for n in ["Class", "a", "Class_9bb", "Class_9bbz", "_9bb9", "Class_9bb9_", "Class_9bb9_x", "db"] {
            assert!(!is_structural_name(n), "{n}");
        }
    }
}
