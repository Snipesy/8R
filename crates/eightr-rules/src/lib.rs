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
    /// Parameter names (debug info).
    ParamNames,
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
pub const SPLIT_MERGED_CLASS: &str = "r8/split-merged-class";
pub const ENUM_UNBOXING_UTILITY: &str = "r8/enum-unboxing-utility";
pub const REBOX_ENUM: &str = "r8/rebox-enum";
pub const COMPOSE_RUNTIME_API: &str = "compose/runtime-api";
pub const COMPOSE_SINGLETONS: &str = "compose/singletons";
pub const COMPOSE_SYNTHETIC_PARAMS: &str = "compose/synthetic-params";
pub const ANNOTATION_MEMBER_NAME: &str = "r8/annotation-member-name";
pub const LIBRARY_OVERRIDE_NAME: &str = "r8/library-override-name";
pub const LATEINIT_FIELD_NAME: &str = "kotlinc/lateinit-field-name";

/// All registered rules, sorted by id.
pub static REGISTRY: &[Rule] = &[
    Rule {
        id: COMPOSE_RUNTIME_API,
        source: Source::Compose,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "Name the Compose runtime's Composer members by the roles the compiler plugin's calls give them (startRestartGroup, endRestartGroup, shouldExecute / getSkipping, skipToGroupEnd, rememberedValue, updateRememberedValue, changed, changedInstance) and updateChangedFlags.",
        preconditions: &[
            "The Composer is found structurally: the class whose (I)C method is the constant-key entry call of the most methods (>= 5, 3x the runner-up).",
            "Each role is played by one method in most composables showing it (4x the runner-up): the composable's first skip check, shouldExecute (ZI)Z skipping when false or getSkipping ()Z skipping when true (dropped when (ZI)Z votes exist), with the skip path's first composer ()V call as skipToGroupEnd; rememberedValue compared then updateRememberedValue(Object); the null-tested object result as endRestartGroup.",
            "changed: every (prim)Z composer method whose result is branched on, other roles' winners excluded; (Object)Z candidates named by body (equals-like call: changed; identity compare only: changedInstance), each verdict held by one candidate.",
            "updateChangedFlags: the static (I)I whose result is an argument of calls to restartable composables (>= 3, 4x the runner-up) and whose body holds the runtime's masks 0x12492492 and 0x24924924.",
            "Override groups (the composer's program supertypes and all their subtypes) are named together.",
        ],
        fallback: Some(STRUCTURAL_NAME),
        fixtures: &["compose_shapes", "compose_shapes_k21", "compose_witness", "compose_witness_k21", "compose_basic", "compose_basic_r94"],
    },
    Rule {
        id: COMPOSE_SINGLETONS,
        source: Source::Compose,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "Name ComposableSingletons fields lambda$K: static fields a <clinit> sets to new ComposableLambdaImpl(K, false, block).",
        preconditions: &[
            "The lambda class is found by behavior: a method opens the composer's startRestartGroup keyed by an int field of its class, which a constructor stores from its first parameter.",
            "The <clinit> stores the instance built with a constant K straight into the field; K is stored into no other field.",
            "The compiler era names these fields lambda$K (>= 2.1.20): proven by the shouldExecute skip check (>= 2.2 by default); refused in the getSkipping era, whose older compilers named them lambda-N.",
        ],
        fallback: Some(STRUCTURAL_NAME),
        fixtures: &["compose_shapes", "compose_shapes_k21", "compose_witness", "compose_basic", "compose_basic_r94"],
    },
    Rule {
        id: COMPOSE_SYNTHETIC_PARAMS,
        source: Source::Compose,
        class: Class::Deterministic,
        attributes: &[A::ParamNames],
        summary: "Name a restartable composable's synthetic parameters ($composer, $changed[K], $default[K]) in debug info, and record its key, roles, parameter bindings and slot bound in a build annotation (@eightr.Composable); its restart lambda gets @eightr.RestartScope.",
        preconditions: &[
            "The composable opens with the composer's startRestartGroup(K) on a parameter: that parameter is $composer (S).",
            "$changed: the parameters receiving updateChangedFlags(..) results at every restart-lambda call back (role S; the index K follows residual order, D: R8 may drop a constant $changedK).",
            "$default: an int parameter used only in single-bit tests (captures aside), one of which selects between another parameter's value and a default (a phi). D.",
            "Bindings kept only when unambiguous (one per parameter, one per slot or bit). Nothing changes code.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["compose_shapes", "compose_shapes_k21", "compose_witness", "compose_witness_k21", "compose_basic", "compose_basic_r94"],
    },
    Rule {
        id: IDENTITY,
        source: Source::Core,
        class: Class::Deterministic,
        attributes: &[A::Package, A::ClassName, A::MemberName, A::Signature, A::Body, A::Lines, A::SourceFile, A::ParamNames],
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
        id: ENUM_UNBOXING_UTILITY,
        source: Source::R8,
        class: Class::Solved,
        attributes: &[A::MemberName],
        summary: "Name the members of R8's shared enum-unboxing utility: `$VALUES`, `ordinal`, `values` (R8 generates these names verbatim).",
        preconditions: &[
            "A synthetic class with a static int[] set in <clinit> to exactly {1..N} by filled-new-array.",
            "`ordinal`: a static (I)I that throws on 0 and returns x - 1; `values`: a static (I)[I copying a prefix of the array with System.arraycopy. At least one of them present.",
        ],
        fallback: Some(STRUCTURAL_NAME),
        fixtures: &["r94_enum", "shapes"],
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
    Rule {
        id: REBOX_ENUM,
        source: Source::R8,
        class: Class::Deterministic,
        attributes: &[A::Body],
        summary: "Re-create an enum R8 unboxed (proven constant names, canonical name when recovered) and turn a method's web of its values back into enum objects; other int uses read the exact adapter `$8r$unboxed(e)` = e == null ? 0 : e.ordinal() + 1.",
        preconditions: &[
            "The enum's constants are proven by an inlined valueOf (or an equals map agreeing with a name() chain).",
            "A web is seeded by the value an inlined name() chain compares and closed over copies and merges within the method; its values only come from constants 0..N and elements of the utility's values(N) array.",
            "Free registers exist for the materialized constants and adapters, and every instruction stays encodable; otherwise the method is left as is.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["r94_enum"],
    },
    Rule {
        id: SPLIT_MERGED_CLASS,
        source: Source::R8,
        class: Class::Deterministic,
        attributes: &[A::Body],
        summary: "Split a class R8's horizontal merger combined (merged siblings, lambda groups) into an abstract base plus one subclass per class id, each overriding the id-dispatching methods with its own arm.",
        preconditions: &[
            "A synthetic final byte/short/int instance field stored by the class's constructors once, in the entry block before `this` escapes, from a constant or a parameter (delegation only to constructors not storing it); every read of it anywhere only feeds if/switch operands (R8's class id is only a dispatch key).",
            "Every instantiation passes a constant id and pairs one new-instance with one <init>; no program subclass, not Serializable, no const-class, method handle, annotation or constant naming the class.",
            "No used getClass() on a value syntactically typed as the class (definition, parameter type, check-cast, array element). An approximation: values typed Object or as an interface escape it; R8 merged the class under the same assumption.",
            "S facts: at least (number of ids) classes were merged, and each override is exactly that id's behavior. The split form (base + subclasses, names) is D.",
        ],
        fallback: Some(IDENTITY),
        fixtures: &["r94_merging", "kotlin_basic", "kotlin_basic_r94", "compose_basic_r94"],
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
