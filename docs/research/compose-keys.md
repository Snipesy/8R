# Compose durable group keys as a library fingerprint (research, 2026-09-25)

Question: can Compose's durable group keys (the int constants passed to `startRestartGroup` / `startReplaceGroup` /
`startReplaceableGroup` / `startMovableGroup` / `sourceInformationMarkerStart`, and lambda keys passed to
`ComposableLambdaImpl` / `composableLambdaInstance` / `rememberComposableLambda`) identify **library** composables in an
R8-minified app, recover their names, and resolve the library version?

**Short answer: yes for restartable library composables, not for the rest.** A restartable composable's entry key
`KEY_fn` survives R8 as the **first int constant of its residual method** (96/96 reachable ones in the probe app). It has no
genuine hash collisions in a DB of about 7k keys, and it is stable across library versions apart from two explainable
breaks. Non-restartable and inline library composables (Column, Row, Box, Layout, remember, …) leave **no key** after R8,
because the Compose 1.7+ compiler emits only a `sourceInformationMarkerStart` for them, and consumer rules strip that call. Inner and lambda keys pin
the **minor** version. Patch versions have identical key sets.

Everything lives under `/home/snipesy/.claude/jobs/ca53d1c2/tmp/research/compose-keys/`. No tracked 8R file was changed.

---

## 1. Method

### 1.1 Artifacts (all real, from dl.google.com)
`fetch.py` downloaded the `-android` AAR (for material3 1.1.2, the plain AAR) and extracted `classes.jar` to `jars/`.

| group | versions |
|---|---|
| runtime, ui, ui-text, foundation, foundation-layout, material, material-ripple, animation, animation-core | 1.5.4, 1.6.8, 1.7.8, 1.8.3, 1.9.5, 1.10.0, 1.10.6, 1.11.0, 1.11.4, 1.12.0, 1.12.1 |
| material3 | 1.1.2, 1.2.1, 1.3.2, 1.4.0 (the last stable; 1.5.0 is alpha only) |

That is 103 jars. Pairs like 1.10.0 / 1.10.6 are there to measure patch-level stability.

### 1.2 Extractor
`ext/KeyExtract.java` uses the ASM that is shaded inside kotlinc 2.4.20's `kotlin-compiler.jar`, with `Analyzer<SourceValue>`, so each
key is the exact constant that feeds the call's key argument (no guessing from the preceding instruction). For every method
that takes a `Composer` or calls a group API, it records:
`owner, name, desc, access, #$changed (from LVT params), composable?, events`. Events are in instruction order:
`RG` startRestartGroup, `RPG` startReplaceGroup, `RPLG` startReplaceableGroup, `MG` startMovableGroup, `RUG`
startReusableGroup, `MKS` sourceInformationMarkerStart (+ its string), `TES` traceEventStart (+ info string), and `LAM`
(ComposableLambdaImpl.<init>(IZ…) / composableLambdaInstance / rememberComposableLambda / composableLambda), each with its key.
`$default` is not in the LVT, so #$default = trailing ints after `Composer` − #$changed.
Output: `tsv/<artifact>-<ver>.tsv` (about 22k rows).

**Entry key** = the key of the first `RG|RPLG|RPG|MKS|TES` event. The **entry kind** says what kind of function it is:
`RG` = restartable. `RPLG` = pre-1.7 compiler, non-restartable. `MKS` = 1.7+ compiler, non-restartable or inline. `TES` first =
composable lambda body, whose key is the LAM key at the construction site. Sanity check: in all **4,294** methods that have both RG and
TES, TES key == RG key (0 mismatches).

"Survivable" keys are those on calls that R8 cannot strip: `RG, RPG, RPLG, MG, RUG, LAM`. `MKS` is always stripped by the runtime
consumer rule (`-assumenosideeffects`). `TES` goes away with it when no tracer is installed (compose.md §2).

Scripts: `load.py` (TSV loader), `norm.py` (source-level identity: merges `$$inlined$` copies, lambda-class and indy forms,
`$module` suffixes, `-mangling`), `analyze1.py` (entry kinds), `analyze2.py` (version stability), `collide.py` /
`collide2.py` (collisions, DB), `why.py` (recomputes key strings), `dexparse.py` (dexdump and mapping parser),
`r8probe2.py` (R8 survival with the mapping as oracle), `gretio.py` (real app, no mapping), `version.py` (version resolution).
Outputs: `out_*.txt`, `db_surviving.json` (K → [(fn, role)]), `db_versions.json` (K → versions).

---

## 2. Which composables even have a survivable entry key

