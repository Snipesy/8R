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
pub const STRUCTURAL_NAME: &str = "core/structural-name";
pub const STRUCTURAL_TIE: &str = "core/structural-tie";
pub const KEPT_NAME: &str = "r8/kept-name";
pub const OUTLINE_INLINE: &str = "r8/outline-inline";
pub const BU_OUTLINE_INLINE: &str = "r8/bu-outline-inline";
pub const ANNOTATION_MEMBER_NAME: &str = "r8/annotation-member-name";
pub const LIBRARY_OVERRIDE_NAME: &str = "r8/library-override-name";
pub const LATEINIT_FIELD_NAME: &str = "kotlinc/lateinit-field-name";

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
        id: STRUCTURAL_NAME,
        source: Source::Core,
        class: Class::Deterministic,
        attributes: &[A::ClassName, A::MemberName],
        summary: "Every name no rule proved gets a deterministic structural name {hint}_{hash} (Weisfeiler-Lehman over the program with non-S names erased).",
        preconditions: &[
            "Renaming must preserve behavior: fields, static and private methods always; a virtual method only if its whole override group lives in classes whose only library supertype is java.lang.Object and it doesn't override an Object method.",
            "New names are unique program-wide per member kind and per package for classes, so no override, shadowing, or clash is introduced.",
            "Structurally indistinguishable candidates are suffixed in input order and reported (only truly automorphic ties are alpha-invariant).",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["hello", "shapes", "opcodes", "names_stress", "dontobfuscate", "kotlin_basic", "kotlin_serialization", "compose_basic"],
    },
    Rule {
        id: STRUCTURAL_TIE,
        source: Source::Core,
        class: Class::NonDeterministic,
        attributes: &[A::ClassName, A::MemberName],
        summary: "Items the structural hash can't tell apart get distinct suffixed names; which item got which suffix is reported as the full candidate set.",
        preconditions: &[
            "Exhaustive: the candidate set is every name assigned to the tie group, so the item's name is certainly among them.",
            "For truly automorphic items (interchangeable, e.g. identical unreferenced interfaces), the emitted program is identical whatever the assignment.",
        ],
        // Exhaustive by construction: there is no failure mode to fall back from.
        fallback: None,
        fixtures: &["kotlin_basic", "kotlin_serialization", "compose_basic"],
    },
    Rule {
        id: LATEINIT_FIELD_NAME,
        source: Source::Kotlinc,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "A lateinit property's backing field is named after the property, and kotlinc's uninitialized-access check names it.",
        preconditions: &[
            "The message is kotlinc/stdlib's exact template: const \"lateinit property X has not been initialized\" (R8-folded), or const \"X\" passed to a helper whose body holds \"lateinit property \" and \" has not been initialized\".",
            "Every path into the message's block is the null branch of a test on a register whose reaching definitions all read the same program field F (iget/sget, through at most one move).",
            "Binding goes through the field reference, which survives inlining; all messages bound to F agree (otherwise refused and reported).",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["kotlin_basic"],
    },
    Rule {
        id: ANNOTATION_MEMBER_NAME,
        source: Source::R8,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "R8 never renames the methods of an annotation interface.",
        preconditions: &[
            "The declaring class has ACC_ANNOTATION (R8's minifier reserves names when getHolder().isAnnotation(); verified by the R8 audit, E16).",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["opcodes"],
    },
    Rule {
        id: BU_OUTLINE_INLINE,
        source: Source::R8,
        class: Class::Deterministic,
        attributes: &[A::Body],
        summary: "Inline R8's bottom-up (throw) outlines back into their call sites and drop the dead default return after them.",
        preconditions: &[
            "Static method of a synthetic class, with straight-line code ending in `throw` of a freshly constructed exception, reached only by invoke-static from >= 2 sites.",
            "Neither its class nor any superclass has a <clinit>, the class has no program subclasses, and everything it references (resolving inherited members) is accessible from each caller.",
            "Outlines it calls are inlined into it first (callee-first order; cycles are skipped).",
            "Registers stay encodable after the splice; otherwise that site keeps its call.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["r94_outline"],
    },
    Rule {
        id: KEPT_NAME,
        source: Source::R8,
        class: Class::Solved,
        attributes: &[A::Package, A::ClassName, A::MemberName],
        summary: "A name R8's minifier cannot have produced is the original name.",
        preconditions: &[
            "Unverifiable from the dex, stated in the report: no -obfuscationdictionary/-classobfuscationdictionary/-packageobfuscationdictionary, no -applymapping.",
            "Generator shape: R8 names are [a-zA-Z][0-9a-zA-Z]* (SymbolGenerationUtils.numberToIdentifier). A name is maybe-minified if it has that shape and is no longer than the namespace's capacity bound (computed from distinct type/member-name counts plus slack); it/by/do are never generated.",
            "Class: the part after the last '$' (R8 names inner classes <outer>$<gen>) is not maybe-minified; no R8/D8 synthetic marker; not a j$ (desugared library) class; not a trailing-digit name inside a repackaging target (collision suffix, Rep -> Rep1).",
            "Package: the class name is kept and the package is not a possible repackaging target (root, all-generator-shaped segments, or the package holding the most maybe-minified classes).",
            "Member: <init>/<clinit>, or not maybe-minified, not synthetic, and not a fresh name$N.",
            "Field: or its position in R8's member-name sequence is beyond what the class's field-naming state can reach (fields of the class and all program supertypes, plus slack): R8 names fields from the start of the sequence, so such a name was kept, not generated.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["hello", "shapes", "opcodes", "names_stress"],
    },
    Rule {
        id: LIBRARY_OVERRIDE_NAME,
        source: Source::R8,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "A method overriding a library method keeps the library's name (R8 reserves it).",
        preconditions: &[
            "Non-static, non-private, non-constructor method of a program class C.",
            "The dex references a library method (owner not a program class) with the same name and descriptor, whose owner is among C's library supertypes (collected through program supertypes).",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["shapes", "kotlin_basic", "compose_basic"],
    },
    Rule {
        id: OUTLINE_INLINE,
        source: Source::R8,
        class: Class::Deterministic,
        attributes: &[A::Body],
        summary: "Inline R8's outlines (shared straight-line helpers) back into their call sites.",
        preconditions: &[
            "Static method of a synthetic class, with straight-line code ending in a return, reached only by invoke-static from >= 2 sites.",
            "At least 3 operations (R8's minimum outline size) including a call, and it only calls and instantiates library classes: keeps out interface companions (app calls), backports (pure arithmetic) and API-model outlines (one call).",
            "Neither its class nor any superclass has a <clinit>, and the class has no program subclasses.",
            "Registers stay encodable after the splice; otherwise that site keeps its call.",
            "D, not S: a hand-written static helper can have the same shape (inlining it is still behavior-preserving).",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["r94_outline", "kotlin_serialization", "r94_desugar"],
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
