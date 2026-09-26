# 8R: a deterministic inverse of R8

Status: draft design · Language: Rust

## 0. The rules

R8 is a function `R8(program, keep_rules, config) -> (dex, mapping.txt)`. It is **not injective**
(dead code is deleted, constants are folded, classes are merged, names become `a`, `b`, `c`),
so "undo R8" needs a precise contract. 8R is built around three classes of **rule**.

**Input is the shipped artifact only (DEX/APK/AAB). No ProGuard/R8 mapping file is ever
available.** Every rule must work from the binary alone. Mapping files appear only in the test
suite, as the held-back ground truth used to grade 8R (§6.1). Wherever this document still
describes a "with mapping" behavior, read it as "what the oracle can verify", not as a
product mode.

8R also undoes more than R8. R8 is the main focus, but the build is a stack of transformation
**sources**: kotlinc lowerings, compiler plugins (Compose, kotlinx.serialization, Parcelize,
…), desugaring, and R8 on top. Each rule belongs to one source, and 8R unwinds the stack in
reverse order (§1.1).

### 0.1 The three classes

| Class | Meaning | What it promises | How it's tested |
|---|---|---|---|
| **Solved (S)** | R8 can be undone 100% | The output attribute **equals the original**. The evidence determines the preimage uniquely. | Exact equality against a ground-truth oracle (§6.1). A single mismatch is a hard failure. |
| **Deterministic (D)** | Not fully undone, but deterministically put back into an easier-to-read form | The output is a **canonical function of the input's meaning**. It's semantically equivalent and more readable. It is *not* claimed to be the original. | α-invariance (§0.3), semantic equivalence, snapshot tests. |
| **NonDeterministic (N)** | Ambiguous: several preimages are consistent with the evidence | The rule outputs the **complete** candidate set, in canonical order. **All possible paths are accounted for**: the original is guaranteed to be a member. N is allowed, but not preferred. | Exhaustiveness: the true original (from the oracle) must be in the candidate set, every time. The set itself must be α-invariant. |

**N requires a finite, enumerable candidate set.** If the preimage is unbounded (e.g. "the
serial name, or *any* identifier annotated `@SerialName(s)`"), N is impossible and the rule
must be D (a hint-bearing canonical name) or identity.

What is never allowed is an *unaccounted* choice: picking one candidate arbitrarily (by
obfuscated name, input order, hash order, or randomness) and presenting it alone. An ambiguous
problem is handled in one of four ways, in order of preference:
1. **Promote to S.** Find evidence that pins the answer down.
2. **Demote to D.** Define a canonical form that depends only on structure, then implement
   that instead of a guess.
3. **Enumerate as N.** Emit every candidate. In the DEX, the attribute takes the
   canonical D form; the report carries the full candidate set, so downstream tools (or a
   later pass with more evidence) can narrow it.
4. **Leave it as identity.** Keep R8's form untouched and annotate it in the report. The
   identity transform is trivially D.

So **8R's output is always deterministic**, including N results: the candidate *set* is a
pure function of the input. The S/D/N split measures how *faithful* it is.

### 0.2 Hard invariants (apply to every rule)

1. **Semantics are preserved.** `dex'` behaves identically to `dex`. A rule whose
   preconditions fail doesn't run.
2. **Labels are honest.** A rule may only label its output S if it *checked at runtime* that
   the S preconditions hold for that specific item. When unsure, it's D.
3. **Classes compose by minimum** (N < D < S). Labels are tracked per attribute: package, simple name,
   member name, signature, body, lines, source file. If any rule that touched an attribute was
   D, the attribute is D. (`S ∘ S = S`, `S ∘ D = D`.)
4. **Names show their class.** A name without an 8R marker is **guaranteed original** (S).
   Every D name carries a hash suffix, like `UserRepo_3fa9` or `Activity_7c21`, so a reader
   can see at a glance what's proven and what isn't.
5. **Pure function.** The output depends only on (input bytes, mapping bytes, config, 8R
   version, sigdb version). Thread count, file order, hash seeds, locale, and time have no effect.
6. **Idempotent.** `8R(8R(x)) == 8R(x)`.
7. **Stable.** A small source change causes a small change in D names.

### 0.3 The operational test for "not N": α-invariance

R8's choices of minified names (`a`, `b`, …) and of class/method order in the dex are
arbitrary. So any decision that depends on them is effectively random. That gives a precise
definition:

> **A rule's output (a D form, or an N candidate set) must be invariant under any consistent
> renaming of minified identifiers and any reordering of classes, members, and dex files in
> the input.** An output that fails this is an unaccounted choice, which is forbidden.

Examples:
- "Name these two equally plausible classes by sorting on their obfuscated names" is an
  **unaccounted choice**: renaming the input flips the result. The legal alternatives are a
  structural D name, or N with both candidates.
- "Name by Weisfeiler–Lehman structural hash" **is D**. Structure doesn't change under
  renaming.

Canonical ordering throughout 8R is therefore by *(S names, then structural hash)*, **never**
by obfuscated name or input position. Suppose WL refinement can't distinguish two classes.
Then either they're truly automorphic, so swapping them yields byte-identical output anyway, or
8R falls back to individualization-refinement canonical labeling (nauty-style), which is still
structural.

This invariant is a property test (§6.4). It is what keeps unaccounted choices out of the
pipeline.

**Documented exception (semantic safety wins):** a member whose name equals a string
constant in its declaring class may be looked up by name (serializer and protobuf-style
tables), so it keeps its name (`reflect::pins`). For names at low positions in R8's generator
sequence (like `b`), "kept for reflection" and "generated, coincidentally equal to a string" are
indistinguishable, so the pin can depend on R8's choice in the coincidental case. The α test
therefore doesn't permute names that equal string constants. Determinism (output is a pure
function of the input) is unaffected.

It applies to the **final** output, after naming (§5.5). The identity labels used during
passes are not α-invariant for names (renaming the input renames them), which is expected:
identity is a placeholder until the naming stage replaces every non-S name with a structural D
name. (Flagged by the R8 audit.)

### 0.4 The rule registry

Every transformation 8R performs is a registered rule. It's code, not prose, and lives in
`crates/eightr-rules`:

```rust
pub enum Class { NonDeterministic, Deterministic, Solved }   // ordered: N < D < S

pub struct Rule {
    id: &'static str,                    // "<source>/<name>", e.g. "r8/kept-name"
    source: Source,                      // R8, Desugar, Compose, KotlinxSerialization, …
    class: Class,
    attributes: &'static [Attribute],    // which item attributes it may label
    summary: &'static str,
    preconditions: &'static [&'static str], // human-readable, mirrored by runtime checks
    fallback: Option<&'static str>,      // rule used when S preconditions fail (never stronger)
    fixtures: &'static [&'static str],   // must be non-empty and exist; CI enforces this
}
```

Each application emits `(rule_id, item, attribute, class, candidates?)` into the report. An N
application must carry a non-empty candidate set; S and D applications must not. The report then
summarizes faithfulness per app: "names: 91.2% S / 8.8% D; bodies: 64% S / 36% D".

## 1. Rulebook: R8 transformations classified

> **Status:** this table predates the no-mapping decision and is being re-audited. The
> **"Without mapping" column is the product.** The "With mapping" column is kept only as a
> statement of what the test oracle can verify. Per-source research and an adversarial audit
> of these R8 rows are landing in `docs/sources/` and will replace this table with one
> generated from the registry (`cargo xtask rulebook`).

S = solved, D = deterministic, "D·id" = identity (left as-is, annotated), ✗N = the naive
approach would be an unaccounted choice, so it's not done (shown for the record). N rows
(exhaustive candidate sets) will be added where the audit finds finite, enumerable ambiguity.

