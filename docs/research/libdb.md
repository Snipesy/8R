# Library fingerprints from scenario builds (LibDB): experiments

Exploratory measurements from 2026-09-27 behind the LibDB plan (`~/.claude/plans/library-fingerprints.md`).
The code was throwaway and is not committed. Copies of the experiment binary (`fpexp.rs`), the
generator (`gen.py`), the resolver (`resolve.py`), the build scripts and the hand-written scenario
apps are in `~/.claude/jobs/ca53d1c2/tmp/research/libdb/`.

**Question:** a library method as it appears in an R8-shrunk app: which reference build reproduces
its body exactly? Fingerprint = sigdb `all` (8R IR, program names erased, platform names kept).

## 1. Setup

- **Target:** fixture `compose_lib` (Compose 1.10.6 + material3 1.4.0, R8 9.4.24, min-api 24). Its
  mapping is the ground truth, including inline frames.
- **References:**
  - **D8 jar:** the same library jars through D8, no optimization.
  - **lib-alone:** the libraries through R8 alone, with their consumer rules and
    `-keep public class * { public protected *; }`. This is how today's sigdb is built.
  - **other app (B):** a *different* 60-line Compose app with the same libraries, same R8 and
    min-api. It uses tabs, chips, a dialog, a dropdown, lazy rows and animation, where
    `compose_lib` has a scaffold, a checkbox, a slider and a lazy column.

## 2. Body reproduction

Rows: surviving library methods in `compose_lib`. A row is "inlined-into" when the mapping shows R8
inlined something into the method.

| method kind | n | D8 jar | lib-alone R8 | other app B |
|---|---|---|---|---|
| plain | 3523 | 49% | 70% | **95%** |
| inlined-into | 3900 | 0.7% | 8.5% | **87%** |
| ctor, plain | 880 | 46% | 53% | **90%** |
| ctor, inlined-into | 792 | 1.5% | 8% | **78%** |
| **all** | 9095 | 24% | 36% | **90%** |

- 52% of surviving library methods had something inlined into them.
- The inlining in lib-alone differs from the app's: the app inlines *more* in 66% of cases, because
  single-caller inlining depends on what is reachable.
- Even with identical inline sets, lib-alone misses 25–40%. The differences come from R8's
  whole-program optimizations:
  - constant arguments propagated into the callee;
  - unused parameters removed;
  - field types narrowed (`Object` → the concrete type);
  - classes merged;
  - dead branches pruned.
- An app-shaped build reproduces all of that because the library's own code dominates the
  context.
- **Caveat:** both apps share the `Recomposer`/`Composition` skeleton, so their surviving sets
  overlap heavily. Diversity across real apps is untested.

## 3. Matching precision (fixture, graded)

`compose_lib` was matched against scenario B's library methods. Matching was exact `all` only,
informative bodies only, unique on both sides, with no propagation.

- **Result:** 4300 matches, 4281 correct (**99.6%**). That's **47%** of all 9232 library methods in
  the app.
- **The 18 errors are not hash collisions.** Each is a body identical to a sibling the scenario
  didn't keep:
  - `IconKt$1` vs `$2`;
  - two identical `<clinit>`s;
  - `Recreator.onStateChanged` vs another lifecycle observer;
  - R8-synthesized lambda classes (`…$1$0`), which the DB side should drop, as xtask `Names`
    already does.
- **Implication:** uniqueness within a scenario is not uniqueness within the library.

## 4. Gretio (no mapping)

Gretio is R8 9.4.24, min-api 26, Compose 1.12.1 and material3 1.4.0, all of them S facts from its
marker and `.version` files. It was matched against lib-alone and app B, both rebuilt at exactly
those versions.

- **Unique matches:** 2668 in `classes.dex` and 647 in `classes2.dex`, out of 45.3k informative
  methods. That count includes app, Firebase and protobuf code, so it is not a recall.
