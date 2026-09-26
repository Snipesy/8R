//! `r8/kept-name`: names that R8's minifier cannot have produced are original.
//!
//! This asserts S, so it must be sound. The predicate follows R8's name generator
//! (`SymbolGenerationUtils.numberToIdentifier`, see docs/sources/r8-desugar.md §1): minified
//! names are `[a-zA-Z][0-9a-zA-Z]*`, with a length bounded by how many names the namespace
//! can have consumed. The test suite grades every S label this pass emits against R8's
//! held-back mapping file, including a fixture that drives the generator into digit names,
//! inner-class names, and repackaging collisions.

use std::collections::{BTreeMap, BTreeSet};

use eightr_rules::{Attribute, Source, KEPT_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{package_of, simple_name_of, ItemId, Program};

pub struct KeptName;

/// Counts that bound how long a generated name can be. Computed from the whole input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NameStats {
    /// Distinct type descriptors referenced anywhere (classes compete for names with every
    /// type R8 reserves: kept, library, missing).
    pub type_descriptors: u64,
    /// Distinct field and method name strings referenced anywhere.
    pub member_names: u64,
    /// Distinct packages among defined classes.
    pub packages: u64,
}

/// Slack for names R8 consumes without them appearing in the dex: `R`, reserved words, and
/// unreferenced classpath types (classes), or library method names in supertypes (members).
const CLASS_SLACK: u64 = 64;
const MEMBER_SLACK: u64 = 4096;

/// Words R8 never generates (`Minifier.RESERVED_NAMES`, the ≤3-character ones present in
/// every version checked). `if` was added in 8.11, so it's deliberately absent here.
const NEVER_GENERATED: &[&str] = &["it", "by", "do"];

/// Substrings of R8/D8 synthetic or fresh names. Such a name is compiler-invented, never
/// the source's own, even when it survives unminified (`-dontobfuscate`, D8 builds).
const SYNTHETIC_MARKERS: &[&str] = &[
    "$$ExternalSynthetic", "$$InternalSynthetic", "-$$Nest$", "$r8$", "$-CC", "-IA", "$EnumUnboxing",
    "$Wrapper", "$VivifiedWrapper",
    // Invented by 8R itself (e.g. re-boxed enums' adapters).
    "$8r$",
];

/// Smallest length whose cumulative lowercase-generator capacity reaches `count`. Lowercase
/// gives the fewest names per length (first char 26 options, then 36), so this is the
/// longest a generated name can be under either casing mode.
pub fn length_bound(count: u64) -> usize {
    let mut len = 1;
    let mut per_len: u64 = 26;
    let mut total: u64 = 26;
    while total < count {
        len += 1;
        per_len = per_len.saturating_mul(36);
        total = total.saturating_add(per_len);
    }
    len
}

/// Could R8's generator produce `name` in a namespace whose names are at most `bound` long?
/// Mixed-case alphabet (the superset of both casing modes).
pub fn may_be_minified(name: &str, bound: usize) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= bound
        && b[0].is_ascii_alphabetic()
        && b[1..].iter().all(u8::is_ascii_alphanumeric)
        && !NEVER_GENERATED.contains(&name)
}

/// Position of `name` in R8's member-name sequence (mixed case: first character from
/// `[a-zA-Z]`, the rest from `[0-9a-zA-Z]`, little-endian; `a` = 1). `None` if R8's generator
/// can't produce it at all.
pub fn generator_index(name: &str) -> Option<u64> {
    const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const REST: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let b = name.as_bytes();
    let head = *b.first()?;
    let first = FIRST.iter().position(|&c| c == head)? as u64;
    let mut n: u64 = 0;
    for &c in b[1..].iter().rev() {
        let d = REST.iter().position(|&x| x == c)? as u64;
        n = n.checked_mul(62)?.checked_add(d + 1)?;
    }
    n.checked_mul(52)?.checked_add(first + 1)
}

/// Slack for field names R8 may have skipped in a class's naming state beyond the fields we
/// can see (short library field names in supertypes, reserved words).
const FIELD_SLACK: u64 = 64;

