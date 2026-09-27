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
  6. **Pack** (`eightr_core::libdb::Pack`, `8RPACK01`), written to
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