| R8 transformation | With mapping | Without mapping |
|---|---|---|
| **Class/member renaming** | **S** (oracle) | **S** only via `r8/kept-name` (generator-shape predicate, §1.2 audit) and proven evidence. Otherwise **D**: `{hint}_{hash}` names (§5.5). |
| **Repackaging** (`-repackageclasses`, and **by default**: R8 ≤8.x moves packages to `a`, `b`, …; ≥9.0 repackages into the root) | **S** | **D**: structural package clustering. A kept class name in a possible repackaging target has a **D** package; a trailing-digit name there is a collision suffix candidate (N: `{Rep1, Rep}`). |
| **Aggressive overloading** (same name, different return type) | **S** | **D**: split into distinct `_{hash}` names |
| **Line numbers** (compaction, pc-encoding) | **S** | D·id: pc-lines are stripped, not fabricated. ✗N: inventing plausible line numbers. |
| **Source file attribute** (plain R8 8.x writes `SourceFile`; R8 9.4 and AGP write `r8-map-id-…`) | **S** | **D**: `{OuterClass}_{hash}.kt`/`.java`, with the language from Kotlin evidence. ✗N: guessing `FooKt` file facades. |
| **Inlining** | **D**: frame-guided extraction (§4.6). The body *may* equal the original, but 8R can't prove that R8 didn't optimize across the boundary after inlining, so it's labelled D. Specialized per-site copies are also D. | D·id. ✗N: splitting methods into guessed callees with no frame boundaries. |
| **Outlining** (classic, and R8 ≥ 9 bottom-up throw outlines) | **S**: `outlineCallsite` gives exact positions, and inlining back is pure code motion. | **D** (`r8/outline-inline`, `r8/bu-outline-inline`): static straight-line method in an `ACC_SYNTHETIC` holder, ≥ 2 `invoke-static` sites, plus a body shape per kind (r8-desugar.md §4.5). It might inline a genuine synthetic helper, which is still readable and still correct. |
| **Lambda desugaring** | **S** when re-sugared to `invoke-custom` with the body method named from the mapping (min-api permitting). **D** when emitted as a named inner class (the default). | **D** |
| **Backports** (usually *inlined* by R8, even with many callers) | **S** | Where a synthetic survives: **S** if the body matches exactly one version-keyed template; **N** when templates are shared (e.g. `Math.X`/`StrictMath.X` compile to byte-identical dex). |
| **API-model outlines** | **S** | **S** (exact template shape, unique target) |
| **Interface `$-CC` companions** | **S** | **SPLIT**: default methods (with their dispatch stub) S; a static method on the companion is **N** {static interface method, other} since default and static shapes are indistinguishable and statics have no owner link. |
| **Nest-access bridges** | **S** | **SPLIT on `ACC_BRIDGE`/synthetic**: S when flagged; otherwise it may be a javac `access$NNN` accessor, which is original code → D·id. |
| **Horizontal class merging** | **S** | **D** when a `$r8$classId` dispatch is visible; the number of distinct classId values is only a **lower bound** on the original class count. Often **invisible** (no classId when no virtual dispatch is needed, merged classes with different interfaces, shared fields) → D·id. |
| **Vertical class merging** | **S**: original owners come from the mapping. | D·id. ✗N: guessing where the subclass boundary was. |
| **Enum unboxing** | **S** | **D**: re-boxed via the utility pattern. Constant names are a **hint** (strings are deleted when `name()` is unused, and survive as ordinary int→string code otherwise). |
| **Staticizing / devirtualization** | **S** (`residualsignature`) | D·id. ✗N: guessing that a static method was once an instance method. |
| **Argument reordering** | **S** | D·id. ✗N |
| **Unused-argument removal** | **D**: the signature is restored exactly and the body is S, but call sites pass a canonical `0`/`null`, because the original values are lost (and never mattered). | D·id. ✗N |
| **Return-value removal** | **D**: the signature is restored, but the returned value is lost. | D·id. ✗N |
| **Access modification** | **D**: tightened to the minimal access that verifies. Originals are never recorded. | **D** (same) |
| **Constant / member-value propagation** | D·id, annotated where detectable. ✗N: re-parameterizing guessed constants. | D·id |
| **Dead code / tree shaking** | D·id. The report lists mapping entries with no residual code. | D·id |
| **Class inlining / scalar replacement** | D·id, annotated from frames | D·id |
| **Local names, generics, stripped annotations** | D·id | D·id. Kept `Signature` strings contain *minified* type names → D, never S. |
| **Kotlin intrinsics parameter names** | **S** | **Hint**, and usually **absent**: R8 ≥ 9.0.26 rewrites Kotlin null checks to `getClass()` by default, dropping the string. Where present (≤ 9.0.25, or opted out), inlining can move a callee's `checkNotNullParameter(p, "title")` into a caller whose parameter is `headline` (audit E10). S only with an inlining-proof binding (open problem). |
| **Enum constant names** | **S** | **Hint**: pre-obfuscated SDKs ship enums whose field is `a` but whose name string is `"EUROPE"`, and the strings vanish when unused. |
| **Kotlin data-class / record `toString`** | **S** | **Records: nothing** (R8 rewrites the names string to minified `"a;b"`). **Kotlin: hint**, S only for an *unfused* template bound to the class (R8 folds call-site constants into the template). |
| **kotlinx.serialization descriptor names** | **S** | **D** (hint) on its own: `@SerialName` can override and even mimic the default. The kxs study's S path relies on Tier-A evidence, which the audit downgraded (see §1.2): S claims there wait on an inlining-proof binding. Enum-entry overrides are exactly detectable (S). |
| **Log tags, exception messages** | n/a | **D** (hint only) |
| **SigDB library match** | n/a | **D** (hint only). Fuzzy matching is never proof. |

"Hint only" means the evidence contributes the `{hint}` part of a D name (`UserRepository_3fa9`)
but is never presented as the original.

### 1.1 Sources and undo order

The build applied transformations in this order: kotlinc/javac lowerings → compiler plugins →
desugaring → R8. 8R undoes them in reverse. The `Source` enum's order *is* the undo order:

| Order | Source | Rule prefix | Research |
|---|---|---|---|
| 0 | Core (identity, analyses) | `core/` | – |
| 1 | R8 (shrink/optimize/minify) | `r8/` | `docs/sources/r8-desugar.md`, `docs/sources/r8-rule-audit.md` |
| 2 | D8/R8 desugaring | `desugar/` | `docs/sources/r8-desugar.md` |
| 3 | Jetpack Compose compiler | `compose/` | `docs/sources/compose.md` |
| 4 | kotlinx.serialization plugin | `kxs/` | `docs/sources/kotlinx-serialization.md` |
| 5 | kotlin-parcelize | `parcelize/` | `docs/sources/other-plugins.md` |
| 6 | kotlinc lowerings | `kotlinc/` | `docs/sources/kotlinc.md` |
| 7 | javac lowerings | `javac/` | `docs/sources/other-plugins.md` |

Annotation processors (Dagger/Hilt, Room, Moshi codegen) generate *additional* code rather
than transforming user code. They're mainly **evidence** sources (SQL strings, JSON names,
factory names), covered in `other-plugins.md`.

Source detection is positive-evidence only (a D8/R8 marker, or referenced types like
`Landroidx/compose/runtime/Composer;`). Because R8 also renames library classes, absence of
evidence is never treated as absence of a source. (Compose must be detected structurally: R8 renames `Composer`; see `docs/sources/compose.md` §3 `compose/detect`.)

### 1.2 Per-source research and proposed rules

Proposed rules live in `docs/sources/*.md` until implemented. A rule is added to the registry
(`crates/eightr-rules`) only together with its implementation, runtime precondition checks,
and a fixture. That keeps "registered" meaning "real".

