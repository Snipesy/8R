# Library signature DB (DESIGN §5.4): name-independent fingerprints for non-Compose libraries

Research prototype and measurements. Nothing in `/home/snipesy/8R` was modified. All work is under
`/home/snipesy/.claude/jobs/ca53d1c2/tmp/research/sigdb/` (paths below are relative to it).

## TL;DR

- **Recommended DB**: build it from each library version **run through R8 alone** (same R8 as the
  target, consumer rules + `-keep public class * { public protected *; }`). Fingerprinting plain
  D8 output instead costs about 11 recall points (60 → 49%) at the same precision. Adding D8 variants
  alongside the R8 ones gains nothing.
- **Recommended matcher**: first, exact unique-unique hashes on the strict body fingerprint (`all`)
  and on the sorted string set. Then class-constrained propagation to a fixpoint (call graph,
  unique erased proto within a voted class, mutual-best similarity). Each round resolves its
  proposals without conflicts, so the result is α-invariant (tested). Same-version result:
  **94.2% precision / 59.4% recall** for method → (class, name) over *all* library methods left in
  the app (15k methods, 7 apps). Class precision per method is 96.6%. Restricted to the 54% of
  methods **R8 actually renamed**: **92.2% P / 54.8% R**. Constructors and `<clinit>` (20%) score
  96.6/67.1; names R8 kept (26%) score 96.2/63.1, so there only the class is new information.
- **Version drift is cheap**. Matching against a DB of *another* version loses about 2 recall
  points and 1 precision point on average. The worst case is okio 2.8 → 3.x, a major rewrite: −5 R
  / −4 P. Matching against **all versions at once** gives the same result as knowing the version
  (cascade E: 58.9/93.9 vs 59.4/94.2). So don't identify the version first: match against every
  version.
- **Version identification** from code alone is unreliable when the app uses only a small slice.
  Similarity voting got the version exactly right in 17/25 (app, library) cases. Coroutines and
  okhttp went 5/5 each (1.10.1 vs 1.10.2 was separated by one vote). collection and stdlib failed
  because the used slice doesn't change between versions, which also means the version doesn't
  matter for naming. Explicit markers do better: `META-INF/kotlinx_coroutines_core.version`, the
  `"okhttp/4.12.0"` user-agent literal.
- **Tier**: every sigdb name is **D** (a hint with a hash suffix). The strictest stage is 97–99%
  precise, not 100%. The DB is not an exhaustive candidate universe: app code and unknown
  libraries can compile to the same body. Only 68–71% of informative library methods have a hash
  that is unique across the 5 libraries × all versions. R8's output is a many-to-one function of
  the source. Within D, use provenance as the strength when sigdb conflicts with other hint sources.
  Strong (≥98%): exact `all`, call-graph, and class+unique-proto. Weaker: mutual-best (~89–91%),
  strings (83–89%). Never seed with `api`/`nums` alone (74–76%). Emit **N candidate sets** only
  when they're small (cap ≈ 8): uncapped they average 140 entries with the truth in them 70% of
  the time.

## 1. Setup

Toolchain: R8/D8 9.4.24 (`target/xtask-cache/r8-9.4.24.jar`), kotlinc 2.4.20, JDK 26, android-36
`android.jar`, `dexdump` 36.0.0. `--min-api 24`, `--release`.

