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
| **Class/member renaming** | **S**. Mapping lines are exact. | **S** where proof-evidence exists (§5). Otherwise **D**: `{hint}_{hash}` names (§5.5). |
| **Repackaging** (`-repackageclasses`) | **S** | **D**: structural package clustering, packages named `p_{hash}`. Falls back to D·id (flat) if the clustering isn't α-invariant. |
| **Aggressive overloading** (same name, different return type) | **S** | **D**: split into distinct `_{hash}` names |
| **Line numbers** (compaction, pc-encoding) | **S** | D·id: pc-lines are stripped, not fabricated. ✗N: inventing plausible line numbers. |
| **Source file attribute** (`r8-map-id-…`) | **S** | **D**: `{OuterClass}_{hash}.kt`/`.java`, with the language from Kotlin metadata or intrinsics use. ✗N: guessing `FooKt` file facades. |
| **Inlining** | **D**: frame-guided extraction (§4.6). The body *may* equal the original, but 8R can't prove that R8 didn't optimize across the boundary after inlining, so it's labelled D. Specialized per-site copies are also D. | D·id. ✗N: splitting methods into guessed callees with no frame boundaries. |
| **Outlining** | **S**: `outlineCallsite` gives exact positions, and inlining back is pure code motion. | **D**: synthetic + static + leaf + shape match. It might inline a genuine synthetic helper, which is still readable and still correct. |
| **Lambda desugaring** | **S** when re-sugared to `invoke-custom` with the body method named from the mapping (min-api permitting). **D** when emitted as a named inner class (the default). | **D** |
| **Backports** | **S**: the synthetic body matches R8's backport template exactly *for the R8 version in the marker*, so the replaced API is uniquely determined. | **S** (same argument, since templates are version-keyed). Downgrade to **D** if the marker is missing. |
| **API-model outlines** | **S** | **S** (exact template shape, unique target) |
| **Interface `$-CC` companions** | **S** | **S** for structure (which interface owns them is unambiguous). Names follow the renaming row. |
| **Nest-access bridges** | **S** | **S** for structure. Names follow the renaming row. |
| **Horizontal class merging** (`$r8$classId`) | **S**: the partition comes from `classId`, and the names and owners come from the mapping. | **D**: the partition is structural (classId constants), names are `_{hash}`. |
| **Vertical class merging** | **S**: original owners come from the mapping. | D·id. ✗N: guessing where the subclass boundary was. |
| **Enum unboxing** | **S**: residual signatures plus utility synthetics | **D**: the class is re-boxed via the utility pattern. Constant names are **S** (from strings). The enum class name is D. |
| **Staticizing / devirtualization** | **S** (`residualsignature`) | D·id. ✗N: guessing that a static method was once an instance method. |
| **Argument reordering** | **S** | D·id. ✗N |
| **Unused-argument removal** | **D**: the signature is restored exactly and the body is S, but call sites pass a canonical `0`/`null`, because the original values are lost (and never mattered). | D·id. ✗N |
| **Return-value removal** | **D**: the signature is restored, but the returned value is lost. | D·id. ✗N |
| **Access modification** | **D**: tightened to the minimal access that verifies. Originals are never recorded. | **D** (same) |
| **Constant / member-value propagation** | D·id, annotated where detectable. ✗N: re-parameterizing guessed constants. | D·id |
| **Dead code / tree shaking** | D·id. The report lists mapping entries with no residual code. | D·id |
| **Class inlining / scalar replacement** | D·id, annotated from frames | D·id |
| **Local names, generics, stripped annotations** | D·id (S if R8 kept them) | D·id |
| **Kotlin intrinsics parameter names** | **S** | **S**: the `checkNotNullParameter(p, "name")` string is compiler-emitted and exact. |
| **Enum constant names** | **S** | **S**: the `<clinit>` name strings are exact. |
| **Kotlin data-class / record `toString`** | **S** | **S** for the simple class name and field names. The package is **D**. |
| **kotlinx.serialization descriptor names** | **S** | **D** (hint only). `@SerialName` can override it, so it isn't proof. |
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
evidence is never treated as absence of a source.

### 1.2 Scope
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

R8's outliner hoists common instruction sequences into static synthetic methods. With
`outlineCallsite` metadata, each call site maps precisely back to original positions. Inlining
an outline back is pure code motion, so it's always semantics-preserving once register
allocation is redone. Without metadata, 8R matches the synthetic by naming and shape: static,
synthetic, leaf, only called from call sites.

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

1. **[S] Kept names.** Anything R8 didn't rename (manifest components, JNI `native` methods, keep
   rules, reflection targets). Detected as names that aren't in the minifier's alphabet, or
   that are referenced from the manifest or layouts. This assumes the default minifier
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
- `outline/basic`: a repeated StringBuilder sequence at 5+ sites gets outlined. Expect it
  inlined back, and the outline class gone.
- `outline/no-metadata`: the same fixture with the mapping stripped.

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
- **M2 Synthetics.** Outlines, API-model outlines, backports, `$-CC`, nest bridges, lambdas
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