| Source doc | Status | Headline findings |
|---|---|---|
| `kotlinx-serialization.md` | Done (plugin `06003680c5`, runtime `397bb56009`; verified with kotlinc 2.4.20 + R8 8.10.9) | ~25 proposed `kxs/` rules. **S:** `$serializer`/companion structure and names the plugin fixes or consumer rules keep, synthetic-ctor marker restoration (the plugin always passes `null`), element table, element↔field binding, property order, optionality, enum-entry `@SerialName`. **Names:** S only via Tier-A evidence; otherwise D hints. **Finite N:** collection interface choice (`List`/`MutableList`), FQN package/nesting split. **R8 effects:** inlines `write$Self$<module>` (the module name is lost, D), strips `@SerialName` and `kotlin.Metadata`. Fixtures need real (non-constant) encoders or R8 folds the fingerprints away. |
| `r8-desugar.md` | Done (R8 `a7ad18a7` = 8.10.9, HEAD, spot checks to 9.4.23) | R8's generator is `[a-zA-Z][0-9a-zA-Z]*` (little-endian; `a0` after 26/52 names), inner classes `Outer$gen`, `allowrepackage` keeps names but moves packages (collision suffix `Rep1`), ≥9.0 repackages into root by default, 8.13+ lowercase class names. Supplied the sound predicate now used by `r8/kept-name`. Synthetic fingerprints, backport templates, optimization fingerprints, line/SourceFile, N enumerations, 28 fixture ideas. |
| `r8-rule-audit.md` | Done (R8 `decf0a4a`, branch 8.10) | Adversarial pass over every §1 row. **`r8/kept-name` was unsound** (now fixed and regression-tested by `names_stress` and `dontobfuscate`). Downgrades folded into §1: backports S→S/N, `$-CC` and nest bridges split, intrinsics/enum/toString names → hints, records → nothing, horizontal merging often invisible. Unverifiable preconditions: `-applymapping` and dictionaries. New sound rules proposed: `r8/library-override-name`, `r8/annotation-member-name`, `r8/jni-symbol`. |
| `compose.md` | Done (kotlinc 2.3.20, runtime 1.10.6, R8 8.10.9) | **`sourceInformation` strings never survive R8** (consumer rules mark them side-effect free); trace strings only with a tracer or `-dontoptimize`. **Restartable composables' group keys survive** (`hash("fun-Name(Params)Ret/pkg-…/file-….kt")`, stable across compiler 2.1–2.4 and most library minors; never invertible, but a key DB names library composables); non-restartable/inline composables keep no key after R8 (updated with docs/research/, R8 9.4.24). `$changed`/`$default` bitmasks survive and index original parameter positions; `$stable` is removed. R8 drops/reorders params and re-homes methods. Rules: `@Composable` detection S, synthetic-param naming by behavior S, defaults from `$default` bits S; everything else annotates rather than rewrites (the Compose lowering must stay for the runtime). |
| `other-plugins.md` | Done (R8 8.10.9) | Best S names per effort: **Moshi** (`unexpectedNull`/`missingProperty` carry the Kotlin name separately from the JSON name), **Room** (schema-validation strings give entity FQNs; no annotation overrides them), **ViewBinding/Safe Args** (id constants + resources.arsc → exact generated names), Parcelize (order-linking), Dagger/Hilt (nothing survives, only manifest-kept names propagate). javac lowerings are D-only re-sugaring. |
| `kotlinc.md` | Done (kotlinc 2.4.20; R8 8.10.9 and 9.5.20-dev) | ~40 proposed `kotlinc/` rules. **Null-check parameter names are dead by default from R8 9.0.26** (`-processkotlinnullchecks` rewrites them to `getClass()`, which R8 also emits for its own checks). **Surviving evidence:** lateinit names (all R8 setups tried); function-reference and delegated-property strings (`"load(Ljava/lang/String;…)"`, `"getObserved()I"`), which carry member names, `@JvmName`, and class names in JVM signatures; coroutine `DebugMetadata` (class/function/file/lines, compat mode only), which is a rare source of original **package** names for repackaged classes. **Value-class mangling** is a 40-bit MD5 of parameter types (not the name): it verifies candidates, never recovers names. Kotlin metadata survives only on pinned classes with `kotlin.Metadata` kept; then property/parameter names are original but functions/classes are renamed. Ambiguous cases (spill fields, when-mapping indices, lambda indices) are finite N sets. |

**Cross-source evidence tiers** (from the kxs study, **corrected by the R8 audit**):
- **Tier A (compiler-emitted strings).** These were proposed as S-grade when structurally
  bound, but R8's inlining and constant folding can move a string into a different method or
  fuse it into a larger template (audit E10, E11). **They are hints until a binding is proven to
  survive inlining** (open problem: e.g. evidence that the method wasn't an inline target, or
  agreement between independent sources). Candidates: data-class `toString`
  templates, `Intrinsics.checkNotNullParameter` names, `throwUninitializedPropertyAccessException`
  names, enum `<clinit>` names, Moshi codegen `missingProperty` names and adapter `toString`, names
  kept by library consumer rules.
- **Tier B (wire-format names, hint-grade):** serial names, JSON names, Gson `@SerializedName`.
- **Tier C (library templates):** identify runtime/library code, not app names.

**Fixture prerequisite:** Kotlin sources need a pinned kotlinc and plugin jars in
`cargo xtask fixtures`, the way R8 is pinned. That's the next infrastructure task after the
DEX writer.

### 1.3 Scope
What 8R is *not*: it's not a decompiler. 8R emits DEX (plus a mapping and a report). You then
feed that to jadx, a baksmali-style dumper, or anything else. This keeps scope tight, and it
means the thing we test is bytecode, which is testable.

## 2. Inputs and preflight

- **APK / AAB / `.dex` / directory of dex files.** Multidex (`classes*.dex`) is handled, and so
  is the DEX v41 container format.
- **R8 marker.** R8 writes a string into the dex string pool, e.g.
  `~~R8{"backend":"dex","compilation-mode":"release","min-api":24,"pg-map-id":"…","r8-mode":"full","version":"8.x.y"}`.
  8R parses it to learn the R8 version, full vs. compat mode, min-api, and the map id. It also
  handles `~~D8{…}` (nothing to undo except desugaring) and `~~L8{…}` (desugared library).
- **No mapping file.** 8R never takes one. (The `pg-map-id` in the marker is still reported,
  since it identifies the build.)
- **Optional extras:** `resources.arsc` + `AndroidManifest.xml` (entry points, resource-id
  re-symbolization), and a library signature DB (§5.4).

## 3. Architecture

Cargo workspace:

```
crates/
  eightr-dex        DEX read + write (format 035–041). Zero-copy reader, canonical writer.
  eightr-mapping    R8/ProGuard mapping parser + printer, incl. JSON metadata (v1.0–2.2+)
  eightr-apk        zip/APK/AAB container IO, manifest + arsc (binary XML) parsing
  eightr-ir         register IR: CFG, def-use, type inference (verifier-grade), dominators
  eightr-rules      rule registry (§0.4): ids, S/D class, preconditions, fixtures
  eightr-evidence   evidence collection: mapping, marker, Kotlin metadata, strings, DB matches
  eightr-unpass     the un-passes (§4), each a separate module
  eightr-naming     name resolution from evidence + stable structural fallback names (§5)
  eightr-sigdb      library fingerprint DB: builder + matcher
  eightr-report     provenance/confidence report (JSON + human-readable)
  eightr-cli        `8r` binary
  xtask             fixture generation (drives javac/kotlinc/D8/R8), DB builds, CI helpers
```

### Pipeline

```
 read ──► preflight ──► lift to IR ──► collect Evidence (read-only)
                                            │
                                            ▼
       ┌──────── structural un-passes (roughly the reverse of R8's pipeline) ────────┐
       │ 1 restore debug/line info       6 un-merge classes (horiz., vert.)           │
       │ 2 inline back outlines          7 re-box enums                               │
       │ 3 inline back API-model/backport 8 un-inline (frame-guided)                  │
       │ 4 restore residual signatures   9 re-sugar lambdas / $-CC / nest bridges      │
       │ 5 un-staticize                 10 tighten access                              │
       └───────────────────────────────────────────────────────────────────────────────┘
                                            │
                                            ▼
                     naming (applied LAST, over the final class/member set)
                                            │
                                            ▼
                     verify (dex verifier rules) ──► lower ──► write dex' + mapping + report
```