| library | versions (Maven) | consumer rules used |
|---|---|---|
| kotlinx-coroutines-core-jvm | 1.7.3, 1.8.1, 1.9.0, 1.10.1, 1.10.2 | `META-INF/com.android.tools/r8/coroutines.pro` |
| okhttp | 4.9.3, 4.10.0, 4.11.0, 4.12.0 | `META-INF/proguard/okhttp3.pro` |
| okio (okio 2.8.0 / okio-jvm 3.x) | 2.8.0, 3.0.0, 3.2.0, 3.6.0 (okhttp's pinned deps) | `okio.pro` (none in 3.6.0) |
| androidx.collection (collection-jvm) | 1.4.0, 1.4.5, 1.5.0 | none |
| kotlin-stdlib | 2.1.21, 2.2.21, 2.3.21, 2.4.20 | none |

**Test apps (oracle = R8 mapping, never an input):**
- `apps/src/Main.kt` is a Kotlin app. It uses a broad slice of coroutines (channels, flows,
  select, Mutex/Semaphore, stateIn/shareIn, supervisor), okhttp (builder, interceptors, forms,
  multipart, pinning, cookies), okio (gzip, hashing, base64), collection (ArrayMap, LruCache,
  scatter maps, IntList, …) and stdlib. Each of `app0..app4` is compiled *against* its own library
  versions (`-no-stdlib`, so kotlinc inlines the right version of inline functions). R8 then
  shrinks it with consumer rules as AGP would (`apps/build.sh`). Versions:
  app0 = {1.7.3, 4.9.3, 2.8.0, 1.4.0, 2.1.21}, app1 = {1.8.1, 4.10.0, 3.0.0, 1.4.5, 2.2.21},
  app2 = {1.9.0, 4.11.0, 3.2.0, 1.5.0, 2.3.21}, app3 = {1.10.2, 4.12.0, 3.6.0, 1.5.0, 2.4.20},
  app4 = {1.10.1, 4.12.0, 3.6.0, 1.4.5, 2.4.20}.
  Each has about 2.6k residual methods, 2.5k of them library code.
- `apps/srcB/b/Main.java` is a second, *different* slice: a Java app using okhttp (dispatcher,
  auth, cache, event listener, websockets), okio (deflate, hashing) and collection.
  `appB0` = app0's versions and `appB3` = app3's (`apps/buildB.sh`). It guards against
  overfitting to one slice.

**Reference DBs** (`db/build.sh`), per library version:
- `d8`: the plain jar dexed with D8 `--release`.
- `r8`: the library alone as R8 program input, with its consumer rules plus
  `-keep public class * { public protected *; }`. Dependencies are passed as `--lib`, and the
  mapping gives the original keys. Note: Kotlin `internal` is public in bytecode, so this keeps
  most of the library. It measures **R8 codegen/optimization residue**, not app-dependent
  shrinking.
- `both`: d8 and r8 as variants of the same canonical key.
- `d8n`: d8 with R8's treatment of Kotlin null-check intrinsics modelled. This is done on the DB
  side only, where names are known: `const-string + Intrinsics.checkNotNull*(Object,String)` →
  `Object.getClass()`.

**Oracle correctness.** `dexio.parse_mapping` resolves every residual dex method (0 misses in
all apps). Two R8 9.x mapping quirks mattered:
1. `residualsignature` is emitted once per method, after its first stack, and applies to all its
   line ranges.
2. R8 9.4 adds a **synthesized outermost frame with a package-relative class name** when it
   moves or bridges a method. For example, `SharedFlowImpl OpenJSSEPlatform$Companion.MutableSharedFlow(int,int,BufferOverflow):0`
   wraps `kotlinx.coroutines.flow.SharedFlowKt.MutableSharedFlow`. The original is the inner
   frame, and relative names must be qualified with the block's package. Before this fix, app3
   showed 50 (r8 DB) / 22 (d8 DB) "false positives on app code". They were library methods whose
   truth class had lost its package. After the fix there are 0–1. **This matters for 8R's oracle too.** Check
   that `ClassMapping::outermost_methods()` handles relative, synthesized outer frames. I did not
   check the Rust code.

## 2. Features (all computed with R8-renamable names erased)

**Erasure rule (α-invariant by construction).**
- App side: a referenced type or member is *stable* iff its class is **not defined in the app's
  dex**.
- DB side: stable iff not defined in this DB dex **and** not a class of any library in the DB
  universe. Program refs from okhttp to okio or stdlib are erased, as they are in the app.
  `sun/misc/Unsafe`, `org/conscrypt/*` and the like stay stable on both sides.
- Stable refs keep their full names. Program refs keep only their shape: `I:?(ILjava/lang/String;)Z`
  for invokes (plus `<init>`), `iget:?Ljava/lang/Object;` for fields, `new-instance:L?` for types.

Per method (`sigdb.fingerprint_program`):

| family | definition |
|---|---|
| `strings` | sorted multiset of `const-string` literals |
| `api` | sorted multiset of stable refs: framework/JDK method and field refs, stable type refs, catch types |
| `refs` | *ordered* sequence of all ref tokens (stable refs full, program refs erased), plus catch types |
| `ops` | normalized opcode sequence: drop moves/goto/nop/payloads; collapse width, `/lit`, `/2addr` and `/range` variants; `invoke-*` → `invoke`; `if-*` → `if`/`ifz`; typed get/put/return families collapsed |
| `nums` | sorted numeric literals except 0/±1 |
| `proto` | erased method proto |
| `ops+proto`, `all` = (ops, refs, strings, nums, proto), `all-noproto` | combinations |
| grams (fuzzy) | multiset of opcode 3-grams ∪ ref tokens ∪ strings ∪ nums; weighted Jaccard |
| class C2/C3 | (first stable ancestor, stable interfaces, erased static/instance field-type multisets); C3 drops static fields |

**Informativeness gate** (app side, exact stages): ≥ 8 normalized ops, or ≥ 1 string, or ≥ 2 stable
refs. Without it, trivial bodies (`<init>` delegators, getters, `new-array; sput`) produce unique
but wrong matches, because the true counterpart's residual body changed. The gate raised `all`
precision from 95.8% to 99.2% (D8 DB).

## 3. Matching algorithm (`sigdb.match_combined`)

1. **Exact cascade.** For each family in order, among still-unmatched methods, take a match only
   when the hash is unique on *both* sides (unique-unique). A DB hash shared by several canonical
   keys, or an app hash shared by several methods, yields a candidate set instead. DB variants
   (versions, d8/r8) collapse to their canonical key (class, name, proto).
2. **Class seeds.** Unique-unique C2/C3 class hashes (about 100 of 520 classes, ~95% correct).
3. **Propagation rounds, until fixpoint** (≤ 8 rounds). Each round gathers proposals against the
   state at the start of the round:
   - call graph: for each matched pair, align the program callee lists by erased-proto sequence
     (`difflib`, no autojunk) and propose aligned pairs whose Jaccard is ≥ 0.5;
   - class votes: 3 per seed, 2 per matched member, 1 per program owner aligned in matched
     bodies. Only a unique top vote counts;
   - within a voted class pair: a unique erased proto on both sides (Jaccard ≥ 0.3); then
     **mutual-best** by Jaccard + 0.2·(same proto), requiring ≥ 0.35 and a margin of ≥ 0.05 over
     the runner-up on both sides.

   A pair is accepted only if neither side has a competing proposal in the round; the rest become
   candidate sets. This makes the result independent of iteration order, which would otherwise
   leak R8's names into the outcome. `alpha_test.py` renames every app class and method with
   random bijections (2 trials) and gets byte-identical results.

Tie-breaking is deterministic: sorts are over DB (original) keys only, hashes are SHA-1, and
thresholds are fixed.

## 4. Results

Grading: method → (original class, name) from the app's mapping (outermost *real* frame). "R" is
over **all library methods residual in the app**. Per-library P excludes the 0–2 false positives
per app on app-own code.

All tables in 4.1–4.4 use cascade A, the first matcher run (all, all-noproto, strings, refs, api,
ops+proto). 4.5 (bottom), 4.6 and the TL;DR use the recommended cascade E (all, strings).

### 4.1 Feature families alone (exact unique-unique, 5 apps; cross = only (app, DB) pairs whose version of that library differs)

| family | DB | same: #pred/app | same: P | same: R | cross: P | cross: R |
|---|---|---|---|---|---|---|
| strings | d8 | 230 | 85.5 | 7.8 | 82.8 | 7.2 |
| strings | r8 | 307 | 91.4 | 11.1 | 88.8 | 10.1 |
| api (framework-call multiset) | d8 | 107 | 84.1 | 3.5 | 82.2 | 3.2 |
| api | r8 | 209 | 85.9 | 7.1 | 84.7 | 6.5 |
| refs (ordered) | d8 | 104 | 95.0 | 3.9 | 93.2 | 3.6 |
| refs | r8 | 236 | 92.6 | 8.7 | 92.3 | 8.1 |
| ops (normalized opcodes) | d8 | 97 | 94.8 | 3.6 | 91.9 | 3.3 |
| ops | r8 | 236 | 95.1 | 8.9 | 93.4 | 7.9 |
| nums | d8 | 73 | 75.5 | 2.2 | 71.8 | 1.9 |
| nums | r8 | 84 | 75.8 | 2.5 | 73.2 | 2.3 |
| proto (erased) | d8 | 61 | 98.0 | 2.4 | 94.7 | 2.2 |
| proto | r8 | 76 | 90.6 | 2.7 | 88.8 | 2.6 |
| ops+proto | r8 | 231 | 96.1 | 8.8 | 94.7 | 7.8 |
| **all** | d8 | 122 | **99.2** | 4.8 | 97.2 | 4.3 |
| **all** | r8 | 278 | **97.3** | 10.7 | 96.6 | 9.5 |
| exact cascade (no propagation) | r8 | 632 | 89.8 | 22.5 | 88.3 | 20.7 |
| **combined** (cascade A + propagation) | d8 | 1346 | 92.4 | 49.2 | 91.6 | 46.9 |
| **combined** | r8 | 1637 | 92.8 | 60.2 | 91.9 | 58.2 |
| combined | both | 1638 | 92.7 | 60.1 | 91.8 | 58.2 |

Takeaways:
- No single family gets past 11% recall. Exact body hashes alone can't work because R8 rewrites
  most library bodies: inlining, residual-signature changes, moves.
- The call graph and class context do most of the work; they add about 38 points of recall.
- `strings` is the most productive seed. Its errors are mostly **inlining**: the caller now holds
  the callee's strings and matches the callee. (Such a match is still a useful inlining hint.)
- `nums` and `api` alone are too weak to seed.
- `proto`-alone matches are few but precise: rare framework-typed signatures.

### 4.2 Combined matcher per library (cascade A)

| lib | DB | same P | same R | same P(cls) | cross P | cross R | all-versions DB P | all-versions DB R |
|---|---|---|---|---|---|---|---|---|
| coroutines | d8 | 90.0 | 45.0 | 96.3 | 89.2 | 41.7 | 90.0 | 44.3 |
| coroutines | r8 | 93.1 | 57.5 | 96.3 | 92.1 | 55.1 | 92.8 | 57.1 |
| okhttp | d8 | 93.8 | 56.8 | 96.5 | 93.7 | 55.3 | 92.9 | 57.6 |
| okhttp | r8 | 93.9 | 67.7 | 95.7 | 93.4 | 66.2 | 93.0 | 67.2 |
| okio | d8 | 92.9 | 49.0 | 98.1 | 88.2 | 46.1 | 92.1 | 47.4 |
| okio | r8 | 87.9 | 54.4 | 92.1 | 84.4 | 51.0 | 87.7 | 52.4 |
| collection | d8 | 92.5 | 56.8 | 96.7 | 93.9 | 55.6 | 92.6 | 57.1 |
| collection | r8 | 92.5 | 68.0 | 95.8 | 92.1 | 65.9 | 93.4 | 68.6 |
| stdlib | d8 | 94.7 | 46.1 | 95.5 | 94.5 | 45.7 | 94.8 | 46.4 |
| stdlib | r8 | 93.2 | 56.8 | 94.4 | 92.9 | 56.4 | 93.0 | 56.8 |
| **ALL** | d8 | 92.4 | 49.2 | 96.4 | 91.6 | 46.9 | 92.1 | 49.1 |
| **ALL** | r8 | 92.8 | 60.2 | 95.3 | 91.9 | 58.2 | 92.5 | 59.7 |

The second slice (Java appB0/appB3, r8 DB) agrees: same-version 92.4–93.0 P / 61–62 R;
cross-version (okhttp 4.9.3 ↔ 4.12.0, okio 2.8 ↔ 3.6) 91.2–91.3 P / 58.7–59.4 R; all-versions DB
92.9–93.0 P / 61 R (`work/appB.json`).

### 4.3 Cross-version grid (combined, r8 DB): recall / precision, rows = app's version, columns = DB version

| coroutines | 1.7.3 | 1.8.1 | 1.9.0 | 1.10.1 | 1.10.2 |
|---|---|---|---|---|---|
| 1.7.3 | **57/93** | 55/93 | 53/91 | 52/91 | 52/91 |
| 1.8.1 | 57/92 | **58/93** | 55/92 | 55/92 | 55/92 |
| 1.9.0 | 55/91 | 55/91 | **57/93** | 57/93 | 57/93 |
| 1.10.1 | 54/91 | 54/91 | 57/93 | **58/93** | 58/93 |
| 1.10.2 | 54/91 | 55/91 | 57/93 | 58/93 | **58/93** |

| okhttp | 4.9.3 | 4.10.0 | 4.11.0 | 4.12.0 |
|---|---|---|---|---|
| 4.9.3 | **68/94** | 67/93 | 65/93 | 66/93 |
| 4.10.0 | 67/93 | **69/95** | 68/94 | 68/94 |
| 4.11.0 | 67/93 | 69/95 | **68/94** | 68/94 |
| 4.12.0 | 64/92 | 64/93 | 63/93 | **67/93** |

| okio | 2.8.0 | 3.0.0 | 3.2.0 | 3.6.0 |
|---|---|---|---|---|
| 2.8.0 | **56/88** | 51/84 | 51/84 | 51/84 |
| 3.0.0 | 52/84 | **54/88** | 53/88 | 50/84 |
| 3.2.0 | 51/84 | 53/87 | **53/87** | 50/83 |
| 3.6.0 | 52/85 | 51/84 | 51/84 | **54/88** |

| collection | 1.4.0 | 1.4.5 | 1.5.0 |
|---|---|---|---|
| 1.4.0 | **73/94** | 65/92 | 65/92 |
| 1.4.5 | 65/92 | **67/92** | 66/92 |
| 1.5.0 | 67/92 | 67/92 | **67/92** |

| stdlib | 2.1.21 | 2.2.21 | 2.3.21 | 2.4.20 |
|---|---|---|---|---|
| 2.1.21 | **56/93** | 56/93 | 56/93 | 55/93 |
| 2.2.21 | 56/93 | **57/93** | 57/93 | 57/93 |
| 2.3.21 | 56/92 | 57/93 | **57/93** | 57/93 |
| 2.4.20 | 56/92 | 57/93 | 57/93 | **57/93** |

Degradation grows with version distance (coroutines 1.7 ↔ 1.10: −5 R, −2 P), and is worst across
a major rewrite (okio 2 → 3). Adjacent minor versions are essentially free.

### 4.4 Where recall is lost (app3, same version; stratified by what R8 did, from the mapping)

| stratum | n | d8 DB P / R | r8 DB P / R |
|---|---|---|---|
| plain (no inlinees, same signature, not moved) | 735 | 98.5 / 61.5 | **98.8 / 67.6** |
| has inlined callees | 566 | 89.0 / 57.4 | 92.1 / 71.7 |
| residual signature changed (staticized/args removed/return changed) | 705 | 93.0 / 45.1 | 93.2 / 58.6 |
| moved / merged into another class | 246 | 90.9 / 32.5 | 86.3 / 41.1 |
| synthetic (R8/D8 synthesized classes/lambdas) | 290 | 76.8 / 25.2 | 76.3 / 36.6 |
| app-own code (false positives) | 76 | 1 FP | 0 FP |

- The R8-built DB helps every stratum. It already contains the residual shapes that R8's own
  optimizations produce: removed null-check intrinsics, inlined accessors, `$default` handling.
- The D8 DB carries about 10k Kotlin parameter-name strings (`checkNotNullParameter(x, "name")`)
  that R8 strips. Modelling that on the DB side (`d8n`) recovers only 2.3 of the 10.8 points of
  the gap (49.2 → 51.5 R), so a real R8 pass is worth it.
- Merged/moved code and synthetic classes are the weakest. They get better once 8R runs sigdb
  **after** its own un-passes (merge splitting, outline inlining).
- Synthetic-class naming differs between tool paths (`okio.Buffer$0` in the app vs
  `…$$ExternalSyntheticLambdaN`), so those "errors" are partly grading artifacts. Grade them by
  enclosing class.

### 4.5 Precision by provenance (combined, same-version, summed over 5 apps)

| stage | DB | matches | P(name) | P(class) |
|---|---|---|---|---|
| exact:all | d8 | 609 | 99.2 | 99.2 |
| exact:all | r8 | 1391 | 97.3 | 97.6 |
| callgraph | d8 / r8 | 405 / 817 | 100.0 / 99.3 | 100.0 / 99.9 |
| class:proto (unique erased proto in voted class) | d8 / r8 | 1792 / 2112 | 98.2 / 98.4 | 98.2 / 98.4 |
| class:mutual-best | d8 / r8 | 2446 / 2098 | 91.3 / 89.1 | 97.2 / 94.9 |
| exact:strings | d8 / r8 | 978 / 1111 | 82.9 / 88.8 | 93.8 / 92.9 |
| exact:ops+proto | d8 / r8 | 90 / 89 | 94.4 / 84.3 | 94.4 / 84.3 |
| exact:refs | d8 / r8 | 76 / 65 | 84.2 / 83.1 | 84.2 / 83.1 |
| exact:all-noproto | d8 / r8 | 42 / 121 | 88.1 / 70.2 | 100 / 78.5 |
| exact:api | d8 / r8 | 290 / 382 | 74.1 / 73.6 | 79.3 / 78.5 |

The precision/recall trade-off (r8 DB, 5 apps, `tune.py`):

| cascade | P(name) | R |
|---|---|---|
| A: all, all-noproto, strings, refs, api, ops+proto | 92.8 | 60.2 |
| **E: all, strings** (recommended) | **94.2** | **59.1** |
| B: all, strings, ops+proto | 94.2 | 59.2 |
| C: all only | 95.2 | 53.9 |
| B with mutual-best threshold 0.5/0.1 | 95.2 | 53.1 |
| B without mutual-best | 95.9 | 43.0 |

Cascade E on all 7 apps (`classlevel.py`, r8 DB): **94.2% P(name) / 59.4% R** at the same
version; **93.9% / 58.9%** with the all-versions DB. Full-signature precision is 92.8%.

Split by what R8 did to the name (`renamed_split.py`, cascade E, r8 DB, 7 apps, 15,073 library methods):

| group | share | P(name) | P(class) | R |
|---|---|---|---|---|
| renamed by R8 (the real name-recovery case) | 54.3% | 92.2 | 95.7 | 54.8 |
| `<init>` / `<clinit>` (name fixed; class recovered) | 20.1% | 96.6 | 96.6 | 67.1 |
| name kept by R8 (library overrides, kept members) | 25.6% | 96.2 | 98.2 | 63.1 |
| all | 100% | 94.2 | 96.6 | 59.4 |

### 4.6 Class-level recovery (cascade E, r8 DB, 7 apps, 3043 library-majority app classes)

- Majority vote over matched members, seeds and owner alignment: **P 88.6%, R 68.3%**.
- With ≥ 2 matched members and ≥ 2/3 vote share: **P 96.0%, R 49.1%**.
- Of the 268 wrong class picks, 173 are R8-merged classes. In 120 of those the pick is *one of
  the classes merged in*, which becomes correct after 8R's merge split. So name classes after the
  split.

### 4.7 Version identification

After matching against the all-versions DB, each matched method votes for the version(s) whose
variant is most similar (only when the versions differ). Pick the arg-max set.

| scoring | exact | truth in tie set | notes |
|---|---|---|---|
| similarity vote, both | 17/25 | 17/25 | coroutines 5/5 (1.10.1 vs 1.10.2 separated by 1 vote of ~43), okhttp 5/5, okio 4/5, collection 2/5, stdlib 1/5 |
| similarity vote, r8 | 17/25 | 17/25 | same pattern |
| similarity vote, d8 | 15/25 | 18/25 | |
| exact-equality vote | 12/25 | 18/25 | 3 empty (no discriminating method), 4 wrong |
| raw exact-hash presence (before matching) | 8–15/25 | | too few discriminating hashes |

The failures are libraries whose used slice (≈140 collection and ≈490 stdlib methods) is
byte-identical across the tested versions. There the version is unidentifiable from code, and
also irrelevant to naming (see 4.3).

Markers that are exact and cheap:
- `META-INF/kotlinx_coroutines_core.version` (`1.10.2`), which R8 kept among the app's resources;
- okhttp's `"okhttp/4.12.0"` user-agent literal, which survives in the dex;
- AndroidX AARs ship `META-INF/androidx.*.version` files.

Use markers first and voting second, and report a version *set*.

### 4.8 DB size, uniqueness, cost

- Per-library-version size, compact prototype encoding (key + 6×64-bit family hashes + u32 gram
  ids with counts), zlib: coroutines 234 KB (r8) / 272 KB (d8); okhttp 181/206 KB; okio 76–112 KB;
  collection 131–159 KB; stdlib 408–427 KB. All 20 lib-versions: 4.4 MB (r8) / 5.5 MB (d8).
- **Across versions, only 24–47% of (key, body) pairs are distinct.** So a versioned DB that
  stores a variant only when the body changes costs about 10–20% of a full version per extra
  version.
- A MinHash sketch (e.g. 32×u32) instead of the full gram multiset would roughly halve the size.
  I didn't test it.
- Uniqueness of the strict `all` hash among informative methods, across the whole universe (5
  libraries × all versions): 71% (d8) / 68% (r8). For `strings` it's 17.5% (d8, because of
  parameter-name strings) / 34% (r8). So ~30% of library methods are exact duplicates of another
  library method: small wrappers, `$default` stubs, delegating ctors.
