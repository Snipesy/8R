//! Inlining hints (Phase 4; D·id, report only). R8 inlines methods without leaving a general
//! way back (DESIGN.md §1): 8R doesn't un-inline, but it reports where inlining evidently
//! happened, each kind measured against the fixture mappings' inline frames
//! (`tests/oracle.rs`, `inlining_hints_precision_recall`) before it's trusted:
//! * `discarded-getclass`: `x.getClass()` whose result is unused — R8's null check of the
//!   receiver of an inlined instance call (or `Objects.requireNonNull` / Kotlin's null checks,
//!   which R8 rewrites to the same thing: ambiguous, so a hint);
//! * `inlined-instance-call`: a discarded `getClass()` whose receiver the next instructions use
//!   (a field or a call on it): the inlined callee's body;
//! * `idiom:areEqual`: Kotlin's `Intrinsics.areEqual(a, b)` (`a == b`), inlined as an `equals`;
//! * `idiom:collectionSizeOrDefault`: `if (x instanceof Collection) ((Collection) x).size()`.
//!
//! Also counted: methods whose line table is compacted (lines 1..n, one per position change),
//! where no original line survives.

use std::collections::BTreeMap;

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{InvokeKind, Op};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct InlineHint {
    /// `Lclass;->name(proto)` in input names.
    pub method: String,
    /// Instruction index in the input body (lines are looked up from it).
    pub insn: u32,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InliningSummary {
    /// Hints by kind.
    pub hints: BTreeMap<&'static str, u64>,
    /// Methods with a compacted line table (1..n): original lines only in the mapping.
    pub compacted_line_tables: u64,
    /// Methods whose lines are their pcs + 1 (R8's pc encoding): same.
    pub pc_encoded_line_tables: u64,
    /// Methods with a line table at all.
    pub line_tables: u64,
}

pub const DISCARDED_GETCLASS: &str = "discarded-getclass";
pub const INLINED_INSTANCE_CALL: &str = "inlined-instance-call";
pub const ARE_EQUAL: &str = "idiom:areEqual";
pub const COLLECTION_SIZE_OR_DEFAULT: &str = "idiom:collectionSizeOrDefault";

/// Hints in one body.
pub fn body_hints(p: &Model, body: &Body) -> Vec<(u32, &'static str)> {
    let s = &p.syms;
    let mut out = Vec::new();
    let n = body.insns.len();
    for (i, insn) in body.insns.iter().enumerate() {
        match &insn.op {
            Op::Invoke { kind: InvokeKind::Virtual, method, args } if s.get(method.name) == "getClass" && s.get(method.proto) == "()Ljava/lang/Class;" => {
                let used = matches!(body.insns.get(i + 1).map(|x| &x.op), Some(Op::MoveResult { .. }));
                if !used && args.len() == 1 {
                    out.push((i as u32, DISCARDED_GETCLASS));
                    // The receiver then used by the inlined body (a field or a call on it).
                    let x = args[0];
                    let body_follows = body.insns[i + 1..(i + 5).min(n)].iter().any(|y| match &y.op {
                        Op::InstanceGet { obj, .. } | Op::InstancePut { obj, .. } => *obj == x,
                        Op::Invoke { kind, args, .. } => *kind != InvokeKind::Static && args.first() == Some(&x),
                        _ => false,
                    });
                    if body_follows {
                        out.push((i as u32, INLINED_INSTANCE_CALL));
                    }
                }
            }
            // areEqual(a, b) = a == null ? b == null : a.equals(b). Kotlin compiles `a == b`
            // to it; R8 usually proves `a` non-null and keeps only `a.equals(b)`.
            // `areEqual` takes Objects: the inlined call references `Object.equals` (a
            // hand-written `x.equals(y)` usually names x's class).
            Op::Invoke { kind: InvokeKind::Virtual, method, args }
                if s.get(method.name) == "equals" && s.get(method.proto) == "(Ljava/lang/Object;)Z" && s.get(method.class) == "Ljava/lang/Object;" && args.len() == 2 =>
            {
                out.push((i as u32, ARE_EQUAL));
            }
            // The full form only: when R8 knows the value is a Collection it keeps a bare
            // `size()`, indistinguishable from any other.
            Op::InstanceOf { ty, .. } if s.get(*ty) == "Ljava/util/Collection;" => {
                // ... then a size() (on the cast copy) shortly after.
                let sized = body.insns[i + 1..(i + 8).min(n)].iter().any(|x| {
                    matches!(&x.op, Op::Invoke { method, .. } if s.get(method.name) == "size" && s.get(method.proto) == "()I")
                });
                if sized {
                    out.push((i as u32, COLLECTION_SIZE_OR_DEFAULT));
                }
            }
            _ => {}
        }
    }
    out
}

/// A compacted line table: lines are exactly 1..n in order.
pub fn is_compacted(body: &Body) -> bool {
    !body.positions.is_empty() && body.positions.iter().enumerate().all(|(k, (_, line))| *line == k as i64 + 1)
}

/// Lines are the instructions' pcs + 1.
pub fn is_pc_encoded(body: &Body) -> bool {
    !body.positions.is_empty() && body.positions.iter().all(|(i, line)| body.insns.get(*i as usize).is_some_and(|x| i64::from(x.pc) + 1 == *line))
}

/// The line of instruction `i` (the last position at or before it).
pub fn line_of(body: &Body, i: u32) -> Option<u32> {
    body.positions.iter().rev().find(|(k, _)| *k <= i).and_then(|(_, l)| u32::try_from(*l).ok())
}

pub fn collect(p: &Model) -> (Vec<InlineHint>, InliningSummary) {
    let mut hints = Vec::new();
    let mut summary = InliningSummary::default();
    for c in &p.classes {
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            if !b.positions.is_empty() {
                summary.line_tables += 1;
                summary.compacted_line_tables += u64::from(is_compacted(b));
                summary.pc_encoded_line_tables += u64::from(is_pc_encoded(b));
            }
            for (insn, kind) in body_hints(p, b) {
                *summary.hints.entry(kind).or_default() += 1;
                hints.push(InlineHint { method: format!("{}->{}{}", p.syms.get(c.ty), p.syms.get(m.name), p.syms.get(m.proto)), insn, kind });
            }
        }
    }
    hints.sort();
    (hints, summary)
}