Why naming comes last: structural passes create and split classes and methods. Naming them all
at the end, from accumulated evidence, gives one consistent resolution step. Names are still
*available* as evidence throughout. For example, un-merge uses the mapping's original owners.

### Core types (sketch)

```rust
pub struct Program { classes: BTreeMap<ClassId, Class>, /* arena-backed */ }

pub enum Source { Mapping, KotlinMetadata, StringEvidence, SigDb, Structural }

pub struct Fact<T> { value: T, source: Source, confidence: Confidence, why: ProvenanceId }

pub trait UnPass {
    const NAME: &'static str;
    fn applies(&self, ev: &Evidence, cfg: &Config) -> bool;
    /// Must preserve semantics. Must be deterministic. Returns what it did and what it refused.
    fn run(&self, prog: &mut Program, ev: &Evidence, rep: &mut ReportSink) -> Result<()>;
}
```

Every un-pass is **all-or-nothing per unit** (a method or a class). If a precondition fails
halfway through, that unit is left untouched and the refusal is reported. 8R never emits a
half-transformed method.

### Determinism rules (enforced, not just aspirational)

- `clippy.toml` `disallowed-types`: `std::collections::{HashMap, HashSet}` are banned in every
  crate except behind an explicit `#[allow]` with a comment. Use `BTreeMap`, or `IndexMap`
  with insertion order derived from sorted input.
- Parallelism (`rayon`) is only used as *map, then sort by stable key, then reduce*. No
  shared mutable state is touched in parallel.
- All iteration over a program is in **canonical order**: S names first, then structural hash.
  It is never by obfuscated name or input position, because those would violate α-invariance
  (§0.3).
- Tie-breaking in every heuristic is explicit and documented: (class S before D, source rank,
  structural hash). A tie that survives all of those is an N-problem. The rule must refuse
  (identity), not pick.
- `eightr-ir` never exposes obfuscated names to un-pass logic except through the mapping lookup
  API. Passes see opaque ids plus structure. This makes α-invariance hold by construction,
  not just by testing.
- The DEX writer is canonical. The same `Program` always produces the same bytes (sorted
  pools, deterministic layout, recomputed checksum/signature).
- No timestamps, paths, or environment in output. The report includes the 8R version and
  input hashes only.

## 4. Un-passes (key details)

> Parts of this section (4.1, frame-guided un-inlining in 4.6, vertical un-merging in 4.4)
> were written for a mapping-supplied mode, which no longer exists. The mapping parser (4.1)
> remains as test infrastructure: it's the grading oracle. The no-mapping versions of these
> passes are being redesigned from the source research in `docs/sources/`.

### 4.1 Mapping parser (`eightr-mapping`)

- Handles class lines, field lines, method lines with `startLine:endLine:` minified ranges and
  `:origStart[:origEnd]` original ranges, and qualified original owners (`com.a.B.m() -> x`,
  from inlining and merging).
- JSON metadata comments: `com.android.tools.r8.mapping` (version header), `sourceFile`,
  `com.android.tools.r8.synthesized`, `com.android.tools.r8.residualsignature`,
  `com.android.tools.r8.rewriteFrame`, `com.android.tools.r8.outline`,
  `com.android.tools.r8.outlineCallsite`. Unknown ids are preserved and produce a warning, not
  an error.
- It's lossless: `print(parse(m)) == m` for canonical inputs. This is what makes it
  fuzz-testable.
- Retrace falls out almost for free: `8r retrace stacktrace.txt --mapping m`. That's a useful
  secondary product and a good test oracle against R8's own `retrace`.

### 4.2 Line / debug info

8R decodes `debug_info_item`s, including R8's pc-based encoding, where the "line" is the
instruction offset and debug info is stripped for min-api ≥ 26. It uses the mapping to rebuild
per-instruction `(original method, original line)` stacks. Those **inline frame stacks** drive
un-inlining (§4.6).

### 4.3 Outlines

R8's outliner hoists common instruction sequences into static methods of synthetic classes;
R8 ≥ 9 also outlines throw blocks bottom-up. With `outlineCallsite` metadata, each call site
maps precisely back to original positions. Inlining an outline back is code motion; it
preserves semantics when the holder chain has no `<clinit>`, the holder has no program
subclasses, the body's references are accessible from the caller, and registers are
re-allocated correctly. **Implemented (Phase 1)** without metadata and **never by name**
(α-invariance): an `ACC_SYNTHETIC` holder, a static straight-line method reached only by
`invoke-static` from ≥ 2 sites, and a per-kind body shape (classic: ≥ 3 operations incl. a
call, library classes only; throw: builds and throws an exception). Details, look-alikes,
register allocation and validation: `docs/sources/r8-desugar.md` §4.5.

### 4.4 Class un-merging

- **Horizontal.** The merged class has a `$r8$classId` int field. Constructors take/assign it.
  Merged virtual methods dispatch via a `switch` on it. 8R splits it into N classes: each
  constructor call site's classId constant tells us which class is being instantiated, and each
  switch arm becomes that class's method. Precondition: every `classId` read is a
  dispatch-switch or pass-through. Otherwise 8R refuses.
- **Vertical.** Members whose mapping owner is `Sub` move back to a new `Sub extends Super`.
  `new Super` sites that the mapping attributes to `Sub.<init>` become `new Sub`.

### 4.5 Enum re-boxing

Unboxed enums are `int` ordinals, plus utility synthetics for `ordinal`/`values`/`valueOf`/
`compareTo`/`toString`. The residual signature (or type inference, plus the utility calls)
identifies which `int`s are really enum values. 8R rebuilds the enum class. Constant names come
from the mapping, or else from the string table used by the unboxed `toString`/`valueOf`.
Precondition: every flow of that int is enum-typed. Arithmetic on it means 8R refuses.

### 4.6 Un-inlining (the hard one)

This requires a mapping (inline frames). For each method, 8R works in three steps:

1. **Group instructions into regions.** Each region shares an inline-frame prefix `[callee @
   line, caller @ line, …]`.
2. **Check each region's preconditions.** Single entry. Exits only to one join point (the
   "return"). No exception edges escaping to handlers that aren't in the original caller's
   frame. Live-in and live-out sets are computable, with at most one live-out value.
3. **Extract the region** into a method whose name and owner come from the frame. Its
   parameters are the live-ins, ordered per the original signature when the mapping provides
   one. Its return value is the live-out.

When the same original method is recovered at several sites:
- If all copies are α-equivalent after normalization, 8R emits **one** method and calls it from
  every site.
- Otherwise the copies are specialized (constant args got folded in). 8R emits the **most
  general** copy if one subsumes the others. If none does, it emits per-site copies named
  `m_{hash}`, where the hash is the copy's own structural hash (not a site counter, which would
  be order-dependent). The report flags them as specialized.

Every un-inlined method is labelled **D**, never S (§1): 8R can't rule out R8 having optimized
across the inlining boundary.

Nested frames are handled innermost-first, iterating to a fixpoint.

### 4.7 Re-sugaring

This is opt-in per feature (`--resugar lambdas,backports,interfaces,nest`). Re-sugaring can
raise the output's min-api, so it's reported. Lambdas can instead become named inner classes,
which keeps min-api. That's the default.

### 4.8 Access tightening

After all structural passes, 8R computes the minimal access modifier that still verifies,
given every reference in the program plus manifest/reflection-kept entry points. It never
tightens anything that's kept, or that's reachable via reflection strings. This pass is
conservative, and `--no-tighten` turns it off.

## 5. Naming without a mapping