- Matching cost (Python prototype, 2.6k app methods): 1.5 s against a single-version r8 DB (23k
  methods), 4 s against d8+r8 variants, a few seconds against all versions (≈100k variants).
  DB loading is cached. Everything is linear except mutual-best, which is O(|A_c|·|D_C|) per voted
  class pair. A Rust implementation should be well under a second per 100k-method app, per library.

## 5. Recommendations for 8R (§5.4)

1. **DB build (`xtask sigdb`)**. Per library version:
   - Run R8 (the pinned version, `--min-api` matching the common targets) on the library alone,
     with its consumer rules plus `-keep public class * { public protected *; }`. Dependencies go
     in as `--lib`.
   - Fingerprint the result using the mapping to get original keys. Names *may* be used on the DB
     side.
   - Store per canonical key (class, name, original proto): a version bitset and one record per
     distinct body. Each record holds the `all` and `strings` hashes, `ops+proto`/`refs` hashes
     for diagnostics, a gram sketch, erased callee protos with callee key indices, and program
     owners with erased tokens.
   - Per class, store C2/C3 hashes.
   - Record the R8 version and DB hash in the report. Validate against more R8 versions: only
     9.4.24 was tested, and R8 residue is version-dependent.
2. **Pipeline position**: sigdb goes after un-outlining and merged-class splitting, and before
   structural naming. Its matches feed `{hint}`, not S.