`out_entrykinds.txt` classifies composable functions (non-lambda, with an entry group):

| artifact ver | fns | RG (restartable) | RPLG entry (old compiler) | **MKS-only (key lost after R8)** |
|---|---|---|---|---|
| runtime 1.5.4 → 1.12.1 | 66 → 74 | 33% → 31% | 41 → 2 | 5% → **66%** |
| ui 1.5.4 → 1.12.1 | 57 → 55 | 44% → 42% | 20 → 1 | 21% → **53%** |
| foundation 1.5.4 → 1.12.1 | 134 → 224 | 45% → 49% | 72 → 0 | 1% → **37%** |
| foundation-layout 1.5.4 → 1.12.1 | 56 → 59 | 4% → 14% | 53 → 0 | 2% → **83%** |
| animation-core 1.12.1 | 53 | 6% | 0 | **77%** |
| material 1.5.4 → 1.12.1 | 251 → 272 | 51% → 53% | 116 → 0 | 3% → **27%** |
| material3 1.1.2 / 1.2.1 / 1.3.2 / 1.4.0 | 513 / 568 / 594 / 767 | 42 / 45 / 46 / 49% | 282 / 294 / 0 / 0 | 3 / 4 / 48 / **46%** |

The switch happens at compose 1.7 and material3 1.3, when those libraries started being built with the newer compiler. The old compiler wrapped non-restartable
functions in `startReplaceableGroup(KEY_fn)`, which survives R8. The new one emits only `sourceInformationMarkerStart(KEY_fn)`, which
R8 deletes. For current libraries, **roughly half of library composables (all inline and non-restartable ones: Column, Row,
Box, Layout, remember*, animate*AsState, …) cannot be fingerprinted by KEY_fn at all.** Their inner `RPG` / `LAM` keys, when
they have any, remain.

Inline composables go further: the Kotlin compiler copies their keys into every caller. Column's `-483455358` (an RPLG
entry in 1.5.4) appears as an "inner" key in dozens of material functions. In new versions it is an MKS, so it is gone after R8. In
the probe app, the 9 app-side key constants lost by R8 were exactly the MKS markers of Column/Row/Box/Layout/ReusableComposeNode
(plus one mask).

---

## 3. Cross-version stability (consecutive pairs; `out_stability.txt`)

RG = restartable functions present in both versions under the same (owner, name, desc).

| pair (compose) | RG common → same K | note |
|---|---|---|
| 1.5.4→1.6.8 | foundation 50/50, material 120/123, ui 21/21, animation 13/13 | 3 material changes = file renamed `X.kt → X.android.kt` |
| 1.6.8→1.7.8 | 100% in all artifacts | |
| 1.7.8→1.8.3 | foundation 66/67, material 131/134 | a function renamed (`BasicTooltipBox → BasicTooltipBoxAndroid`), file moved back |
| **1.8.3→1.9.5** | foundation 54/83, material **39/138**, ui 9/20, animation 0/16, runtime 1/4, f-layout 1/8 | **K1→K2 compiler switch** |
| 1.9.5→1.10.0, 1.10.x→1.11.0, 1.11.4→1.12.0 | 100% everywhere | |
| 1.10.0→1.10.6, 1.11.0→1.11.4, 1.12.0→1.12.1 | 100%, and **surviving key set identical (100%)** | patches are key-identical |
| material3 1.1.2→1.2.1 | 162/164 | 2 = file renamed to `.android.kt` |
| material3 1.2.1→1.3.2 | 191/191 | |
| **material3 1.3.2→1.4.0** | **73/220** | **K1→K2** |

**Why the keys changed although name and desc did not.** I verified this by recomputing `String.hashCode` in `why.py`:
- **K1→K2**: K2 names a composable function type `ComposableFunctionN`, while K1 used `FunctionN` of the *pre-lowering* arity.
  In every changed function in the K2 transition, the descriptor has a `kotlin/jvm/functions/Function*` param (material 99/99,
  foundation 29/29, material3 145/147). Example: material3 `Button`:
  `fun-Button(Function0,Modifier,Boolean,Shape,ButtonColors,ButtonElevation,BorderStroke,PaddingValues,MutableInteractionSource,<T>)Unit/pkg-androidx.compose.material3/file-Button.kt`
  gives `Function1` → **650121315** (the 1.3.2 key) and `ComposableFunction1` → **−1310015664** (the 1.4.0 key). Functions with only
  non-composable function params (such as `onClick: () -> Unit`) kept their keys.