fn is_synthetic(name: &str) -> bool {
    SYNTHETIC_MARKERS.iter().any(|m| name.contains(m))
}

/// `name$123`: R8's fresh-name pattern (`createFreshMethodNameWithoutHolder`).
fn is_fresh(name: &str) -> bool {
    name.rsplit_once('$').is_some_and(|(head, n)| !head.is_empty() && !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// The part of a simple name R8 generates for inner classes: after the last `$`.
fn tail(simple: &str) -> &str {
    simple.rsplit('$').next().unwrap_or(simple)
}

struct Bounds {
    class: usize,
    member: usize,
    package: usize,
}

/// Packages R8 may have moved classes *into*: the root package, packages that are
/// entirely generator-shaped, and the package(s) holding the most maybe-minified classes
/// (a stand-in for an unknown `-repackageclasses` target). A class there may have a kept
/// simple name but a changed package.
fn repackaging_targets(program: &Program, bounds: &Bounds) -> BTreeSet<String> {
    let mut targets = BTreeSet::from([String::new()]);
    let mut minified_per_pkg: BTreeMap<&str, u64> = BTreeMap::new();
    for id in program.class_ids() {
        let desc = program.descriptor(id);
        let pkg = package_of(desc);
        if !pkg.is_empty() && pkg.split('/').all(|seg| may_be_minified(seg, bounds.package) && seg.bytes().all(|b| !b.is_ascii_uppercase())) {
            targets.insert(pkg.to_string());
        }
        if may_be_minified(tail(simple_name_of(desc)), bounds.class) {
            *minified_per_pkg.entry(pkg).or_default() += 1;
        }
    }
    if let Some(&max) = minified_per_pkg.values().max() {
        targets.extend(minified_per_pkg.iter().filter(|(_, &n)| n == max).map(|(p, _)| p.to_string()));
    }
    targets
}

impl Pass for KeptName {
    fn name(&self) -> &'static str {
        "kept-name"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let stats = cx.evidence.name_stats;
        let bounds = Bounds {
            class: length_bound(stats.type_descriptors + CLASS_SLACK),
            member: length_bound(stats.member_names + MEMBER_SLACK),
            package: length_bound(stats.packages + CLASS_SLACK),
        };
        let targets = repackaging_targets(cx.program, &bounds);

        // Fields visible to each class's field-naming state: its own and every program
        // supertype's. R8 names a class's fields from the start of its sequence, skipping only
        // names used in that state, so a minified field can't be further along the sequence
        // than this count (plus slack).
        let field_capacity: Vec<u64> = {
            let p = &*cx.program;
            p.class_ids()
                .map(|id| {
                    let mut seen = BTreeSet::new();
                    let mut stack = vec![id];
                    let mut n = 0u64;
                    while let Some(c) = stack.pop() {
                        if !seen.insert(c) {
                            continue;
                        }
                        let k = p.class(c);
                        n += k.fields.len() as u64;
                        for t in k.superclass.iter().chain(&k.interfaces) {
                            if let Some(sid) = p.find(p.str(*t)) {
                                stack.push(sid);
                            }
                        }
                    }
                    n + FIELD_SLACK
                })
                .collect()
        };

        let mut labels: Vec<(ItemId, Attribute)> = Vec::new();
        for id in cx.program.class_ids() {
            let p = &*cx.program;
            let class = p.class(id);
            let desc = p.descriptor(id);
            let item = ItemId::Class { class: id };
            let simple = simple_name_of(desc);
            let t = tail(simple);
            let in_target = targets.contains(package_of(desc));
            // R8 ≥ 9 names its synthetics `Outer$N` (no marker) and flags them ACC_SYNTHETIC;
            // javac's anonymous `Outer$1` isn't flagged.
            let r8_numbered = class.access & eightr_dex::class::access::SYNTHETIC != 0 && !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit());
            let class_kept = !may_be_minified(t, bounds.class)
                && !is_synthetic(simple)
                && !r8_numbered
                && !desc.starts_with("Lj$/")
                // A kept name moved into a repackaging target gets a numeric suffix on
                // collision (`Rep` → `Rep1`), so a trailing digit there isn't proof.
                && !(in_target && t.ends_with(|c: char| c.is_ascii_digit()));
            if class_kept {
                labels.push((item, Attribute::ClassName));
                if !in_target {
                    labels.push((item, Attribute::Package));
                }
            }
            // Members of a compiler-synthesized class never existed in the source.
            let class_synthetic = is_synthetic(simple) || r8_numbered;
            let member_kept = |name: &str| {
                !class_synthetic
                    && (name == "<init>"
                        || name == "<clinit>"
                        || (!may_be_minified(name, bounds.member) && !is_synthetic(name) && !is_fresh(name)))
            };
            // A generator-shaped field name further along the sequence than this class's field
            // naming state can reach wasn't generated: it was kept (e.g. names pre-obfuscated
            // by a library vendor, or kept for reflection).
            let beyond_capacity = |name: &str| {
                !class_synthetic
                    && !is_synthetic(name)
                    && generator_index(name).is_some_and(|i| i > field_capacity[id.0 as usize])
            };
            for (index, f) in class.fields.iter().enumerate() {
                if member_kept(p.str(f.name)) || beyond_capacity(p.str(f.name)) {
                    labels.push((ItemId::Field { class: id, index: index as u32 }, Attribute::MemberName));
                }
            }
            for (index, m) in class.methods.iter().enumerate() {
                if member_kept(p.str(m.name)) {
                    labels.push((ItemId::Method { class: id, index: index as u32 }, Attribute::MemberName));
                }
            }
        }
        for (item, attr) in labels {
            cx.labels.record(item, attr, KEPT_NAME, None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of R8's `numberToIdentifier` (lowercase mode), used to check the capacity math.
    fn number_to_identifier(mut n: u64) -> String {
        const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
        const REST: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut s = vec![FIRST[((n - 1) % 26) as usize]];
        n = (n - 1) / 26;
        while n > 0 {
            s.push(REST[((n - 1) % 36) as usize]);
            n = (n - 1) / 36;
        }
        String::from_utf8(s).unwrap()
    }

    #[test]
    fn generator_port_matches_r8_study() {
        // Indices from docs/sources/r8-desugar.md §1.1.
        assert_eq!(number_to_identifier(1), "a");
        assert_eq!(number_to_identifier(26), "z");
        assert_eq!(number_to_identifier(27), "a0");
        assert_eq!(number_to_identifier(287), "aa");
        assert_eq!(number_to_identifier(963), "a00");
        for n in [1, 26, 27, 962, 963, 34_658, 34_659, 1_000_000] {
            assert_eq!(length_bound(n), number_to_identifier(n).len(), "n={n}");
        }
    }

    #[test]
    fn generator_index_matches_r8_sequence() {
        assert_eq!(generator_index("a"), Some(1));
        assert_eq!(generator_index("z"), Some(26));
        assert_eq!(generator_index("A"), Some(27));
        assert_eq!(generator_index("Z"), Some(52));
        assert_eq!(generator_index("a0"), Some(53));
        assert_eq!(generator_index("b0"), Some(54));
        assert_eq!(generator_index("zze"), Some(((15 * 62 + 36) * 52) + 26));
        assert_eq!(generator_index("0a"), None);
        assert_eq!(generator_index("a_b"), None);
    }

    #[test]
    fn predicate() {
        let b = 3;
        for n in ["a", "zz", "a0", "h0", "Abc", "p0", "Z9x"] {
            assert!(may_be_minified(n, b), "{n}");
        }
        for n in ["Main", "abcd", "0a", "a_b", "it", "by", "do", "", "<init>", "a$b"] {
            assert!(!may_be_minified(n, b), "{n}");
        }
        assert!(!may_be_minified("abc", 2));
        assert_eq!(tail("Outer$a"), "a");
        assert_eq!(tail("Main$1"), "1");
        assert_eq!(tail("Main"), "Main");
        assert!(is_synthetic("Foo$$ExternalSyntheticLambda0"));
        assert!(is_synthetic("$r8$classId"));
        assert!(is_fresh("work$1"));
        assert!(!is_fresh("$1") && !is_fresh("lambda$main$0x") && !is_fresh("work"));
    }
}