3. **Matcher**: the §3 algorithm with cascade **E (`all`, `strings`)**, the informativeness gate,
   C2/C3 class seeds, and conflict-free propagation rounds. Match against **all versions at once**.
   Seed from exact matches only; `api`/`nums`/`refs`-only seeds hurt precision.
4. **Labels**:
   - Every name from sigdb is **D**. Hint strength follows provenance:
     - *strong*: exact:all, callgraph, class:proto (98–99%);
     - *medium*: mutual-best, strings (≈89%).
   - Class names: require ≥ 2 agreeing members and ≥ 2/3 vote share (≈96% P). Otherwise emit a
     role-only hint.
   - **N** candidate sets only when there are ≤ 8 candidates. Report version sets the same way.
   - Nothing reaches S. There's no exhaustive candidate universe, duplicate bodies are common
     (~30%), and even the strictest stage is 97–99%, not 100%. This is consistent with audit §3.21
     and DESIGN §9.7.
   - **Version markers** (`*.version` resources, `okhttp/x.y.z`) are exact facts about the
     library version, but they don't make any member name S.
5. **Expected quality** on R8 9.4 apps with these libraries: about 94% precise / 59% recall for
   method names, about 96% precise for the class of a matched method, and 1–3 points worse when
   the exact version isn't in the DB (up to −5 R / −4 P across a major rewrite). Recall should rise
   after 8R's un-merge and un-outline passes, since moved/merged and synthetic code are the weakest
   strata. Precision is capped mainly by **inlining**: a caller whose body is mostly an inlined
   callee matches that callee. A sigdb-vs-callgraph conflict detector could turn these into
   inlining hints for §4.6.