Name evidence sources, highest priority first. Each is marked **[S]** (proof: the output is
a clean original name) or **[hint]** (it contributes only the `{hint}` of a D name
`{hint}_{hash}`).

1. **[S] Kept names** (`r8/kept-name`). Names R8's generator cannot produce: see the rule's
   preconditions in `crates/eightr-rules`. Also planned: library-override names, annotation
   member names, manifest components, JNI symbols exported from `.so` files. Note: `native`
   methods **are** renamed by R8 unless a keep rule (AGP's default file has one) prevents it. This assumes the default minifier
   dictionary. If the names look like a custom `-obfuscationdictionary` was used, kept-name
   detection is limited to manifest, layout, and JNI references.
2. **[hint] Serialization descriptors.** kotlinx.serialization `PluginGeneratedSerialDescriptor("com.x.User", …)`
   + `addElement("name")` gives the original class FQN and property names. Gson
   `@SerializedName` is also strong but not definitive. Either can be overridden by an
   annotation, so this isn't proof.
3. **[S] Kotlin null-check strings.** `Intrinsics.checkNotNullParameter(p, "userId")` gives
   parameter names. `checkNotNullExpressionValue(v, "getFoo(...)")` names the called method
   (**[S]** for the callee only when the string resolves to a unique method).
4. **[S] Generated `toString`.** For a verified compiler-generated shape only; a hand-written
   `toString` is a hint. Data class `"User(name=" … ", age="` gives the class name and
   field names, ordered. The same goes for records (`ObjectMethods` desugaring).
5. **[S] Enum `<clinit>` strings.** Constant names always survive as strings.
6. **[structure] Kotlin metadata.** When R8 keeps it, R8 has *rewritten* it to the minified names, so it
   supplies structure rather than names: property↔getter/setter/backing-field links,
   `suspend`, data class, and companion. That structure lets a single recovered name propagate
   (`name` field → `getName`/`setName`).
7. **[hint] Log tags and exception messages.** `Log.d("PaymentRepo", …)` is a weak class-name hint.
8. **[hint] Library signature DB (§5.4).** Identifies third-party code wholesale.
9. **[D] Structural fallback.** A stable, readable generated name (§5.5).

Propagation: names flow through overrides (a method overriding a named method gets that name),
through property accessors, through the `this$0`/outer links of inner classes, and through
field-to-constructor-parameter assignments. Propagation runs to a fixpoint. An S name
propagates as S only along edges that are themselves proofs, such as overrides and Kotlin
metadata accessor links. Along a heuristic edge like a constructor assignment, it becomes a
hint. If two S facts conflict, that's a bug, so 8R fails loudly. Conflicting hints are
resolved by the tie-break rule. A tie that survives means no hint, and the name falls back to
role only.

### 5.4 Library signature DB

- **Build (offline, `xtask sigdb`).** Take the Maven artifacts of popular libraries (AndroidX,
  Kotlin stdlib/coroutines, OkHttp, Retrofit, Gson, Moshi, Guava, Glide, Room, Compose, …) at
  many versions. Compute **name-independent** fingerprints per method: normalized opcode
  sequence, CFG shape, constant strings, and the arity/types of API calls into the Android
  framework, whose names R8 never changes.
- **Match.** Seed high-confidence unique matches: methods with distinctive strings, or
  framework-call fingerprints. Then propagate over the call graph and class hierarchy, the
  same way BinDiff/Diaphora do. R8 optimizes library code too, so matching is fuzzy
  (similarity over normalized features), with deterministic thresholds.
- **Scope.** Versioned DB files, with the DB hash recorded in the report. It's a later
  milestone.

### 5.5 Stable structural fallback names

Naming unknowns with counters (`Class1`, `Class2`, …) is **N**: the numbering depends on
input order. It also causes cascade renames on every app update. Every D name is
`{Hint}_{hash}` instead. The hash suffix is mandatory even when the hint is strong, because
it's how a reader knows the name is not proven original (§0.2.4):

- **Hint** comes from the best [hint] evidence (`UserRepository_3fa9`), or else from role. It's the supertype or interface (`Activity_…`, `Runnable_…`,
  `ViewModel_…`), or `Obj`. Members use their type (`str_…`, `list_…`) or `m_…`.
- **Hash** is the class's **Weisfeiler–Lehman structural hash**, computed over the class graph
  with minified names erased, a few rounds deep. It's truncated to the shortest length that's
  unique in the app (minimum 4 hex chars), and collisions are resolved canonically.

Result: an unrelated edit elsewhere in the app leaves the class's name unchanged. This is
tested (§6.5).

## 6. Testing

### 6.1 The oracle: we can make ground truth

This is the key to the whole test strategy. For any source program `P`:

```
javac/kotlinc(P) ─► D8 --debug (no shrinking)                 = G   (ground truth)
                 └► R8 --release + keep rules  → O, mapping M = obfuscated
8R(O, M) = U       8R(O) = U₀
```

Every test compares `U` or `U₀` against `G`, `O`, and `M`. The checks follow the rule classes:

- **S exactness (hard gate, both modes):** Every attribute the report labels **S** must equal
  the oracle exactly.
  - For names, the oracle is `M`. In no-mapping mode 8R never saw `M`, but the test harness
    holds it back and checks against it.
  - For structure and bodies, the oracle is `G`, compared under a normal form (SSA + canonical
    register allocation + canonical block order). Where R8 exposes a switch for a single
    optimization, a second reference `G_T` is built with only that pass `T` disabled. It's the
    tightest oracle for "T was undone".

  **One wrong S label fails CI.** S precision is 100% by definition. Mapping mode is also
  expected to reach 100% S coverage on names (every name S).
- **Semantic equivalence:** `U` must behave like `O` (and therefore like `G`). The test runs
  each fixture's `main`/test entry points on ART and diffs stdout, exit code, and exceptions.
  For line mapping, it diffs `retrace(O's stack traces)` against U's stack traces.
- **Structural similarity:** A score for `U` vs `G`: % of G's methods present in U by
  name+signature, and % whose normalized CFGs are isomorphic. This is a **ratcheting
  metric**, stored per fixture in `tests/metrics.toml`. CI fails if it decreases. Improvements
  are committed.
- **D quality (metrics, ratcheted):**
  - **S coverage:** % of attributes labelled S, per mode.
  - **Hint accuracy:** % of D names whose `{hint}` part matches the original simple name.
  - **Structural similarity** (above).

  None of these can go down. Hints are allowed to be wrong, which is why they carry a hash
  suffix, but hint accuracy is tracked so bad hint sources get demoted.

Fixtures are generated by `cargo xtask fixtures` using **pinned R8 versions** (a matrix of
several recent R8 releases, downloaded from Google Maven, with hashes checked). Compiled
outputs (`G.dex`, `O.dex`, `M.txt`, `expected/`) are **checked in**, so the unit and golden
tiers need no JDK. Only fixture regeneration and the ART tier do.

### 6.2 Test tiers

| Tier | What | Runs |
|---|---|---|
| T1 unit | parsers, IR, individual analyses, writer | every commit, < 30 s |
| T2 golden | checked-in fixtures; `insta` snapshots of a canonical smali-like dump of `U`; names vs G | every commit |
| T3 property | determinism, idempotence, round-trips (`proptest`) | every commit |
| T4 fuzz | `cargo-fuzz` on dex reader, mapping parser, binary XML | nightly + OSS-Fuzz later |
| T5 semantic | execute G/O/U on ART (host `art`/`dalvikvm` build or emulator), diff behavior | nightly |
| T6 real-world | open-source apps built from source with R8 (we get the mapping), scored with metrics | nightly, dashboard |
| T7 perf | 8R on a ~100k-method app: time + peak RSS budgets | nightly |

### 6.3 Fixture catalog (T2/T5)

Each fixture is a tiny program targeting **one** R8 behavior, with keep rules chosen to force
it. Every fixture runs in both modes (with and without the mapping) across the R8 version matrix.