/// The build-time annotation 8R puts on methods with hints (`@eightr.Inlined(areEqual = 2,
/// nullChecks = 1, ...)`), so decompilers show them. Build visibility: never loaded at runtime.
pub const ANNOTATION: &str = "Leightr/Inlined;";

/// Removes 8R's own hint annotations (re-running 8R on its output must not see them).
pub fn strip(p: &mut Model) {
    let Some(ty) = p.syms.lookup(ANNOTATION) else { return };
    for m in p.classes.iter_mut().flat_map(|c| c.methods.iter_mut()) {
        m.annotations.retain(|a| a.annotation.ty != ty);
    }
}

/// Annotates every method of `p` that has hints with their counts by kind.
pub fn annotate(p: &mut Model) {
    use eightr_ir::value::{Annotation, EncodedAnnotation, Value, Visibility};
    fn element(kind: &'static str) -> &'static str {
        match kind {
            DISCARDED_GETCLASS => "nullChecks",
            INLINED_INSTANCE_CALL => "inlinedInstanceCalls",
            ARE_EQUAL => "areEqual",
            COLLECTION_SIZE_OR_DEFAULT => "collectionSizeOrDefault",
            other => other,
        }
    }
    let mut plan: Vec<(usize, usize, BTreeMap<&'static str, i32>)> = Vec::new();
    for (ci, c) in p.classes.iter().enumerate() {
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(b) = &m.code else { continue };
            let mut counts: BTreeMap<&'static str, i32> = BTreeMap::new();
            for (_, kind) in body_hints(p, b) {
                *counts.entry(element(kind)).or_default() += 1;
            }
            if !counts.is_empty() {
                plan.push((ci, mi, counts));
            }
        }
    }
    let ty = p.syms.intern(ANNOTATION);
    for (ci, mi, counts) in plan {
        let elements = counts.into_iter().map(|(k, n)| (p.syms.intern(k), Value::Int(n))).collect();
        let m = &mut p.classes[ci].methods[mi];
        m.annotations.retain(|a| a.annotation.ty != ty);
        m.annotations.push(Annotation { visibility: Visibility::Build, annotation: EncodedAnnotation { ty, elements } });
    }
}