6. **Oracle harness**: add `infer/sigdb-*` fixtures with the stratified P/R above, ratcheted like
   the inlining harness. Also fix or check the mapping oracle for R8 9.x relative synthesized
   outer frames (§1).

## 6. Caveats

- One R8 version (9.4.24), one min-api (24), one kotlinc (2.4.20).
- **CFG shape was not evaluated as a separate family.** The opcode sequence (with `goto` dropped
  and `if` polarity collapsed) and its 3-grams only approximate block structure. A real
  basic-block/dominator-shape hash is untested.
- Two app slices (Kotlin app ×5 versions, Java app ×2), all written for this study. Real apps use
  more of each library, which helps propagation and version ID.
- Five libraries. kotlinx-serialization-core wasn't run (it needs the compiler plugin in the app);
  the jars are in the 8R cache if someone wants to add it.
- Consumer-rule-kept names (volatile AFU fields, etc.) are a separate S anchor and weren't used;
  using them should add class seeds.
- The prototype's feature extraction parses `dexdump -d` text. 8R would use its own reader and IR,
  where normalization can be better, e.g. SSA-based shapes instead of opcode 3-grams.

## 7. Files

| path | purpose |
|---|---|
| `sigdb/apps/src/Main.kt`, `sigdb/apps/build.sh` | Kotlin test app + R8 build (per version set) |
| `sigdb/apps/srcB/b/Main.java`, `sigdb/apps/buildB.sh` | second (Java) app slice |
| `sigdb/apps/app{0..4},appB{0,3}/r8/{classes.dex,mapping.txt}` | minified apps + oracle mappings |
| `sigdb/db/build.sh`, `sigdb/db/{d8,r8}/<lib>-<ver>/` | reference DB builds |
| `sigdb/dexio.py` | dexdump parser, R8 mapping oracle parser (residual signatures, relative synthesized frames) |
| `sigdb/sigdb.py` | fingerprints, erasure, DB loader (`d8`/`r8`/`d8n`), class features, matcher |
| `sigdb/evalsig.py` | app/version table, grading |
| `sigdb/run_matrix.py` → `work/matrix-{d8,r8,both}.json`, `sigdb/report.py` → `work/report.md` | full matrix + tables |
| `sigdb/strata.py`, `sigdb/tune.py`, `sigdb/classlevel.py`, `sigdb/classthr.py` | stratified, cascade tuning, class-level |
| `sigdb/run_version.py`, `sigdb/run_version2.py` → `work/version-*.json` | version identification |
| `sigdb/run_B.py` → `work/appB.json` | second-slice evaluation |
| `sigdb/dbstats.py` | DB size, cross-version dedup, universe uniqueness |
| `sigdb/alpha_test.py` | α-invariance (order/rename independence) check of the matcher |
| `sigdb/renamed_split.py` | P/R split by renamed / ctor / kept names |