**Naming**
- `names/basic`: class, field, and method renaming. Also private/static/instance variants.
- `names/overload`: Java overloads that R8 renames to the same or different names.
- `names/overload-aggressive`: `-overloadaggressively`, same name with different return types.
- `names/repackage`: `-repackageclasses ''` and `-flattenpackagehierarchy`.
- `names/inner-classes`: inner, anonymous, and local classes, and the `InnerClasses`/`EnclosingMethod` attributes.
- `names/interfaces-override`: name propagation through a deep override hierarchy.
- `names/kept-mixed`: partially kept classes. Kept names must be untouched.
- `names/reflection`: `Class.forName("…")` strings for a kept class. Must stay identical.
- `names/jni`: `native` methods keep their names.

**Lines & source files**
- `lines/basic`: exceptions thrown at known lines. U's stack trace must equal `retrace(O)`.
- `lines/pc-encoding`: min-api 26+, pc-based lines. Same assertion.
- `lines/sourcefile`: `r8-map-id-…` SourceFile gets restored to `Foo.java`/`Foo.kt`.
- `lines/rewrite-frame`: null-check `rewriteFrame` metadata.

**Inlining**
- `inline/single-caller`: a private method with one caller gets inlined and removed. U must
  contain it again.
- `inline/multi-site-identical`: inlined at 3 sites with no specialization. Expect one
  recovered method.
- `inline/multi-site-specialized`: constant args folded differently per site. Expect per-site
  copies, flagged.
- `inline/nested`: A→B→C, all inlined. Expect three methods back.
- `inline/with-try`: an inlined body inside a try/catch in the caller.
- `inline/multi-return`: a callee with several returns.
- `inline/refuse`: code motion that makes the region non-contiguous. Must be **refused**,
  reported, and still semantically equal.