- **File basename** is part of the key, and the JVM facade is not. For material `ExposedDropdownMenuBox`, `file-ExposedDropdownMenu.kt` →
  1456052980 (1.5.4, and again in 1.8.3 when the source moved back to common with the `_androidKt` facade kept) and
  `file-ExposedDropdownMenu.android.kt` → −617870381 (1.6.8, 1.7.8). `TrailingIcon` followed the same file:
  876077373 / 1752693020.
- A renamed function changes its key (`BasicTooltipBox → BasicTooltipBoxAndroid`).

Inner keys (`RPG`/`RPLG`/`MG`) depend on source character offsets, so any edit earlier in the file changes them. Across minor
versions only 7–100% (typically 40–80%) of the survivable key set is kept, and across patches 100%. That makes them bad for identity
and good for **version resolution**.

**Consequence:** entry keys can be **computed** from a source signature. So a DB built from a few versions per compiler generation
covers the versions in between. One DB entry per (function, K1 or K2 variant, file name) is enough. There is no need to store every version.

---

## 4. Collisions and DB size

- Distinct entry keys over all 103 jars: **3,837**. Distinct RG entry keys: **1,241**.
- **Genuine hash collisions among RG entry keys (same K, different source function): 0.** The one multi-mapping
  (1456052980) is the same function on both sides of a file move. Birthday expectation: 1241²/2³³ ≈ 0.0002.
- Entry keys shared between different normalized functions within one version: 3. All are `ReusableComposeNode` vs
  `ComposeNode` in runtime ≤1.7.8. Both are inline and both start with the same inner replaceable group (2058660585), so this is a
  shared body key, not a hash clash.
- **Survivable-key DB** (all roles, all versions): **7,060 distinct keys, 8,507 (K, fn, role) rows**, about 820 KB as JSON
  (`db_surviving.json`), or about 100 KB packed. Roles: 4,991 inner, 2,119 entry, 1,397 lambda. Keys that map to more than one normalized function: 70.
  All of them are explained by:
  (a) **inline-body propagation**: the keys of animateValue / animateFloat / Crossfade / Layout / remember / Row / Column / Box /
  ReusableComposeNode appear as "inner" in every caller;
  (b) lambda class vs indy `$lambda$N`, and `ComposableSingletons` lambda renames;
  (c) file moves;
  (d) internal-module suffixes.
  None is a hash coincidence. Case (a) is useful evidence in its own right: "this method contains an inlined call to X".
- Low-entropy keys: only 2 of 7,399 have |K| < 2²⁰ (`207`, `-380568`). Exclude |K| < 2²⁰ from bare-constant matching.

---

## 5. R8 survival (probe app with the mapping as oracle)

The probe is `app/src/com/example/app/{Entry,Main}.kt`. It uses the same `Entry.start()` + `Composition(UnitApplier, Recomposer)` shape as
`fixtures/src/compose_basic`, and the screen uses MaterialTheme, Scaffold, TopAppBar, FAB+Icon, Column/Row/Box, Text, Checkbox,
Switch, Spacer, Button, OutlinedButton, Slider, OutlinedTextField, HorizontalDivider, AnimatedVisibility+Card,
CircularProgressIndicator, LazyColumn/items/ListItem, and remember/mutableStateOf.
`app/build.sh` mirrors `xtask`: kotlinc 2.4.20 + compose plugin, D8 `--debug` (the app-only oracle), then R8 **9.4.24** `--release
--min-api 24` with libraries as **program input** plus every AAR `proguard.txt` / jar R8 rules, `-dontwarn **`, `-ignorewarnings`.
Libraries (`app/fetchlibs.py`): compose 1.10.6 (runtime, saveable, ui*, foundation*, animation*, material-ripple),
material-icons-core 1.7.8, material3 1.4.0, plus core, lifecycle, savedstate, autofill, collection, annotation and coroutines.
The output is a 1.8 MB dex with 18,409 residual methods.

Results (`r8probe2.py` → `out_r8probe.txt`; DB restricted to the exact versions used):

- **Reachable restartable library composables** (the original method appears in the mapping): 96, excluding
  `ComposableLambdaImpl.invoke`, which has no constant key. **96/96 keep KEY_fn as a `const` in a residual method whose outermost
  original is that function, and in all 96 it is the method's first int constant.** 0 were inlined by R8. They have at least 2
  call sites: the caller and their own restart lambda. (The script prints 6 as "inlined". That is a parser artifact: the mapping repeats
  the outer frame in short-type form. Manual check, e.g. `SwitchImpl` in `b31.b`, shows they are residual.)
