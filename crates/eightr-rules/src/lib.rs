//! The rule registry (DESIGN.md §0.4).
//!
//! Every transformation 8R performs, and every fact it asserts, is attributed to a registered
//! [`Rule`]. The pipeline refuses to record an application of an unregistered rule, and the
//! registry tests enforce the invariants below, so the rulebook in DESIGN.md can't drift from
//! what the code actually does.
//!
//! Inputs never include a mapping file: 8R works from the shipped artifact alone. Mapping
//! files appear only in tests, as the ground-truth oracle for grading S labels.

use serde::Serialize;

/// How faithful a rule's output is. See DESIGN.md §0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Ambiguous: several preimages are consistent with the evidence. Allowed only if the
    /// rule enumerates **every** candidate (exhaustive), in canonical order. Tested by
    /// checking that the true original is always among the candidates.
    NonDeterministic,
    /// Not the original, but a canonical, α-invariant, more readable form.
    Deterministic,
    /// Provably equal to the original. Tested by exact equality against ground truth.
    Solved,
}

impl Class {
    /// Classes compose by minimum: an attribute touched by an S rule and a D rule is D.
    /// Ordering: N < D < S.
    pub fn compose(self, other: Class) -> Class {
        self.min(other)
    }

    pub fn letter(self) -> char {
        match self {
            Class::Solved => 'S',
            Class::Deterministic => 'D',
            Class::NonDeterministic => 'N',
        }
    }
}

/// The tool or compiler stage whose transformation a rule undoes. 8R treats the build as a
/// stack of transformations and undoes them in reverse order of application, so this enum's
/// order is the **undo order**: R8 ran last, so it is undone first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Not a transformation: analyses and identity that every pipeline run includes.
    Core,
    /// R8 shrinking, optimization, and minification.
    R8,
    /// D8/R8 desugaring: lambdas, backports, default interface methods, nest access, records.
    Desugar,
    /// Jetpack Compose compiler plugin: $composer/$changed params, groups, stability.
    Compose,
    /// kotlinx.serialization compiler plugin: generated serializers and descriptors.
    KotlinxSerialization,
    /// kotlin-parcelize plugin: generated CREATOR/writeToParcel.
    Parcelize,
    /// kotlinc lowerings: coroutine state machines, data classes, when-mappings, intrinsics.
    Kotlinc,
    /// javac lowerings: string switch, enum switch maps, string concat, synthetic accessors.
    Javac,
}

/// The attributes of an item that labels are tracked on (DESIGN.md §0.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribute {
    Package,
    ClassName,
    MemberName,
    Signature,
    Body,
    Lines,
    SourceFile,
}

#[derive(Debug)]
pub struct Rule {
    /// `source/name`, e.g. `r8/kept-name`. Unique.
    pub id: &'static str,
    pub source: Source,
    pub class: Class,
    pub attributes: &'static [Attribute],
    pub summary: &'static str,
    /// Human-readable; each must be mirrored by a runtime check in the implementation.
    pub preconditions: &'static [&'static str],
    /// Rule used when this rule's preconditions fail for an item. Must have class ≤ ours.
    pub fallback: Option<&'static str>,
    /// Fixture directories (under `fixtures/src/`) that exercise this rule. Must be
    /// non-empty and exist.
    pub fixtures: &'static [&'static str],
}

use Attribute as A;

pub const IDENTITY: &str = "core/identity";
pub const KEPT_NAME: &str = "r8/kept-name";

/// All registered rules, sorted by id.
pub static REGISTRY: &[Rule] = &[
    Rule {
        id: IDENTITY,
        source: Source::Core,
        class: Class::Deterministic,
        attributes: &[A::Package, A::ClassName, A::MemberName, A::Signature, A::Body, A::Lines, A::SourceFile],
        summary: "Leave the attribute exactly as the input has it. The default for anything no other rule touched.",
        preconditions: &[],
        fallback: None,
        fixtures: &["hello", "shapes", "opcodes"],
    },
    Rule {
        id: KEPT_NAME,
        source: Source::R8,
        class: Class::Solved,
        attributes: &[A::Package, A::ClassName, A::MemberName],
        summary: "A name R8's minifier cannot have produced is the original name.",
        preconditions: &[
            "The build used R8's default naming (no -obfuscationdictionary / -classobfuscationdictionary).",
            "Class: neither the simple name nor its last '$' segment is 1-3 ASCII letters.",
            "Package: R8 renames a class descriptor as a unit, so a kept class name implies a kept package.",
            "Member: the name is not 1-3 ASCII letters, or it is <init>/<clinit>.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["hello", "shapes", "opcodes"],
    },
];

pub fn lookup(id: &str) -> Option<&'static Rule> {
    REGISTRY.binary_search_by(|r| r.id.cmp(id)).ok().map(|i| &REGISTRY[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sorted_and_unique() {
        for w in REGISTRY.windows(2) {
            assert!(w[0].id < w[1].id, "registry must be sorted by id: {} >= {}", w[0].id, w[1].id);
        }
    }

    #[test]
    fn ids_match_source_prefix() {
        for r in REGISTRY {
            let prefix = serde_plain_source(r.source);
            assert!(r.id.starts_with(&format!("{prefix}/")), "{} should start with {prefix}/", r.id);
        }
    }

    fn serde_plain_source(s: Source) -> &'static str {
        match s {
            Source::Core => "core",
            Source::R8 => "r8",
            Source::Desugar => "desugar",
            Source::Compose => "compose",
            Source::KotlinxSerialization => "kxs",
            Source::Parcelize => "parcelize",
            Source::Kotlinc => "kotlinc",
            Source::Javac => "javac",
        }
    }

    #[test]
    fn fallbacks_exist_and_are_no_stronger() {
        for r in REGISTRY {
            if let Some(f) = r.fallback {
                let fb = lookup(f).unwrap_or_else(|| panic!("{}: fallback {f} not registered", r.id));
                assert!(fb.class <= r.class, "{}: fallback {f} is stronger", r.id);
                assert!(r.attributes.iter().all(|a| fb.attributes.contains(a)), "{}: fallback {f} must cover all attributes", r.id);
            }
        }
    }

    #[test]
    fn every_rule_has_existing_fixtures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/src");
        for r in REGISTRY {
            assert!(!r.fixtures.is_empty(), "{} has no fixtures", r.id);
            for f in r.fixtures {
                assert!(root.join(f).is_dir(), "{}: fixture {f} missing", r.id);
            }
        }
    }

    #[test]
    fn solved_rules_state_preconditions() {
        for r in REGISTRY.iter().filter(|r| r.class == Class::Solved) {
            assert!(!r.preconditions.is_empty(), "{}: an S rule must state what it proves from", r.id);
            assert!(r.fallback.is_some(), "{}: an S rule needs a fallback for when proof fails", r.id);
        }
    }

    #[test]
    fn composition() {
        use Class::*;
        assert_eq!(Solved.compose(Deterministic), Deterministic);
        assert_eq!(Deterministic.compose(NonDeterministic), NonDeterministic);
        assert_eq!(Solved.compose(Solved), Solved);
        assert!(Source::R8 < Source::Desugar && Source::Desugar < Source::Compose && Source::Compose < Source::Kotlinc);
    }
}