**Outlining**
- `outline/basic` (**done**: `r94_outline`): a repeated StringBuilder sequence at ≥ 20 sites
  (R8's threshold; the fixture uses 24–26) and repeated throw blocks get outlined. Expect them
  inlined back, and the outline class gone. There's no separate "no-metadata" variant: the
  mapping is never an input.
- `outline/look-alikes` (**done**: `r94_desugar`): backports, `$-CC`, API-model outlines at a low
  min-api. Expect them left alone.

**Synthetics / desugaring**
- `synth/lambda-java`, `synth/lambda-kotlin`, `synth/method-ref`, `synth/capturing-lambda`.
- `synth/backport`: `Math.floorMod`, `Objects.requireNonNull`, `List.of` at min-api 21.
- `synth/default-interface`: default and static interface methods at min-api 21 (`$-CC`).
- `synth/nest-access`: private access across nestmates.
- `synth/records`: `toString`/`equals`/`hashCode` on records.
- `synth/twr`: try-with-resources, `$closeResource`.
- `synth/api-model-outline`: new-API calls wrapped for older min-api.
- `synth/string-switch`: switch on strings.

**Class structure**
- `merge/horizontal`: two small same-shape classes get merged (`$r8$classId`). Expect them split.
- `merge/vertical`: a single-subclass hierarchy gets merged. Expect it split.
- `merge/refuse`: classId used in non-dispatch arithmetic. Must be refused.
- `enum/unbox-basic`, `enum/unbox-switch`, `enum/unbox-valueOf`, `enum/refuse-arith`.
- `sig/unused-arg`, `sig/arg-reorder`, `sig/return-removed`, `sig/staticized`: covers
  `residualsignature` restoration.
- `access/tighten`: `-allowaccessmodification`. Access must be restored to the original, or
  to something no wider.

**No-mapping inference**
- `infer/kotlin-intrinsics`: parameter names from `checkNotNullParameter`.
- `infer/data-class-tostring`: class and field names from `toString`.
- `infer/kotlinx-serialization`: FQN and properties from the descriptor.
- `infer/enum-names`.
- `infer/property-propagation`: one recovered field name propagates to its getter and setter.
- `infer/log-tags`: a weak hint. It must not override stronger evidence.
- `infer/conflict`: two sources disagree. Resolution must be the documented tie-break.
- `infer/sigdb-okhttp`: a small app bundling OkHttp gets its library classes identified.

**N-refusal (each must produce identity or D, never a guess, and must pass α-invariance)**
- `nrefuse/automorphic-classes`: two structurally identical, interchangeable classes. The
  output must be byte-identical under swapping their obfuscated names.
- `nrefuse/wl-hard`: classes that WL refinement can't separate but that aren't automorphic.
  This exercises the canonical-labeling fallback.
- `nrefuse/tied-hints`: two equally strong conflicting log-tag hints. The name must drop to a
  role-only hint.
- `nrefuse/vertical-merge-nomap`, `nrefuse/staticized-nomap`, `nrefuse/inline-nomap`: must stay
  identity (D·id), reported.
- `nrefuse/custom-dictionary`: R8 with `-obfuscationdictionary` of real-looking words. Kept-name
  detection must not claim S for dictionary words.
- `nrefuse/fake-tostring`: a hand-written `toString` that looks like a data class but lies. It
  must not be taken as S.

**Preflight / robustness**
- `preflight/d8-only`: D8 output. 8R must be ~identity (only re-sugaring, if requested).
- `preflight/mismatched-mapping`: a mapping from another build must produce a hard error with
  a clear message.
- `preflight/truncated-mapping`: parse error with line/column. No partial output.
- `preflight/no-marker`: stripped marker. 8R falls back to heuristic detection and warns.
- `preflight/multidex`: cross-dex references and merged output.
- `preflight/l8-desugared-lib`: `j$.` desugared library classes must be recognized and mapped
  back to `java.`.
- `preflight/dex-container-v41`.

### 6.4 Property tests (T3)

- **Determinism:** `8R(x)` byte-equals `8R(x)` across `RAYON_NUM_THREADS ∈ {1, 2, 8, 64}`, with
  the dex file order shuffled inside the APK, with zip entry order shuffled, and under
  different `HashMap` seeds (a canary: if anything leaks a `RandomState`, it shows up here).
- **α-invariance (the anti-N test, §0.3):** For each fixture `O`, generate random `O'` by:
  - consistently renaming every minified identifier with a fresh random scheme, and rewriting
    the mapping to match in mapping mode;
  - shuffling class, member, string-pool, and dex-file order;
  - re-splitting multidex.

  Then `8R(O')` must be **byte-identical** to `8R(O)`. The randomness lives only in the test;
  the pipeline must erase it. Any failure is an N-leak and gets bisected to the rule id via the
  report.
- **Label monotonicity:** Adding evidence (supplying the mapping, or a sigdb) may only turn D
  into S, never S into D, and never change an S value.
- **Idempotence:** `8R(8R(x)) == 8R(x)`, for every fixture. D names with hash suffixes must be
  recognized as already-canonical, not re-hashed.
- **Round-trips:** `write(read(dex))` is semantically identical, and byte-identical if the
  input is canonical. `print(parse(mapping)) == mapping`.
- **Retrace agreement:** for random stack traces synthesized over O, `8r retrace` equals R8's
  `retrace` tool.
- **Verifier:** every output dex passes an ART-equivalent structural verifier, our own plus
  `dexdump -d` as a sanity check.
- **Generative:** a random program generator (a small typed Java subset, emitted as source) is
  run through javac → R8 → 8R, checking semantic equivalence and name exactness in mapping
  mode. This finds the combinations the hand-written fixtures miss.

### 6.4.1 Rule-registry checks (T1)

- Every registered rule has ≥ 1 fixture, and every fixture exercises its S path *and* its
  fallback path (by withholding the evidence).
- Every report entry references a registered rule id. An un-pass that transforms without
  emitting a rule application fails the test harness.
- The rulebook table in §1 is generated from the registry (`cargo xtask rulebook`), so the
  doc can't drift from the code.

### 6.5 Stability tests

The same app is built at two versions with a small source diff (from a git history of a real
open-source app, or a scripted mutation). 8R runs without the mapping on both. The test
measures **name churn**: the fraction of classes that are unchanged in the source but whose
8R-generated name changed. The target is ≈ 0, tracked as a ratcheted metric.

## 7. CLI

Implemented today:

```
8r info  app.apk            # dex files, integrity, D8/R8 markers, detected sources, S/D/N summary
8r undo  app.apk [--report out.json] [--verbose]   # full pipeline; JSON report (DEX output: M1)
8r rules                    # the rule registry
```

Planned: `8r undo -o out/` writing `classes*.dex` + `8r-mapping.txt` (obfuscated → recovered,
consumable by jadx) + `report.json`; `--resugar …`; `--no-tighten`; `8r diff a.dex b.dex`.

## 8. Milestones

- **M0 Foundations.** *(Mostly done: see "Status" below.)* Rule registry + report plumbing, and the α-invariance harness (with only
  the identity rule, which must pass it trivially). DEX reader, mapping parser (lossless), marker parsing, `8r info`,
  `8r retrace`. Fuzzing set up. Fixture xtask with a pinned R8 matrix.
- **M1 Names.** Canonical DEX writer. Rename-only undo with mapping (exact). Lines and source
  files. No-mapping inference sources 1–7 plus structural fallback names. `--emit
  mapping-only`. Determinism/idempotence property tests green.
- **M2 Synthetics.** Outlines (**done**, Phase 1), API-model outlines, backports, `$-CC`, nest bridges, lambdas
  (as inner classes, and optionally re-sugared).
- **M3 Signatures & inlining.** Residual signatures, un-staticize, frame-guided un-inlining.
  ART semantic tier live.
- **M4 Structure.** Class un-merging, enum re-boxing, access tightening.
- **M5 SigDB.** Library fingerprint DB and matcher, and the real-world corpus dashboard.

## 9. Open questions

1. **Output target.** Is DEX enough, or do we also want JVM `.class` output (for Java
   decompilers like CFR/Vineflower)? That's a whole second backend. I'd defer it.
2. **Re-sugar defaults.** Should lambdas become inner classes (min-api safe) or
   `invoke-custom` (closer to the original source)? The current proposal is inner classes by
   default.
3. **R8 version matrix.** How far back do we support? The mapping format is only versioned
   from ~2.0. Older R8 and ProGuard maps are "names only".
4. **Resource re-symbolization** (int constants → `R.layout.foo`, using `resources.arsc`).
   It's cheap and high value for readability, but it belongs as annotations in the report or
   in a decompiler plugin, not in the DEX.
5. **Faithfulness vs. min-api.** Some S results raise the output's min-api, such as re-sugaring
   lambdas to `invoke-custom` or restoring backported calls. Should 8R prefer S by default
   (the original form) or D (the output still runs on the original min-api)? I lean toward S
   by default, since output is for reading, with `--keep-min-api` to force D.
6. **D-name marker format.** Is `Name_3fa9` fine, or do we want something that can't collide
   with real code, like `Name$8r3fa9`? The marker must be unambiguous for invariant 0.2.4 and
   for idempotence.

7. ~~Probabilistic proof~~ **Decided:** cryptographic fingerprints of ≥ 40 bits (e.g. the Kotlin
   value-class MD5 mangling) count as S when they confirm exactly one candidate from an
   exhaustive list, with the collision bound (≈ n·2⁻⁴⁰) stated in the report. 32-bit
   non-cryptographic hashes (Compose group keys, `String.hashCode`) only confirm D names.
8. ~~Template S~~ **Decided:** hand-writable templates (data-class `toString`) need an ensemble of
   independent agreeing templates for S (kotlinc.md §5.3); a single match is D.
9. ~~Re-synthesizing metadata~~ **Decided:** yes. 8R will emit `@kotlin.Metadata` (D) rebuilt from
   recovered facts, so Kotlin-aware decompilers show properties, data classes, suspend functions.

## Status (M0)

Implemented and tested (`cargo test`: dex, mapping, rules, core):

- `eightr-dex`: zero-copy DEX reader (035–040; 041 rejected until tested). Header, map,
  id tables, class defs/data, code items + try/catch, **full opcode table + decoder**
  (all formats and payloads), debug-info state machine, encoded values/annotations, checksum
  and SHA-1. Tests: unit vectors from the spec; a **differential test against AOSP
  `dexdump -d`** on every fixture (classes, members, code headers, the mnemonic at every pc,
  catch tables, positions); deterministic mutation testing (no panics, overflow checks on).
- `eightr-mapping`: lossless R8 mapping parser/printer (byte-identical round-trip on R8
  8.10 output), lenient JSON for the spec's metadata examples, inline-frame lookup. Used as
  the test oracle.
- `eightr-rules`: the registry, with `Source` (undo order), `Class` (N < D < S), and
  `Attribute`; tests enforce sorted ids, source prefixes, fixture existence, and fallback
  sanity.
- `eightr-core`: input loading (dex/APK/AAB/dir, canonical multidex order), marker
  parsing, source detection, program model, labels (composition by minimum; N must carry
  candidates), passes in undo order, JSON report. First real rule: `r8/kept-name` (S).
  Oracle tests grade every S label against R8's held-back mapping and against the D8 build;
  report determinism under input reordering; an S-coverage ratchet (`fixtures/metrics.json`).
- `xtask fixtures`: javac → D8 (ground truth) / R8 (+mapping) / dexdump for each fixture,
  with tool versions recorded in `BUILD.txt`.
- `eightr-ir`: semantic ops, CFG with exceptional edges, dominators, verifier-style type
  inference, reaching definitions; the whole-program **model** (`model::Program`: classes,
  members, code, annotations, static values, retained marker strings) and a consistent
  **renamer** (`rename::Renaming`: descriptors, protos, member refs, generic signatures,
  `InnerClass` names).
- `eightr-dexwrite`: canonical DEX writer (sorted pools, branch relaxation, payloads, debug
  info, annotations, multidex split). Tests: exact round trip of every fixture, byte
  idempotence, independence from model order, and acceptance by AOSP `dexdump` and D8.
- **Registered rules (step 4):** `r8/kept-name`, `r8/annotation-member-name`,
  `r8/library-override-name` (all S, vouching for current names), and
  `kotlinc/lateinit-field-name` (S, *recovers* renamed backing-field names from kotlinc's
  uninitialized-access message, bound through the field reference, so inlining can't
  mis-attribute it). The oracle grades recovered values against the mapping, translating
  residual types in signatures back to original types.
- **Kotlin fixtures (step 5):** pinned kotlinc 2.4.20 + plugins and runtime libraries
  (`fixtures/toolchain.conf`, SHA-256 verified), consumer rules passed as AGP does.
- **Naming + output (step 6):** `core/structural-name` (D) gives every unproven name a
  structural `{hint}_{hash}` (Weisfeiler–Lehman over the program with non-S names erased,
  including incoming and position-sensitive usage; members also by where they're used).
  Renaming is behavior-preserving: fields, static/private methods always; virtual methods
  only when no member of the override group can override a platform method (checked against
  an embedded android.jar table, `cargo xtask platform-api`). Structurally indistinguishable
  items are `core/structural-tie` (N, full candidate set). `8r undo app.apk -o out/` writes
  `classes*.dex`, `8r-mapping.txt` (ProGuard format, recovered -> current; load in jadx with
  `--mappings-path out/8r-mapping.txt -Prename-mappings.format=PROGUARD_FILE
  -Prename-mappings.invert=yes`), and `report.json`. Tests: inverse renaming restores the input
  exactly, overrides preserved and none created, unique field names, mapping matches both
  sides, D8 accepts the output, 8R is idempotent, jadx shows recovered names, and the α test
  requires the **emitted dex to be byte-identical** under scrambling.
- **Body rewrites (Phases 0.3–1):** an execution-equivalence harness (`tests/exec.rs`: JVM via
  dex2jar, ART on an emulator, plus whole-program ART verification with `dex2oat64
  --compiler-filter=verify`, which catches register/wide-pair bugs no `main` reaches); IR edit
  utilities (`eightr_ir::{edit, inline, liveness, refs}`: splicing, unreachable-code removal,
  static-call inlining into dead registers or a register gap); a rewrite phase in the pipeline;
  and the first rewrites, `r8/outline-inline` and `r8/bu-outline-inline` (D). Tests: outline
  detection matches every fixture mapping exactly (precision and recall), every program
  reference in the output resolves (`tests/output.rs`), ART verifies every output class. On a
  real app (Gretio): 132 outlines, 12441 of 12567 call sites inlined, all classes verify. The
  renamer now parses generic signatures properly (`types::signature_class_ranges`; it used to
  miss class types after primitives, e.g. `(ILa/B<…>;)V`).
- **Merged classes (Phase 2):** `r8/split-merged-class` (D, with S facts) splits classes R8's
  horizontal merger combined (merged siblings, lambda groups) into an abstract base plus one
  final subclass per class id; each overrides the id-dispatching methods with its own arm
  (`edit::fold_constant_branches`). Detection: a synthetic final byte/short/int id stored in
  the constructors' entry block, read only as a dispatch key, constant at every instantiation;
  refused on identity observations (class literals, annotations, handles, used `getClass()` on
  values syntactically typed as the class), Serializable, program subclasses. Moved arms keep
  access (private helpers widened, `invoke-super`/foreign-protected arms stay in the base).
  Oracle: 58 splits, no false ones against `$r8$classId`; recall ≥ 85%. Gretio: 1274 classes
  split into 7553, all classes verify.
- **Enum unboxing (Phases 3A/3B):** `r8/enum-unboxing-utility` (S) names the shared utility's
  `$VALUES`/`ordinal`/`values` by exact fingerprints. `report.enums` recovers enums R8
  removed: constant names are *proven* only by an inlined `valueOf` (literal → the value it
  yields) with its own failure message (the canonical name), or by an equals map agreeing with a
  `name()` chain; `name()` chains alone are unproven (a String field's per-value strings look
  identical); string switches prove nothing. `r8/rebox-enum` (D) re-creates proven enums in
  javac's shape and converts webs of their values (seeded by name chains and `valueOf` results,
  closed within a method over copies and merges, sourced only from constants and `values(N)`)
  back to objects; other int uses read the exact adapter `$8r$unboxed(e)` = e == null ? 0 :
  e.ordinal() + 1. Fixtures `r94_enum`, `r94_enum_rebox` run identically on ART. Gretio:
  `SVG$Unit`, `SVG$GradientSpread`, `StorageHelper$TokenType` (+1 unnamed) re-boxed.
- **Inlining (Phase 4, D·id, report only):** nothing is un-inlined. `report.inlining` counts
  hints (`eightr_core::inline_hints`; the per-hint list, with input method and pc, with verbose
  labels) and line tables carrying no original lines (compacted 1..n, or R8's pc encoding);
  methods with hints also get a build-time `@eightr.Inlined(...)` annotation with the counts,
  so decompilers show them. Every kind is graded against the fixture mappings' inline frames
  by a permanent harness (`tests/oracle.rs::inlining_hints_precision_recall`, ratcheted in
  `fixtures/inlining-metrics.json`, per kind and per R8 version, with the base rate: any
  instruction lies within two of inlined code ~52% of the time, so read precision as lift):
  `inlined-instance-call` (a discarded `getClass()` whose receiver the next instructions use)
  0.87; `discarded-getclass` 0.78 (includes R8's rewritten null checks); `idiom:areEqual` (an
  `Object.equals` call: Kotlin's `a == b`) 0.85 precision / 0.87 recall;
  `idiom:collectionSizeOrDefault` 0.40 / 0.30 (weak: R8 usually leaves a bare `size()`);
  `param-null-check` (a method's own prologue argument checks) 0.30 — below the base rate, as
  expected: kept apart so it doesn't dilute the others. All hints together touch 3.4% of the
  fixtures' inlined occurrences (mapping ranges, not call sites): the baseline any future
  un-inlining work is measured from. Gretio: 13.4k discarded `getClass()` (plus 4.7k parameter
  null checks), 3.8k inlined instance calls, 1.5k `areEqual`; 92% of line tables pc-encoded.
- **Compose (M0–M4; docs/sources/compose.md "Implemented"):**
  - The Composer is found structurally.
  - Its members are named by the plugin's call shapes (`compose/runtime-api`, S, with body proofs; both compiler
    eras).
  - App composables' synthetic parameters: debug names, `@eightr.Composable` / `@eightr.RestartScope`, slot and
    default bindings, removed-parameter bounds (`compose/synthetic-params`, D).
  - ComposableSingletons `lambda$K` (D).
  - Library composables by durable group key (`compose/lib-key` S when a key unique to the function
    corroborates, `-hint` D), from a key DB built from 76 AARs (`cargo xtask compose-keys`).
  - Gretio: 552 restartables, 191 library composables (97 S), Compose 1.12 / material3 1.4.
- **Library signature DB and matcher (M3/M5, docs/research/sigdb.md):**
  - `cargo xtask sigdb` runs each version in `fixtures/sigdb.conf` through R8 alone.
  - It normalizes the result with 8R's rewrites, fingerprints it with names erased (stable = platform classes
    only), and keys it by original owner/name/proto (`sigdb/*.sigdb`, embedded).
  - `sigdb::matcher`: exact `all`/string hashes, class seeds, then call-graph, class-proto and mutual-best
    propagation, conflict-free per round.
  - `r8/sigdb-method-name` (D) names methods outside override groups, with `@eightr.Original`; voted classes
    get the library simple name as their structural hint.
  - On `sigdb_app` (same versions): 95% precise, 59% recall of library methods, constructors aside (`tests/sigdb.rs`, ratcheted; about the same across versions).
- **Library versions (M6):** `report.libraries` from `META-INF/*.version`, root `*.properties`, `name/x.y.z`
  strings, registrar id/version pairs, and the Compose key version sets. Gretio: 176 entries.
- **SDK extractors (M7):**
  - `kotlinc/data-class-name` (S): a data class's `toString` template, with `hashCode` and `equals`
    both over the same fields in declaration order. Recovered S class simple names are applied in their
    package; passes check each other's claims.
  - Property names: S only when kotlinc's `copy` survives (hand-written or IDE-generated
    `toString`/`hashCode`/`equals` look the same), otherwise D (`-property-hint`). A hand-written
    `PlatformTextStyle` with a typo showed why (M7 review).
  - Gretio: 128 classes, 400 D property names.
  - `r8/protobuf-message-name` (S): kept `*_FIELD_NUMBER` constants matched to the APK's bundled
    `.proto` sources (a unique match of ≥3 fields). The name must also read as kept on a re-run
    without the `.proto` files (idempotence), otherwise it's a hint. Gretio: 21 messages.
  - kotlinx.serialization descriptors: serial names give D hints for the class and its serializer;
    `report.serialization`. Gretio: 317.
  - Room entity FQNs from schema validation messages (`report.room_entities`). Gretio: 9.
- **Idempotence:** 8R on its own output is byte-identical, fixtures and Gretio alike.
  - Rewrites run to a fixpoint.
  - The kept-name bound counts only generator-shaped names.
  - The dex writer lays out annotations and static values canonically.
- **α-invariance test** (`crates/eightr-core/tests/alpha.rs`): for each R8 fixture, permutes
  the names R8 generated (known from the held-back mapping), rewrites and randomly splits the
  program across dex files, and requires the pipeline's output to be *equivariant*: every
  label, mapped back through the inverse renaming, is identical. Verified to fail on a
  deliberately name-dependent rule.