- Every DB-key constant found in the R8 dex: 348 occurrences with |K|≥2²⁰. **347 attributed** to a function whose class is in
  that method's frames. 1 "unattributed" is 802480018, the key of `LazyDsl.items`' content lambda. It sits in *app* code because
  `items` is inline, so it is correct evidence too.
- **Shape after R8:** the Composer interface is merged into `ComposerImpl` (`Ldj;` here) and shows up as one class type. 48/90
  residual signatures changed their parameter count. Examples: `Text-Nvy7gAk` 18+1+2+1 → 14 params; `OutlinedTextField`
  23+1+3+1 → 15; `ListItem` 9+1+1+1 → 5; `LazyList` 14+1+2+1 → 12. R8 drops constant and unused params, often `$default`. So
  arity is only an **upper bound**.
- App-side: 23/32 app key constants survive. The 9 lost are all MKS markers of inline Column/Row/Box/Layout + 1 mask. One app
  constant (802480018) is in the library DB, and that is the legitimate `items` lambda key.

Composable lambda keys also survive: `new ComposableLambdaImpl(K, Z, λ)` in `<clinit>` and at call sites. In the probe, 70 LAM
constants were found with correct attribution.

---

## 6. Real app: Gretio `~/Downloads/10804000.apk` (R8 9.4.24, no mapping; α-invariant counts only)

`gretio.py` → `out_gretio.txt`, over 2 dex files, 138,575 methods, 172,197 int-const instructions (10,712 distinct values, 6,520 with |v|≥2²⁰).
The composer class was found α-invariantly as the type most often followed by trailing `I` params (`Lbl4;`: 579, next best `Parcel`: 263).

| layer | count |
|---|---|
| (a) const occurrences equal to any DB key | 766 (727 distinct keys) |
| (b) after excluding abs(K) < 2²⁰ | 752 (726 distinct) |
| (d) distinct matched keys by DB role | entryRG 194, entry RPG/RPLG 40, inner 346, lambda 154 |
| (c) **entry-position** (first int const of a method that takes `Lbl4;`) | **198 distinct**. Roles: 187 entryRG only, 5 entryRG+RPLG, 3 inner, 3 entry-RPG |
| entry-position keys with more than one candidate function | 1, and it is not an entryRG key. **All 194 RG entry keys found anywhere in Gretio are the constant argument of the method's first `C.x(I)C` call (startRestartGroup), with 0 ambiguous.** 192/194 are also the literal first int constant. In the other 2 (`SwipeToDismissBox`, `PullToRefreshBox`), R8 hoisted a `const/16 54` (for `Integer.valueOf`) ahead of the key. |
| distinct library functions named via entry RG keys | **167** (material3 108, foundation 51, ui 13, animation 11, f-layout 4, animation-core 3, runtime 2) |
| shape check (order-insensitive needed!) | 182/192 pass the naive "C then trailing ints" test. The 10 failures are **R8 parameter reordering** in Gretio, e.g. `LazyList` → `(IILze;…Lbl4;…Z)V` with the ints moved *before* C. So shape tests must use multisets (counts), not positions. |

Expected random matches: 6,520 distinct large app constants × 7,060 DB keys / 2³² ≈ **0.011**. For entry-position against RG keys
only: 574 × 1,241 / 2³² ≈ **0.00017**. At a hypothetical 50k-key DB (many more versions and libraries): 0.076 and 0.0012.

---

## 7. Version resolution

`version.py`: take the functions identified by their stable RG entry key. For each version v, take the full survivable key set
E_v of those functions (their own bodies plus their lambdas), and score each version by the fraction of E_v present in the app.

Probe (ground truth compose **1.10.6**, material3 **1.4.0**; `out_version_probe.txt`):

| artifact | 1.9.5 | **1.10.0 / 1.10.6** | 1.11.x | 1.12.x |
|---|---|---|---|---|
| foundation | 48% | **65%** | 61% | 59% |
| ui | 73% | **92%** | 70% | 70% |
| animation | 25% | **53%** | 25% | 5% |
| animation-core | 33% | **100%** | 67% | 67% |
| material3 (1.1.2 / 1.2.1 / 1.3.2 / **1.4.0**) | 4 / 5 / 7 / **57%** | | | |

Gretio (`out_version_gretio.txt`): animation 1.12.x **88%** (vs 50% for 1.11), foundation 1.12.x **76%** (67% for 1.11),
ui 1.11–1.12 85%, material3 1.4.0 **76%** (≤8% for the others). So Gretio is compose **1.12.x** and material3 **1.4.0**
(or a later version not in the sample).