- **For comparison:** today's sigdb names 502 methods on Gretio.
- **Class coherence:** 956 of 1049 voted classes agree on one original class.
- **Gap diagnosis:** within the coherent classes, 3002 methods are unmatched.
  - 49% have no method of that proto in the scenario class. This is **coverage**: the scenarios
    don't reach that code.
  - 51% are present with a different body (55% of those have a sketch ≥ 0.6). This is
    **context**: Gretio's transitive dependencies differ (coroutines 1.11.0 vs 1.10.2, core
    1.19.1 vs 1.16.0, lifecycle 2.11 vs 2.9.4, stdlib unknown), and so does reachability.
  - On the fixture the same split is 322 absent / 212 different out of 534.
- **Both causes are real.** Pinning the transitive closure and adding scenario diversity are
  separate remedies.

## 5. Inlined sections

- **R8 inlining:** the mapping has inline frames per position. When a target method matches a
  scenario body exactly, the scenario's frames carry over to it, and 87% of inlined-into methods
  reproduce exactly (§2). 8R's IR keeps debug positions (`lift.rs`), so the output dex can carry
  the positions those frames need.
- **Naive fragment search** (the callee's jar body as a token substring of the caller) found only
  11% of inlined callees with ≥ 4 tokens (807 of 7140), and 69% of callees are < 4 tokens.
  - Fragments must come from R8'd scenario bodies, split by inline frame, not from the jar.
  - This is unvalidated.
- **kotlinc `inline fun` (Column, Row, Layout, remember):** no R8 mapping records these. In
  `compose_lib`, the `Column` body sits in the app lambda as plain app lines. A separate scenario
  type is needed: callers of each public inline function, compiled with the matching kotlinc and
  Compose plugin.

## 6. Generated scenarios (L0, with the user's decision to prefer generated)

Two generators, both driven only by the library's public API (from the D8 dex of the jars):

- **keep-rule roots (`k`):** `-keep,allowobfuscation,allowoptimization` on a deterministic
  hash-sample of public methods (by seed). No consumer code at all. Kept roots can't be inlined
  into callers or specialized.
- **Java callers (`j`):** `gen.py` writes `gen.G*.run()` methods that call the sampled API.
  - Arguments come from `gen.O` (a kept opaque source, `volatile` backed), so R8 can't fold them.
  - Each call sits in its own `try`.
  - javac `--release 17` compiles against the jars; calls it rejects are dropped (about 20%).
  - No kotlinc is needed.

### Fixture `compose_lib` (graded)

| scenario set | bodies reproduced (of those the scenario has) | unique matches | precision | recall |
|---|---|---|---|---|
| hand app B | 90% | 4300 | 99.6% | 46.6% |
| `k` 5%, 1 seed | 76% | 2724 | 99.3% | 29.5% |
| `k` 5%, 6 seeds | — | 3539 | 99.4% | 38.3% |
| `j` 5%, 1 seed | 80% | 2056 | 99.5% | 22.3% |
| `j` 5%, 6 seeds | — | 3694 | 99.4% | 40.0% |
| `j`×6 + `k`×6 | — | 3926 | 99.5% | 42.5% |
| all 15 generated | — | 3938 | 99.6% | 42.7% |

- Sparse samples look more like an app; dense ones (20%) cover more code but inline less the way
  an app does.
- Several sparse seeds beat one dense sample.
- Generated scenarios come within 4 points of a hand-written app that happens to share the target's
  skeleton.

### Gretio (profile R8 9.4.24, api 26, closure pinned)

- **Closure:** `resolve.py` resolved it from Gretio's 118 declared `.version` artifacts, pinned,
  plus everything else at the highest requested version, the way Gradle resolves. Result:
  154 artifacts, stdlib 2.2.20, collection 1.6.0, coroutines 1.11.0.
- **Resolver notes:**
  - Gradle `.module` metadata is required: KMP roots redirect through `available-at` to
    `-android`/`-jvm` artifacts.
  - BOM/platform modules must be skipped.
- **Scenarios:** 9 generated (`j`×3, `k`×6, 5% each; 3 `j` seeds failed on a javac
  type-annotation error in `androidx.window`).

| references | unique matches (classes + classes2) | voted classes, single-class |
|---|---|---|
| today's sigdb | 502 (names applied) | — |
| hand app B + lib-alone, unpinned closure (§4) | 3315 | 91% |
| **9 generated, pinned closure** | **6606** | **92%** |
| 9 generated + lib-alone + hand app B | 6872 | 92% |

- Class coherence equals the fixture's (92%), where precision was 99.6%.
- **Remaining gap in coherent classes:** 2075 methods are absent from the scenarios (coverage:
  more seeds or targeted roots); 3062 are present with a different body (half with sketch ≥ 0.6).
- **Candidate causes of the body differences:**
  - stdlib: the app's Kotlin plugin chooses it, and 2.2.20 is only the highest *declared*
    version. The forge can identify it by building candidates and keeping the best-matching one.
  - reachability differences.
- **Open:** the saturation curve (how many seeds), targeted roots for classes that are voted but
  incomplete, and stdlib version identification.

## 7. The forge (L1, `crates/eightr-forge`, binary `8r-forge`)

- **Profile:** `8r info --profile APP` or `8r-forge profile APP` prints the build profile JSON.
  - R8 version, min-api and mode come from the marker.
  - Declared libraries come from `META-INF/<group>_<artifact>.version`, and `kotlinx_<name>.version`
    for `org.jetbrains.kotlinx:kotlinx-<name>`.
- **`8r-forge build PROFILE|APP [--scenarios FILE] [--pin g:a:v] [--jobs N]`** does these steps:
  1. **Resolve** (`maven.rs`): Gradle module metadata first, then the POM. Pins win; everything
     else gets the highest version requested within the selected graph. KMP `available-at` targets
     of pins count as declared.
  2. **Prepare** (`artifacts.rs`): AAR `classes.jar`, `libs/*.jar` and `proguard.txt`. For jars,
     the `META-INF/com.android.tools/r8*` directories whose version range contains the R8
     version, else `META-INF/proguard`.
  3. **API index** (`api.rs`): read straight from the class files.
  4. **Scenarios** from the catalog (`data/scenarios.conf`: lib-alone, 6 roots, 6 callers, 5%
     each). R8 is fetched by version from Google Maven. All closure jars are program input, with
     their consumer rules.
  5. **Fingerprint** (`fingerprint.rs`): 8R's rewrites first, then keys from the mapping.
     Inline-frame tables are computed per instruction before the rewrites and carried over by pc
     where the instruction is unchanged.
  6. **Pack** (`eightr_core::libdb::Pack`, `8RPACK02`), written to
     `<cache>/packs/<profile key>-<inputs hash>.8rpack`. The cache is `$EIGHTR_FORGE_CACHE`, else
     `~/.cache/8r-forge`.
- **Reproducible:** a rebuild from an empty work cache gives the same bytes. The ignored test
  `packs_are_reproducible` checks this.
- **`compose_lib` fixture** (`8r-forge grade`), with its profile read from its own `.version`
  files: 70 artifacts, 13 scenarios, a 6.9 MB pack, 48 s.
  - Exact unique matching: **41.6% recall, 99.45% precision.**
  - For correct matches with inlined code, the pack's frame table equals the app's own mapping
    frames in 84% of cases.
  - The remaining errors are sibling lambdas with identical bodies, and one version gap:
    `androidx.collection` is undeclared, so the resolver's version differs from the fixture's.

### L1 review fixes

- **Resolver fixpoint:** each round now walks the previous round's selection, so the dependencies
  of an evicted version no longer count. Non-convergence is an error.
- **POM parsing:** parent POMs, `<properties>`/`project.*` substitution and `dependencyManagement`
  versions; only top-level `<dependencies>` count, build plugins don't. BOM imports and exclusions
  are not modelled yet.
- **POM fallback:** a `.module` file without a runtime library variant falls back to the POM.
- **Missing modules:** coordinates on neither repository are recorded in the lock as `missing`
  instead of aborting.
- **Downloads:** they need curl success as well as HTTP 200, and must match the repository's
  `.sha1` sidecar.
- **Frames:** R8's same-name wrapper frames and its own synthesized frames are left out of the
  inline stacks.
- **Pack identity:** packs carry `fingerprint = libdb::code_id()`, a hash of the source of 8R's
  lifting, rewrites and fingerprint code. It is part of the forge cache key, and 8R ignores a pack
  whose id differs from its own.
- **Decoding:** `decode` validates every cross-reference and caps the inflated size.

## 8. Packs in 8R (L2)

- **Selection:** `8r undo APP --libdb PACK|DIR` uses the packs whose profile equals the app's
  (from its marker and `.version` resources) and whose fingerprint code matches. Other packs are
  reported and ignored.
- **Matching:** a used pack is converted to a matcher DB (`Pack::to_sigdb`) and goes first. The
  embedded DBs follow, minus the classes a pack covers (`sigdb::matcher::with_packs`), because the
  same method in two DBs would make exact matches ambiguous. Every sigdb name stays D, via
  `r8/sigdb-method-name`.
- **Embedded fallback DBs:** `cargo xtask sigdb` now forges them. Each `fixtures/sigdb.conf` line
  is a Maven coordinate forged as "that library alone" with the default catalog. Only the
  library's own (declared) classes are kept.

On `sigdb_app` (graded matcher test):

| embedded DBs | precision | recall |
|---|---|---|
| before (library-alone builds) | 95.1% | 59.3% |
| forged | 98.1% | 72.9% |

`compose_lib` fixture, 8R labels graded against the mapping:

| setup | sigdb names | correct |
|---|---|---|
| embedded DBs only | 151 | 92.7% |
| with its pack | 2051 | 99.3% |

Gretio:

- The pack covers 160 artifacts, 121 of them declared, including `androidx.fragment` 1.9.1
  (`fragment`, `-ktx`, `-compose`), and has 195k records. It took 7m35s to forge (13 scenarios,
  `--jobs 3`).
- `r8/sigdb-method-name` goes from 544 applications without the pack to 4865 with it.

### Default-argument scenarios (`defaults`)

- **Why:** in Gretio, `setContent { … }` and `enableEdgeToEdge()` stayed unnamed. Both are Kotlin
  calls that take default arguments. R8 constant-propagates the defaults kotlinc passes (`0`/`null`
  plus the mask) into the `$default` bridge and the function, drops the parameters, and inlines the
  bridge. The result, e.g. `setContent(ComponentActivity, ComposableLambdaImpl)`, is a body no
  opaque-argument scenario produces.
- **What the scenario does:** it calls a hash-sample of the public bridges (`f$default(…, int, Object)` and
  constructors ending in `(…, int, DefaultConstructorMarker)`) as kotlinc does for "every default".
  - Which parameters have defaults is read from the bridge's own bytecode: `iload mask; <bit>;
    iand; ifeq L; …; <x>store p` (api.rs `default_bits`).
  - Those parameters get constant `0`/`null`; the others are opaque.
  - javac can't call synthetic members, so the callers are written as class files directly
    (`classfile.rs`: straight-line code, no stack map frames needed).
- **Catalog:** a single `d1` covering 100% of the bridges; there are only about 2,000 of them.
- **Call shape:**
  - Each call is made twice, from `gen/D*` and `gen/E*`. With one call site, R8 inlines the callee
    into it and no method is left to fingerprint.
  - Each call sits in its own method, because a reified inline function's compiled body always
    throws, and that turns the rest of its method into dead code.
- **Keying:** when a bridge survives with its own function inlined into it, the record is keyed
  as that function (fingerprint.rs). It is "`f` with its defaults", the body an app has once the
  bridge was inlined into the call site.
- **`compose_lib` grade:** recall 41.6% → 42.5%, precision 99.45% → 99.52%. Inline frames equal the
  app's own in 87.5% of matches, up from 84%; that gain comes from the wrapper-frame fix.

### Instantiable argument types

- **Problem:** when a scenario passes an opaque value as a program class nothing instantiates
  (e.g. `ComponentActivity`), R8's instantiated-types analysis compiles every cast to it as a
  `ClassCastException`, and the library code behind it is lost. In an app, AGP keeps manifest
  components' constructors and app code subclasses these types.
- **Fix:** callers and defaults scenarios now keep the constructors of the concrete classes they
  pass values as (`gen::instantiable`). Abstract classes and interfaces would need generated
  subclasses; that is not done yet.
- **Effect on Gretio:** each callers scenario fingerprints 24–26k methods, up from 11–15k, and
  the pack grows from 197k to 209k records.

### Performance

These are measured by two investigation agents. Output is unchanged: packs are byte-identical,
and 8R output is byte-identical with and without a pack.

- **Forge:**
  - R8 JVMs get `-XX:ActiveProcessorCount = cores / jobs`, and `--jobs` defaults to 4. That
    alone is about −28% CPU.
  - Each worker fingerprints its scenario right after R8.
  - Java keywords are filtered out of the generated callers.
  - Isolation of files with unattributable javac errors runs in parallel.
  - Scenario outputs are cached per scenario definition and generator version, separately from
    the fingerprint code, so catalog edits and fingerprint changes don't re-run unchanged R8
    builds.
  - Result on Gretio: 7:35 → about 3–4 min cold, and seconds to minutes when only some
    scenarios change.
- **8R:**
  - `naming::class_labels` computes its reference sites once and prints each method once per
    round.
  - `Renaming::apply` memoizes its per-symbol rewrites.
  - The rebox pre-checks skip the dataflow when a method can't hold a chain.
  - The sigdb pass builds a child index once.
  - Result on Gretio: about 36 s → about 22 s quiet (the earlier 2½-minute runs were CPU
    contention from parallel forge JVMs), and the pack's extra cost fell from +6.6 s to +1.2 s.
- **Two correctness fixes found on the way:**
  - A directory input's resources are selected like a package's, so a fixture's `mapping.txt` is
    never read.
  - `code_id` now covers `enum_unboxing.rs`.

### Gretio with its pack (final L2)

- **Pack:** 160 artifacts, 14 scenarios, 209k records.
- **Result:** `r8/sigdb-method-name` 4981 (544 without a pack). Every class ART-verifies, and the
  app launches and reaches `MainActivity`.
- **Embedded fallback DBs, re-forged** (`sigdb_app`): 98.4% precision, 76.6% recall. The test
  ratchets at 97 / 75.
- **Still unnamed: `setContent`.** Gretio's build pinned `ComposeView` (its original name and full
  constructor survive, presumably through a layout or manifest keep rule), and that changes how R8
  compiles `setContent` around it. Next step: derive the app's pins from its own dex, i.e. library
  classes and members that kept their library names, and apply them as keep rules in every
  scenario (per-app packs).

### L2 review fixes

- **Pack selection doesn't depend on order:** packs are sorted by path and selected in
  (profile, catalog, tools, lock) order. Packs record their generator versions, so two packs for
  one profile differ. Selection findings are sorted.
- **Coverage:** a pack displaces embedded DB records only for its declared artifacts' classes.
  Undeclared versions (a stdlib the resolver chose) are a guess.
- **Name validation:** `Pack::decode` rejects names that aren't dex simple names, class
  descriptors and protos, because those names end up in the output dex.
- **Code identity:** `code_id` also covers `data/platform-api.txt`, the dex reader and `sym.rs`.
- **`$default` keying:** a bridge is keyed as its function only when that function (same
  class, name, the bridge's proto minus masks, marker and optional receiver) is the outermost
  inlined frame.
- **`default_bits`:** only `ifeq` patterns count, bit 31 is accepted, and bridges with several
  masks are skipped.
- **Resolver:** interpolated `dependencyManagement` keys, dependencies inherited from parents,
  and monotone version selection (converges).
- **CLI and inputs:** undecodable pack files are skipped with a warning, and an unpacked `.aab`
  directory's `base/root/` resources are read.
- **After the fixes:** `sigdb_app` holds at 98.4% / 76.6%, and Gretio has 4927 sigdb names, down
  from 4981 before the fixes (exact `$default` keying; no displacement by guessed versions). It
  ART-verifies and runs.

## 9. Matching the app's build context: `setContent`

In Gretio, `setContent { … }` (activity-compose) was the method that stayed unnamed. Three generic
gaps between the scenarios and a real app build caused it. Each is fixed below, and the method now
fingerprints identically (`12fecb805b8c39ff`) in 16 of the 17 scenarios and in the app.

1. **App-derived pins** (`pins.rs`).
   - **Problem:** the app's build kept library code for reasons the forge can't see: AGP's
     manifest and layout rules, the app's own rules, reflection. Gretio keeps `ComposeView`'s name
     and constructors, and R8 compiles `setContent` around those pins.
   - **What counts as a pin:** a closure class whose exact descriptor is in the app dex kept its
     name. A member counts when it has the library's name (at least 3 characters) and descriptor,
     or a unique library name of at least 4 characters with compatible parameter kinds; the app may
     have renamed the types.
   - **Use:** the pins become `-keep` rules in every scenario (Gretio: 409 classes).
   - **Per-app packs:** `8r-forge build APP` records `Pack::app`, the sha256 of the app's dex
     files (`libdb::app_id`). 8R uses such a pack for that app only. Its own pack goes before a
     profile pack.
2. **Library `R` classes** (`rclass.rs`).
   - **Problem:** an app build generates every library's `R` classes with final ids, and R8 folds
     `R.id.x` into constants. That is what makes `ViewTreeLifecycleOwner.set` a
     `setTag(0x7f…, owner)` small enough to inline into `setContent`. AARs ship only `R.txt`.
   - **Fix:** the forge writes the `…/R$<type>` classes the closure's bytecode reads, with ids by
     sorted (type, name) and styleable indices and array lengths from the AARs' `R.txt`, and adds
     them to every scenario.
   - **Fingerprints on both sides** normalize app resource ids (`0x7f` package, type 1..0x40) to
     one token, because the numbers are per app (`sigdb::print::is_app_resource_id`).
3. **Default-argument calls in disjoint parts.**
   - **Problem:** calling every bridge in one scenario adds call edges an app doesn't have, which
     changes single-caller inlining.
   - **Fix:** `defaults` is split by hash bucket (`part=I/K`, 4 parts).
   - **Keying:** a bridge whose function is inlined into it is keyed as the function only when R8
     removed its mask and marker (every caller passed the same defaults), because then the body is
     the function specialized with them. A bridge that kept its mask stays a bridge.
   - **Ambiguity:** the specialized body is also what an app has when R8 inlined the function into
     the surviving bridge. The name `f` vs `f$default` is R8's context-dependent choice, and the
     pack uses the API name `f`. `grade` counts these separately.

**Results:**

- **Gretio:** `setContent(ComponentActivity, ComposableLambdaImpl)` is named; 5003 sigdb names;
  ART-verifies and runs.
- **`enableEdgeToEdge`** is still unmatched. In the app, R8 inlines `SystemBarStyle.auto` into it
  twice, and no scenario reproduces that call-graph context yet.
- **`compose_lib` fixture:** 40.5% recall, 98.86% precision, after the review fixes below.
  - 20 of the remaining errors name `f` where the app kept `f$default` for the same specialized
    body.
  - Correction: the earlier 42.5% came from a pack built before the L2 review fixes. This commit's
    parent grades 40.1%.
  - The fixture's AGP-less build (unresolved library `R` reads) costs only about 0.3 points. It is
    still a follow-up to generate library `R` classes in the fixture pipeline as AGP does.
- **`sigdb_app`:** 98.6% precision, 76.5% recall.

**Review fixes:**

- **`$default` re-keying:** decided by the body. A bridge whose `and` still reads an int parameter
  before any write to it tests its mask and stays a bridge. R8 always drops the unused marker, so
  arity alone was wrong.
- **Pack cache path:** includes the app id.
- **Library-alone scenario:** no longer keeps the generated `R` classes, so its `R` reads fold.
- **Pins:** constructors and `equals`/`hashCode`/`toString` overrides survive in any build, so
  they aren't evidence on their own. Constructors are pinned only in classes with other kept
  members (a view kept for its layout).
- **Gretio:** `setContent` still matches (16 scenarios), 5004 sigdb names, ART-verifies and runs.

## 10. Outline detection graded on forge scenarios

Every scenario build is real R8 output over real libraries, with a mapping that marks R8's
outlines (`com.android.tools.r8.outline`; throw outlines are synthesized methods). That is a far
larger truth set than the fixtures. Running `8r undo` on 7 of Gretio's scenario builds:

- **Precision:** 822 detections, every one an R8-synthesized method, and none of them app or
  library code. 572 are marked outlines; the rest are throw outlines.
- **Recall:** 100% of marked outlines in 6 scenarios. Library-alone missed 30, all in one holder
  class R8 had merged with a class whose initializer builds a constant `int[]`
  (`filled-new-array`). The purity check didn't accept that instruction, so the holder was
  rejected. Now fixed: constant primitive arrays are pure. 1 miss remains: a pure-arithmetic
  outline (`(a*b)/c + d`). Classic outlines must make a call, so backports (also pure arithmetic)
  aren't labeled as outlines. Missing one only costs readability.
- **Label mismatch:** outlines that return the exception their callers throw are labeled
  `r8/bu-outline-inline`, while R8's mapping marks them as classic outlines. Both are inlined the
  same way.

## 11. L3: S names, class, field and package restoration from packs

**Rules** (`crates/eightr-core/src/passes/libdb_names.rs`, `libdb/mod.rs::PackFacts`):

- `r8/libdb-method-name` (S). The match is `exact:all` against a strict record, with at least two
  such matches of the app class agreeing on one pack class. A strict record meets all of these:
  - informative and unique in the pack;
  - not a `$default` bridge and not of a function that has one;
  - not in a synthetic-looking class, and its artifact is declared;
  - not mostly one other method inlined;
  - not a sibling: same class and erased proto with a near-identical sketch, or a thin wrapper
    of one;
  - not a code clone: no other method ever had the same body in any build (the copies of
    `AnchoredDraggableState` in foundation, material and material3).
- `r8/libdb-class-name` (S), for the simple name and the package. At least two S methods must
  agree, every exact match must agree, and the class must fit:
  - its exact shape (supertypes, fields and now its kind: abstract or interface) is one the pack
    saw for that class, and for no class of its hierarchy. R8 merges a class into its only
    subclass, which then has its shape: `FragmentFactory` into `FragmentManager$3`, and
    `AbstractClickableNode` into `ClickableNode`;
  - every method's erased proto is one of the class's.
  
  The package label is recorded only when the full original name is safe:
  - its package isn't on the boot class path (android.jar's packages, `java/`, legacy boot
    packages such as `org/apache/http` and `org/json`);
  - it isn't named, in dotted, slashed or descriptor form, by a program string or a resource.

  The label states the original package even where the move is refused (refusals are reported
  as a finding).
- `r8/libdb-field-name` (S) and `r8/libdb-field-hint` (D). The n-th field access of a matched body
  is the n-th of its pack twin. Every aligned access must agree, the owner's identity must match
  the pack field's class, and there must be no collision within the class tree. S also needs every
  supporting match to be S, a strong identity, and a type unique in the class or two methods
  agreeing. The field namespace is a class's whole hierarchy component, interfaces included.
- Package restoration (`crates/eightr-core/src/repackage.rs`) runs over the whole program after
  every move. A move is cancelled, to a fixpoint, when:
  - it separates a non-public class from a class that references it;
  - it separates a package-private member (or a protected one, outside subclasses) from its user;
  - it changes whether a method overrides a package-private one;
  - it separates a method handle or call site from the classes it mentions;
  - the target name collides with another class. This is checked every round, so a cancelled
    move still holds its name.

  Member access is resolved the way ART does:
  - fields are static or instance, as the instruction says;
  - methods are looked up in the superclass chain before interfaces;
  - method handles are access-checked too.
  
  Items named from a pack carry `@eightr.Library(value = "g:a:v", app = …)`.

**Forge.**
- Pack v6 records each class's original superclass, which gives the hierarchy in the shape rule.
- Gradle `dependencyConstraints` align modules (collection 1.4 makes `collection-ktx` 1.4).
- R8 8.4.x comes from R8's release bucket, because Google Maven skips that line.
- Mapping attribution: without line info, R8 can open a method with a stack that holds only an
  inlinee and attach the method's residual signature to it. The signature goes to the method's
  next outermost frame when the frame is inlined elsewhere in the method, or when it has fewer
  parameters than the signature minus a receiver.

**Truth harness (`8r-forge harness`).** Leave-one-out over each pack's 17 scenario builds, plus the
full-R8 Pokédex built from source (`scripts/build-pokedex-r8.sh`), whose mapping the pack never
saw:

| Truth set | S methods | S classes (name / package) | S fields | D methods | D field hints |
|---|---|---|---|---|---|
| Gretio pack, LOO | 25,211 / 25,211 | 1,911 / 1,910, all right | 6,246 / 6,246 | 99.27% | 99.94% |
| Pokédex pack, LOO | 13,316 / 13,316 | 1,397 / 1,397, all right | 3,409 / 3,409 | 99.16% | 99.94% |
| Pokédex app (held out) | 569 / 569 | 53 / 53, all right | 161 / 161 | 98.13% | 99.89% |

Every S label is right on every truth set. Each S error the harness found became a generic rule:
- vertical merges: class kind, and shape ambiguity within a hierarchy;
- code clones;
- thin wrappers;
- R8's lone-inlinee frames: forge attribution.

The clone, sibling and shape rules cost about 4% of S recall.

**Real apps** (`scripts/smoke-suite.sh`, ART, packs from `8r-forge build APK`). For each app: the
whole program verifies, it launches, and a tap reaches the next screen. A second 8R run on the
output, with the same packs, gives the same bytes.

| App | Packages restored |
|---|---|
| Gretio | 89 |
| Pokédex, full R8 | 51 |
| Pokédex CTF | none; R8 kept androidx and kotlin, and there are no version files, so no pack |

**Idempotence with packs.** A per-app pack also applies to input tagged `@eightr.Library(app = its
app)`, which is 8R's own output for that app. Matches of already-named methods still count as
evidence. Provenance (`@eightr.Original`) is the first writer's, and an earlier run counts as
first. It is replaced only when it names a different method. Classes split off merged classes get structural names: `X$$SplitN` is not a kept name.

**α and offline tests.** `crates/eightr-forge/tests/oracle_pack.rs` merges a pack from the
`compose_shapes` and `compose_witness` builds and applies it to `compose_witness2`. It checks:
- every S label is the mapping's truth;
- packages are restored and tagged;
- the output is a fixed point with the pack;
- the result is α-invariant under random renamings (machinery shared in
  `crates/eightr-core/tests/support/alpha.rs`).

**Fixed on the way, by the Pokédex builds.** A superclass method implementing an interface method
only through a subclass is now in that method's override group. Without that, the result was an
`AbstractMethodError`. Package-qualified class names in strings (R8's `-adaptclassstrings`, such
as Hilt's `@LazyClassKey`) now pin their classes. Fixture: `r94_slot_strings`.

**Not done or known gaps.**
- Gretio's `ComponentActivityKt.setContent` stays D. Its class has a single method, and S needs
  two agreeing.
- Class names in default-package strings (`-repackageclasses ''`) are not pinned. Strings like
  `"r"` are everyday strings, and pinning on them cost S names.
- L3f "near-miss": the matcher's call-graph and mutual-best stages already propagate from
  anchors. D precision stays ≥ 98% (98.1–99.3%).
