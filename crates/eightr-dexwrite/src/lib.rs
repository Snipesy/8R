//! Canonical DEX writer.
//!
//! [`write`] lays out a dex file for a set of classes from an [`eightr_ir::model::Program`].
//! The output is a pure function of the program's content: pools are sorted as the format
//! requires, handles/call sites/class order are canonical, and data items are emitted in a
//! fixed section order. Writing the same program twice yields identical bytes.

mod encode;
mod pool;

use std::collections::BTreeMap;
use std::fmt;

use eightr_dex::item_type as it;
use eightr_ir::model::{Class, Program};
use eightr_ir::op::{FieldRef, MethodRef, Op};
use eightr_ir::types::parse_proto;
use eightr_ir::value::{default_for, Annotation, Value};

use pool::Pools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError(pub String);

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dex write: {}", self.0)
    }
}

impl std::error::Error for WriteError {}

const NO_INDEX: u32 = 0xffff_ffff;

/// A growing byte buffer with alignment and little-endian helpers.
#[derive(Default)]
struct Buf(Vec<u8>);

impl Buf {
    fn pos(&self) -> u32 {
        self.0.len() as u32
    }
    fn align(&mut self, to: usize) {
        while !self.0.len().is_multiple_of(to) {
            self.0.push(0);
        }
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn set_u32(&mut self, at: u32, v: u32) {
        self.0[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// Minimum dex version needed for the features used.
fn required_version(p: &Program, classes: &[usize]) -> u32 {
    let mut v = 35;
    for &i in classes {
        for m in &p.classes[i].methods {
            for insn in m.code.iter().flat_map(|b| &b.insns) {
                match insn.op {
                    Op::ConstMethodHandle { .. } | Op::ConstMethodType { .. } => v = v.max(39),
                    Op::InvokePolymorphic { .. } | Op::InvokeCustom { .. } => v = v.max(38),
                    _ => {}
                }
            }
        }
    }
    v
}

/// Class definition order: supertypes defined in this file come first; otherwise by type index.
fn class_order(p: &Program, classes: &[usize], pools: &Pools) -> Vec<usize> {
    let s = &p.syms;
    let by_ty: BTreeMap<u32, usize> = classes.iter().map(|&i| (pools.ty(s.get(p.classes[i].ty)), i)).collect();
    let mut done = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(classes.len());
    fn visit(
        t: u32,
        p: &Program,
        pools: &Pools,
        by_ty: &BTreeMap<u32, usize>,
        done: &mut std::collections::BTreeSet<u32>,
        out: &mut Vec<usize>,
    ) {
        if !done.insert(t) {
            return;
        }
        let Some(&i) = by_ty.get(&t) else { return };
        let c = &p.classes[i];
        let s = &p.syms;
        for sup in c.superclass.iter().chain(&c.interfaces) {
            visit(pools.ty(s.get(*sup)), p, pools, by_ty, done, out);
        }
        out.push(i);
    }
    for &t in by_ty.keys() {
        visit(t, p, pools, &by_ty, &mut done, &mut out);
    }
    out
}

struct Section {
    ty: u16,
    count: u32,
    off: u32,
}

/// Writes one dex file containing `classes` (indices into `p.classes`).
pub fn write(p: &Program, classes: &[usize]) -> Result<Vec<u8>, WriteError> {
    let s = &p.syms;
    let pools = Pools::build(p, classes)?;
    let order = class_order(p, classes, &pools);
    let version = required_version(p, classes);

    let n_str = pools.strings.len() as u32;
    let n_ty = pools.types.len() as u32;
    let n_proto = pools.protos.len() as u32;
    let n_field = pools.fields.len() as u32;
    let n_method = pools.methods.len() as u32;
    let n_class = order.len() as u32;
    let n_cs = pools.call_sites.len() as u32;
    let n_mh = pools.handles.len() as u32;

    // Fixed-size id sections, right after the header.
    let mut off = 0x70u32;
    let mut place = |count: u32, width: u32| {
        let o = if count == 0 { 0 } else { off };
        off += count * width;
        o
    };
    let string_ids_off = place(n_str, 4);
    let type_ids_off = place(n_ty, 4);
    let proto_ids_off = place(n_proto, 12);
    let field_ids_off = place(n_field, 8);
    let method_ids_off = place(n_method, 8);
    let class_defs_off = place(n_class, 32);
    let call_site_ids_off = place(n_cs, 4);
    let method_handles_off = place(n_mh, 8);
    let data_off = off;

    let mut b = Buf(vec![0; data_off as usize]);
    let mut sections: Vec<Section> = Vec::new();
    let section = |_: &Buf, ty: u16, start: u32, count: u32, sections: &mut Vec<Section>| {
        if count > 0 {
            sections.push(Section { ty, count, off: start });
        }
    };
    // Members in canonical (pool index) order: model order reflects the input file, which
    // must not influence the output layout.
    let methods_sorted = |ci: usize| -> Vec<usize> {
        let c = &p.classes[ci];
        let mut v: Vec<(u32, usize)> = c
            .methods
            .iter()
            .enumerate()
            .map(|(mi, m)| (pools.method(&MethodRef { class: c.ty, name: m.name, proto: m.proto }, s), mi))
            .collect();
        v.sort();
        v.into_iter().map(|(_, mi)| mi).collect()
    };
    let fields_sorted = |ci: usize| -> Vec<usize> {
        let c = &p.classes[ci];
        let mut v: Vec<(u32, usize)> = c
            .fields
            .iter()
            .enumerate()
            .map(|(fi, f)| (pools.field(&FieldRef { class: c.ty, name: f.name, ty: f.ty }, s), fi))
            .collect();
        v.sort();
        v.into_iter().map(|(_, fi)| fi).collect()
    };
    // (class, method) in layout order.
    let method_order: Vec<(usize, usize)> =
        order.iter().flat_map(|&ci| methods_sorted(ci).into_iter().map(move |mi| (ci, mi))).collect();

    // string_data_item
    let start = b.pos();
    let mut string_data_offs = Vec::with_capacity(pools.strings.len());
    for st in &pools.strings {
        string_data_offs.push(b.pos());
        let mut v = Vec::new();
        encode::uleb(&mut v, st.encode_utf16().count() as u32);
        b.bytes(&v);
        b.bytes(&eightr_dex::mutf8::encode(st));
        b.0.push(0);
    }
    section(&b, it::STRING_DATA_ITEM, start, n_str, &mut sections);

    // type_list (deduplicated): proto parameters and class interfaces.
    b.align(4);
    let start = b.pos();
    let mut type_lists: BTreeMap<Vec<u32>, u32> = BTreeMap::new();
    let mut lists_in_order: Vec<Vec<u32>> = Vec::new();
    for (d, _) in &pools.protos {
        let (params, _) = parse_proto(d).expect("validated");
        let l: Vec<u32> = params.iter().map(|t| pools.ty(t)).collect();
        if !l.is_empty() && !lists_in_order.contains(&l) {
            lists_in_order.push(l);
        }
    }
    for &i in &order {
        let l: Vec<u32> = p.classes[i].interfaces.iter().map(|t| pools.ty(s.get(*t))).collect();
        if !l.is_empty() && !lists_in_order.contains(&l) {
            lists_in_order.push(l);
        }
    }
    for l in &lists_in_order {
        b.align(4);
        type_lists.insert(l.clone(), b.pos());
        b.u32(l.len() as u32);
        for &t in l {
            b.u16(u16::try_from(t).map_err(|_| WriteError("type index exceeds 16 bits".into()))?);
        }
    }
    section(&b, it::TYPE_LIST, start, lists_in_order.len() as u32, &mut sections);

    // Per-method encoded code, computed once (debug info and code items both need it).
    let mut encoded: BTreeMap<(usize, usize), encode::EncodedCode> = BTreeMap::new();
    for &(ci, mi) in &method_order {
        {
            let m = &p.classes[ci].methods[mi];
            if let Some(body) = &m.code {
                let e = encode::encode_body(body, &pools, s)
                    .map_err(|e| WriteError(format!("{}->{}{}: {}", s.get(p.classes[ci].ty), s.get(m.name), s.get(m.proto), e.0)))?;
                encoded.insert((ci, mi), e);
            }
        }
    }

    // debug_info_item
    let start = b.pos();
    let mut debug_offs: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for (ci, mi, e) in method_order.iter().filter_map(|&(ci, mi)| encoded.get(&(ci, mi)).map(|e| (ci, mi, e))) {
        let body = p.classes[ci].methods[mi].code.as_ref().expect("encoded");
        if let Some(d) = encode::debug_info(body, &e.pcs, &pools, s) {
            debug_offs.insert((ci, mi), b.pos());
            b.bytes(&d);
        }
    }
    section(&b, it::DEBUG_INFO_ITEM, start, debug_offs.len() as u32, &mut sections);

    // annotation_item (deduplicated), then annotation_set_item, annotation_set_ref_list.
    let annotation_bytes = |a: &Annotation| -> Vec<u8> {
        let mut v = vec![match a.visibility {
            eightr_ir::value::Visibility::Build => 0,
            eightr_ir::value::Visibility::Runtime => 1,
            eightr_ir::value::Visibility::System => 2,
        }];
        encode::encoded_annotation(&mut v, &a.annotation, &pools, s);
        v
    };
    let mut all_sets: Vec<&[Annotation]> = Vec::new();
    let mut ref_lists: Vec<&Vec<Vec<Annotation>>> = Vec::new();
    for &ci in &order {
        let c = &p.classes[ci];
        all_sets.push(&c.annotations);
        for fi in fields_sorted(ci) {
            all_sets.push(&c.fields[fi].annotations);
        }
        for mi in methods_sorted(ci) {
            let m = &c.methods[mi];
            all_sets.push(&m.annotations);
            if let Some(ps) = &m.parameter_annotations {
                ref_lists.push(ps);
                for set in ps {
                    all_sets.push(set);
                }
            }
        }
    }
    let start = b.pos();
    let mut item_offs: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    let mut item_count = 0;
    for set in &all_sets {
        for a in set.iter() {
            let bytes = annotation_bytes(a);
            if !item_offs.contains_key(&bytes) {
                item_offs.insert(bytes.clone(), b.pos());
                b.bytes(&bytes);
                item_count += 1;
            }
        }
    }
    section(&b, it::ANNOTATION_ITEM, start, item_count, &mut sections);

    b.align(4);
    let start = b.pos();
    let mut set_offs: BTreeMap<Vec<u32>, u32> = BTreeMap::new();
    let set_key = |set: &[Annotation]| -> Vec<u32> {
        // Entries sorted by annotation type index.
        let mut v: Vec<(u32, u32)> = set
            .iter()
            .map(|a| (pools.ty(s.get(a.annotation.ty)), item_offs[&annotation_bytes(a)]))
            .collect();
        v.sort();
        v.into_iter().map(|(_, o)| o).collect()
    };
    let mut set_count = 0;
    for set in &all_sets {
        if set.is_empty() {
            continue;
        }
        let key = set_key(set);
        if !set_offs.contains_key(&key) {
            b.align(4);
            set_offs.insert(key.clone(), b.pos());
            b.u32(key.len() as u32);
            for o in &key {
                b.u32(*o);
            }
            set_count += 1;
        }
    }
    section(&b, it::ANNOTATION_SET_ITEM, start, set_count, &mut sections);
    let set_off = |set: &[Annotation]| if set.is_empty() { 0 } else { set_offs[&set_key(set)] };

    b.align(4);
    let start = b.pos();
    let mut ref_list_offs: BTreeMap<Vec<u32>, u32> = BTreeMap::new();
    for ps in &ref_lists {
        let key: Vec<u32> = ps.iter().map(|set| set_off(set)).collect();
        if !ref_list_offs.contains_key(&key) {
            b.align(4);
            ref_list_offs.insert(key.clone(), b.pos());
            b.u32(key.len() as u32);
            for o in &key {
                b.u32(*o);
            }
        }
    }
    section(&b, it::ANNOTATION_SET_REF_LIST, start, ref_list_offs.len() as u32, &mut sections);

    // annotations_directory_item per class that has any annotations.
    b.align(4);
    let start = b.pos();
    let mut dir_offs: BTreeMap<usize, u32> = BTreeMap::new();
    for &ci in &order {
        let c = &p.classes[ci];
        let mut fields: Vec<(u32, u32)> = c
            .fields
            .iter()
            .filter(|f| !f.annotations.is_empty())
            .map(|f| (pools.field(&FieldRef { class: c.ty, name: f.name, ty: f.ty }, s), set_off(&f.annotations)))
            .collect();
        let mut methods: Vec<(u32, u32)> = c
            .methods
            .iter()
            .filter(|m| !m.annotations.is_empty())
            .map(|m| (pools.method(&MethodRef { class: c.ty, name: m.name, proto: m.proto }, s), set_off(&m.annotations)))
            .collect();
        let mut params: Vec<(u32, u32)> = c
            .methods
            .iter()
            .filter_map(|m| {
                let ps = m.parameter_annotations.as_ref()?;
                let key: Vec<u32> = ps.iter().map(|set| set_off(set)).collect();
                Some((pools.method(&MethodRef { class: c.ty, name: m.name, proto: m.proto }, s), ref_list_offs[&key]))
            })
            .collect();
        if c.annotations.is_empty() && fields.is_empty() && methods.is_empty() && params.is_empty() {
            continue;
        }
        fields.sort();
        methods.sort();
        params.sort();
        b.align(4);
        dir_offs.insert(ci, b.pos());
        b.u32(set_off(&c.annotations));
        b.u32(fields.len() as u32);
        b.u32(methods.len() as u32);
        b.u32(params.len() as u32);
        for (i, o) in fields.iter().chain(&methods).chain(&params) {
            b.u32(*i);
            b.u32(*o);
        }
    }
    section(&b, it::ANNOTATIONS_DIRECTORY_ITEM, start, dir_offs.len() as u32, &mut sections);

    // encoded_array_item: call sites (in call-site order, so offsets are sorted), then static values.
    let start = b.pos();
    let mut call_site_offs = Vec::with_capacity(pools.call_sites.len());
    let mut arrays = 0;
    for cs in &pools.call_sites {
        call_site_offs.push(b.pos());
        let mut vals = vec![Value::MethodHandle(cs.bootstrap), Value::String(cs.name), Value::MethodType(cs.proto)];
        vals.extend(cs.extra.iter().cloned());
        let mut v = Vec::new();
        encode::array(&mut v, &vals, &pools, s);
        b.bytes(&v);
        arrays += 1;
    }
    let mut static_offs: BTreeMap<usize, u32> = BTreeMap::new();
    let mut static_dedup: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    for &ci in &order {
        let c = &p.classes[ci];
        let mut statics: Vec<(u32, &eightr_ir::model::Field)> = c
            .fields
            .iter()
            .filter(|f| f.access & eightr_dex::class::access::STATIC != 0)
            .map(|f| (pools.field(&FieldRef { class: c.ty, name: f.name, ty: f.ty }, s), f))
            .collect();
        statics.sort_by_key(|(i, _)| *i);
        let Some(last) = statics.iter().rposition(|(_, f)| f.static_value.is_some()) else { continue };
        let vals: Vec<Value> = statics[..=last]
            .iter()
            .map(|(_, f)| f.static_value.clone().unwrap_or_else(|| default_for(s.get(f.ty))))
            .collect();
        let mut v = Vec::new();
        encode::array(&mut v, &vals, &pools, s);
        let o = match static_dedup.get(&v) {
            Some(&o) => o,
            None => {
                let o = b.pos();
                b.bytes(&v);
                static_dedup.insert(v, o);
                arrays += 1;
                o
            }
        };
        static_offs.insert(ci, o);
    }
    section(&b, it::ENCODED_ARRAY_ITEM, start, arrays, &mut sections);

    // code_item
    b.align(4);
    let start = b.pos();
    let mut code_offs: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for (ci, mi, e) in method_order.iter().filter_map(|&(ci, mi)| encoded.get(&(ci, mi)).map(|e| (ci, mi, e))) {
        let body = p.classes[ci].methods[mi].code.as_ref().expect("encoded");
        b.align(4);
        code_offs.insert((ci, mi), b.pos());
        b.u16(body.registers);
        b.u16(body.ins);
        b.u16(body.outs);
        b.u16(body.tries.len() as u16);
        b.u32(debug_offs.get(&(ci, mi)).copied().unwrap_or(0));
        b.u32(e.insns.len() as u32);
        for &u in &e.insns {
            b.u16(u);
        }
        if !body.tries.is_empty() {
            if e.insns.len() % 2 == 1 {
                b.u16(0);
            }
            // Handler lists, deduplicated, encoded first to know their offsets.
            let mut handler_bytes: Vec<Vec<u8>> = Vec::new();
            let mut handler_of_try = Vec::new();
            for t in &body.tries {
                let mut h = Vec::new();
                let typed: Vec<_> = t.handlers.iter().filter(|h| h.ty.is_some()).collect();
                let all = t.handlers.iter().find(|h| h.ty.is_none());
                let n = typed.len() as i32;
                encode::sleb(&mut h, if all.is_some() { -n } else { n });
                for x in &typed {
                    encode::uleb(&mut h, pools.ty(s.get(x.ty.expect("typed"))));
                    encode::uleb(&mut h, e.pcs[x.target as usize]);
                }
                if let Some(a) = all {
                    encode::uleb(&mut h, e.pcs[a.target as usize]);
                }
                let idx = match handler_bytes.iter().position(|x| *x == h) {
                    Some(i) => i,
                    None => {
                        handler_bytes.push(h);
                        handler_bytes.len() - 1
                    }
                };
                handler_of_try.push(idx);
            }
            let mut list = Vec::new();
            encode::uleb(&mut list, handler_bytes.len() as u32);
            let mut rel = Vec::new();
            for h in &handler_bytes {
                rel.push(list.len() as u16);
                list.extend_from_slice(h);
            }
            for (t, &hi) in body.tries.iter().zip(&handler_of_try) {
                let start_pc = e.pcs[t.start as usize];
                let end_pc = e.pcs[t.end as usize];
                b.u32(start_pc);
                b.u16((end_pc - start_pc) as u16);
                b.u16(rel[hi]);
            }
            b.bytes(&list);
        }
    }
    section(&b, it::CODE_ITEM, start, code_offs.len() as u32, &mut sections);

    // class_data_item
    let start = b.pos();
    let mut class_data_offs: BTreeMap<usize, u32> = BTreeMap::new();
    for &ci in &order {
        let c = &p.classes[ci];
        if c.fields.is_empty() && c.methods.is_empty() {
            continue;
        }
        let fidx = |f: &eightr_ir::model::Field| pools.field(&FieldRef { class: c.ty, name: f.name, ty: f.ty }, s);
        let midx = |m: &eightr_ir::model::Method| pools.method(&MethodRef { class: c.ty, name: m.name, proto: m.proto }, s);
        let is_static = |f: &&eightr_ir::model::Field| f.access & eightr_dex::class::access::STATIC != 0;
        let mut sf: Vec<(u32, u32)> = c.fields.iter().filter(is_static).map(|f| (fidx(f), f.access)).collect();
        let mut inf: Vec<(u32, u32)> = c.fields.iter().filter(|f| !is_static(f)).map(|f| (fidx(f), f.access)).collect();
        let mut dm: Vec<(u32, u32, u32)> = Vec::new();
        let mut vm: Vec<(u32, u32, u32)> = Vec::new();
        for (mi, m) in c.methods.iter().enumerate() {
            let e = (midx(m), m.access, code_offs.get(&(ci, mi)).copied().unwrap_or(0));
            if m.is_direct() { dm.push(e) } else { vm.push(e) }
        }
        sf.sort();
        inf.sort();
        dm.sort();
        vm.sort();
        let mut v = Vec::new();
        for n in [sf.len(), inf.len(), dm.len(), vm.len()] {
            encode::uleb(&mut v, n as u32);
        }
        for list in [&sf, &inf] {
            let mut prev = 0;
            for &(i, a) in list.iter() {
                encode::uleb(&mut v, i - prev);
                encode::uleb(&mut v, a);
                prev = i;
            }
        }
        for list in [&dm, &vm] {
            let mut prev = 0;
            for &(i, a, co) in list.iter() {
                encode::uleb(&mut v, i - prev);
                encode::uleb(&mut v, a);
                encode::uleb(&mut v, co);
                prev = i;
            }
        }
        class_data_offs.insert(ci, b.pos());
        b.bytes(&v);
    }
    section(&b, it::CLASS_DATA_ITEM, start, class_data_offs.len() as u32, &mut sections);

    // map_list
    b.align(4);
    let map_off = b.pos();
    let mut map: Vec<Section> = vec![Section { ty: it::HEADER_ITEM, count: 1, off: 0 }];
    for (ty, count, o) in [
        (it::STRING_ID_ITEM, n_str, string_ids_off),
        (it::TYPE_ID_ITEM, n_ty, type_ids_off),
        (it::PROTO_ID_ITEM, n_proto, proto_ids_off),
        (it::FIELD_ID_ITEM, n_field, field_ids_off),
        (it::METHOD_ID_ITEM, n_method, method_ids_off),
        (it::CLASS_DEF_ITEM, n_class, class_defs_off),
        (it::CALL_SITE_ID_ITEM, n_cs, call_site_ids_off),
        (it::METHOD_HANDLE_ITEM, n_mh, method_handles_off),
    ] {
        if count > 0 {
            map.push(Section { ty, count, off: o });
        }
    }
    map.extend(sections);
    map.push(Section { ty: it::MAP_LIST, count: 1, off: map_off });
    map.sort_by_key(|m| m.off);
    b.u32(map.len() as u32);
    for m in &map {
        b.u16(m.ty);
        b.u16(0);
        b.u32(m.count);
        b.u32(m.off);
    }
    b.align(4);
    let file_size = b.pos();

    // Id sections.
    for (i, &o) in string_data_offs.iter().enumerate() {
        b.set_u32(string_ids_off + 4 * i as u32, o);
    }
    for (i, t) in pools.types.iter().enumerate() {
        b.set_u32(type_ids_off + 4 * i as u32, pools.string(t));
    }
    for (i, (d, key)) in pools.protos.iter().enumerate() {
        let at = proto_ids_off + 12 * i as u32;
        b.set_u32(at, pools.string(&pool::shorty(d)?));
        b.set_u32(at + 4, key.ret);
        b.set_u32(at + 8, if key.params.is_empty() { 0 } else { type_lists[&key.params] });
    }
    let u16_idx = |i: u32, what: &str| u16::try_from(i).map_err(|_| WriteError(format!("{what} index exceeds 16 bits")));
    for (i, (c, n, t)) in pools.fields.iter().enumerate() {
        let at = (field_ids_off + 8 * i as u32) as usize;
        b.0[at..at + 2].copy_from_slice(&u16_idx(pools.ty(c), "type")?.to_le_bytes());
        b.0[at + 2..at + 4].copy_from_slice(&u16_idx(pools.ty(t), "type")?.to_le_bytes());
        b.set_u32(at as u32 + 4, pools.string(n));
    }
    for (i, (c, n, pr)) in pools.methods.iter().enumerate() {
        let at = (method_ids_off + 8 * i as u32) as usize;
        b.0[at..at + 2].copy_from_slice(&u16_idx(pools.ty(c), "type")?.to_le_bytes());
        b.0[at + 2..at + 4].copy_from_slice(&u16_idx(pools.proto(pr), "proto")?.to_le_bytes());
        b.set_u32(at as u32 + 4, pools.string(n));
    }
    for (k, &ci) in order.iter().enumerate() {
        let c: &Class = &p.classes[ci];
        let at = class_defs_off + 32 * k as u32;
        let ifaces: Vec<u32> = c.interfaces.iter().map(|t| pools.ty(s.get(*t))).collect();
        b.set_u32(at, pools.ty(s.get(c.ty)));
        b.set_u32(at + 4, c.access);
        b.set_u32(at + 8, c.superclass.map_or(NO_INDEX, |t| pools.ty(s.get(t))));
        b.set_u32(at + 12, if ifaces.is_empty() { 0 } else { type_lists[&ifaces] });
        b.set_u32(at + 16, c.source_file.map_or(NO_INDEX, |f| pools.string(s.get(f))));
        b.set_u32(at + 20, dir_offs.get(&ci).copied().unwrap_or(0));
        b.set_u32(at + 24, class_data_offs.get(&ci).copied().unwrap_or(0));
        b.set_u32(at + 28, static_offs.get(&ci).copied().unwrap_or(0));
    }
    for (i, &o) in call_site_offs.iter().enumerate() {
        b.set_u32(call_site_ids_off + 4 * i as u32, o);
    }
    for (i, h) in pools.handles.iter().enumerate() {
        let at = (method_handles_off + 8 * i as u32) as usize;
        let member = match &h.member {
            eightr_ir::value::HandleMember::Field(f) => pools.field(f, s),
            eightr_ir::value::HandleMember::Method(m) => pools.method(m, s),
        };
        b.0[at..at + 2].copy_from_slice(&h.kind.code().to_le_bytes());
        b.0[at + 4..at + 6].copy_from_slice(&u16_idx(member, "method handle member")?.to_le_bytes());
    }

    // Header.
    b.0[..8].copy_from_slice(format!("dex\n{version:03}\0").as_bytes());
    b.set_u32(0x20, file_size);
    b.set_u32(0x24, 0x70);
    b.set_u32(0x28, 0x1234_5678);
    b.set_u32(0x34, map_off);
    for (at, count, o) in [
        (0x38, n_str, string_ids_off),
        (0x40, n_ty, type_ids_off),
        (0x48, n_proto, proto_ids_off),
        (0x50, n_field, field_ids_off),
        (0x58, n_method, method_ids_off),
        (0x60, n_class, class_defs_off),
    ] {
        b.set_u32(at, count);
        b.set_u32(at + 4, o);
    }
    b.set_u32(0x68, file_size - data_off);
    b.set_u32(0x6c, data_off);
    use sha1::{Digest, Sha1};
    let sig: [u8; 20] = Sha1::digest(&b.0[32..]).into();
    b.0[12..32].copy_from_slice(&sig);
    let sum = eightr_dex::adler32(&b.0[12..]);
    b.set_u32(8, sum);
    Ok(b.0)
}

/// Writes all classes of `p` into as many dex files as `assign` asks for: `assign(i)` gives
/// the file index for class `i`. Returns files in index order.
pub fn write_split(p: &Program, files: usize, assign: impl Fn(usize) -> usize) -> Result<Vec<Vec<u8>>, WriteError> {
    let mut groups = vec![Vec::new(); files];
    for i in 0..p.classes.len() {
        groups[assign(i)].push(i);
    }
    groups.iter().map(|g| write(p, g)).collect()
}

/// Writes every class into one dex file.
pub fn write_all(p: &Program) -> Result<Vec<u8>, WriteError> {
    let all: Vec<usize> = (0..p.classes.len()).collect();
    write(p, &all)
}