Resolution works at **minor-version granularity**. Patch releases in the sample had identical key sets, so they are
indistinguishable, and for naming purposes they do not need to be told apart. Recall at the true version is below 100% (57–100%) because R8 removes
dead branches and unreachable lambdas, and E_v includes lambdas that were never reached. So use the argmax, not a threshold. A version
that is not in the DB scores partially on its neighbours, so the DB should sample **every minor** (the last patch of each is enough).

---

## 8. Proposal for `compose/lib-key`

**Match condition (effectively unique):**
1. Composer class C is found α-invariantly: the class type most often followed by trailing int params, or pinned by a runtime SigDB.
2. Residual method M takes a C param, and **K is the constant argument of M's first `C.x(I)C` invoke** (startRestartGroup). This held in 96/96 probe cases and 194/194 Gretio cases. "First int constant of M" is a weaker version that failed 2/194 in Gretio.
3. abs(K) ≥ 2²⁰ and K ∈ DB with role **entryRG** and exactly one normalized function F.
4. **Order-free shape** consistent with F: `|params(M)| ≤ arity(F) + [this] + 1 + nch + ndef`; `#int params(M) ≥ 1`
   (the `$changed` slot survives in all observed cases); the reference-type params of M are a sub-multiset of F's
   (by count). Arity is only an upper bound, because R8 drops params, and R8 may reorder them, so never test positions.
5. Corroboration (turns D into S): at least one more key of F's DB body (an RPG / MG / LAM key of F, or the LAM key of F's content
   lambda) occurs in M or in a lambda class that M instantiates. Alternatively, F's DB owner file has more than one identified function in the same app.

With conditions 1–4 the expected false-positive count is about 2·10⁻⁴ per app (Gretio numbers), about 10⁻³ at a 50k-key DB. With 5, it is
negligible. Conditions 1–4 alone are a strong D hint. With 5 it qualifies as S for the **method name**.

**What is recovered:** the original method name, the original owner class, and the Kotlin-level signature (from the DB entry) for
the report and sidecar. **Do not** rename M's *host* class after F's owner. R8 moves restartable composables into unrelated host
classes: `LazyList` ended up in the class mapped from `Key_androidKt`, and `SwitchImpl` in `b31`. The owner is provenance, not
the host's name.

**Version:** per artifact, take the argmax over minor versions of the recall of identified functions' full key sets (§7). Report it as
"library X ≈ minor M.N". Use the version to choose which DB rows are canonical when K1 and K2 variants both exist.

**DB construction:** scan AAR `classes.jar` files as `KeyExtract` does. No hashing is needed. Store `K → (owner, name, desc, role,
first/last version seen)`, plus per-version inner / LAM key sets for version resolution. Sample the last patch of every
minor of each artifact. The K1→K2 boundary (compose 1.9, material3 1.4) doubles the entries only for functions with
composable-lambda params. Size is about 7k keys for 9 artifacts across 11 compose minors and 4 material3 versions (about 100 KB
packed).

**Out of reach (report only):**
- Non-restartable and inline library composables built with compiler ≥1.7 (about 30–80% per artifact). Their KEY_fn is an MKS and R8
  strips it.
- Their inline-copied inner keys, when they survive (pre-1.7 RPLG, or LAM keys such as `items`), identify **inline call sites**
  inside app methods ("inlined Column / items here"). That is a D hint for the report, not a rename.
- `compose/group-offset`-style inversion of inner keys is not needed for libraries, because the DB has them verbatim.

---

## 9. Caveats
- The DB covers 9 compose artifacts plus material3. `ui-text`, `ui-graphics` and similar have almost no composables. Icons have none.
  Other Compose-using libraries (navigation-compose, activity-compose, lifecycle-runtime-compose, coil, accompanist) are
  not covered. Gretio's unexplained app keys are presumably those libraries or the app itself.
- Only one probe app (compose 1.10.6) and one real app were measured. "K feeds the first `C.x(I)C` call" held 96/96 and 194/194.
  "K is the literal first constant" failed 2/194 in Gretio, where R8 hoisted a small constant above it.
- The 1.12.x AARs are 20–70% smaller than 1.11.x (animation-core 1.37 → 0.43 MB). Even so, they yield as many or more extracted rows, and RG keys are
  retained 100% from 1.11.4 to 1.12.0. I did not investigate the packaging change.
- The R8-inlined-restartable case was not observed: 0/96 in the probe, and in Gretio every one of the 194 RG keys sits at the entry of a method that takes C. It is still possible for a restartable function with a single call site whose restart
  lambda R8 manages to fold away. The mid-body key would then show up in the caller, and the DB role check (entryRG found mid-body)
  should classify it as "inlined F here".
