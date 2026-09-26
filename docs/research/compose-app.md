# Compose in app code after R8 9.4: measurements on Gretio, a validated rule plan, and corrections to compose.md

Status: research notes (2026-09-25). Inputs: `fixtures/out/compose_basic{,_r94}/{d8,r8}` (kotlinc 2.4.20, runtime 1.10.6,
R8 8.10.9 / 9.4.24) and **Gretio** `~/Downloads/10804000.apk` (R8 9.4.24 full mode, min-api 26, compose runtime/ui
**1.12.1**, material3 1.4.0, androidx.tracing 2.0.0, no runtime-tracing). Mappings were used only as oracles.

All numbers come from a structural analysis tool (no names, no mapping) built on 8R's IR (`eightr-ir`: lift, Cfg,
ReachingDefs). The tool and raw outputs are in `research/compose-app/`:

| Path | Contents |
|---|---|
| `ctool/src/main.rs` | the analyser (`ctool OUT a.dex …`): Composer identification, restartable detection, restart lambdas, roles, masks, slot/default bindings, keys |
| `r-gretio/`, `r-fx94/`, `r-fx810/`, `r-gretio-8r/` (8R output), `r-lib/` | `report.txt`, `composables.tsv` (one row per restartable), `keys.tsv` |
| `lib/dex/*` | D8 (`--no-desugaring`) of 25 cached Compose AARs (runtime/ui/foundation/animation 1.10.6, material3 1.4.0, activity/lifecycle/navigationevent/savedstate-compose) = the **library ground truth** |
| `summarize.py`, `libtruth.py`, `bindcheck.py` | the summaries quoted below |

**Library ground truth.** 201 of Gretio's 552 restartable composables have a group key that equals a key in the
library DB. The durable key hashes the simple name, the simple type names of all *original* parameters, the class
chain, the package and the file, so a match gives the pre-R8 Kotlin signature. The chance of an accidental 32-bit
collision is about 710·552/2³², roughly 10⁻⁴. These 201 methods let us check R8 effects and rule precision on the
real app without its mapping. The rules below are checked twice: on the fixtures (by mapping) and on these 201
methods.

---

## 1. Gretio measurements

### 1.1 Detection (fingerprint 1): works, with a clear margin
- **Composer = `Lbl4;`**, found as the class `T` with the most calls `T.x(I)T` made with a constant on a parameter
  of type `T`. There are 552 such calls, and the next candidate (`String.substring`) has 15. `startRestartGroup` is
  `bl4.d0`. `Composer` was merged into `ComposerImpl`: `bl4` has super `Object` and no interfaces.
- `endRestartGroup` = `bl4.u()Lh19;`. It is followed by `if-eqz` and **`iput-object lambda, h19.d`** at 584 of ~590
  `u()` sites (the rest are longer windows or runtime-internal uses): R8 9.4 inlined `RecomposeScopeImpl.updateScope`. No `updateScope` call is left.
- `updateChangedFlags` = the static `y58.B(I)I`, found by the masks `0x12492492`/`0x24924924`. It is **never
  inlined**: 596 restart-lambda call sites call it.
- `ComposableLambdaImpl` = `ft1`: `<init>(IZ…)` is called with a const int and a const bool at 259 sites. 152 of them
  are in `<clinit>` (singletons, `tracked=false`) and 107 are inlined `rememberComposableLambda` sites
  (`tracked=true`).
- `shouldExecute` = `bl4.S(IZ)Z`. **R8 reordered the parameters of this runtime method**: it is `v(ZI)Z` in the
  fixture. The runtime API has to be found by shape, never by descriptor.
- **8R today does not detect Compose on Gretio.** `8r info` lists R8, kotlinx-serialization and kotlinc, but no
  Compose, because `sources.rs` looks for a reference to `Landroidx/compose/runtime/Composer;`, and R8 renamed it.

### 1.2 Population
| | count |
|---|---|
| methods with code | 76,268 |
| methods with a Composer-typed param | 785 |
| **restartable** (`startRestartGroup(const)` on the Composer param) | **552**, all keys distinct |
| … of which match the library key DB | 201 (material3 110, foundation 55, ui 14, animation 10, other 12): a lower bound on library composables, since revenuecat-ui, constraintlayout-compose and material3-adaptive are not in the DB and ui is 1.10.6 vs 1.12.1 |
| … not in the DB (app plus uncovered libraries) | 351 |
| non-restartable with a Composer param | 167 (36 contain `startReplaceGroup`). 94 have the Composer **last**: `$changed` removed. **No function key survives** (the markers are stripped and there is no outer group with OptimizeNonSkippingGroups) |
| composable-lambda / other bodies that `check-cast` to the Composer | 534 (213 call `shouldExecute`) |
| `startRestartGroup` not at a composable entry (R8-inlined restartable) | **0**. 376/552 are literally the first instruction; the rest follow hoisted consts, null checks or field reads. The result is **never** moved back (no `move-result`) |

### 1.3 Surviving strings: none
- `sourceInformation` / `C(…)` / `N(…)` / `P(…)` strings: **0**. The only `"C("` and `"CC("` constants belong to
  ui-tooling-data's parser.
- Trace strings `"fq (File.kt:N)"`: **0**. There is no tracer, so R8 folds `isTraceInProgress()`.
- `ComposableSingletons` / `lambda$` / `.kt` strings: 0. No field named `$stable` survives. R8 renames fields anyway, so this does not verify that the fields were removed.
- Side evidence for `kotlinc/`: 452 Kotlin function-reference signature strings survive, including
  `"DashboardPages$lambda$33$pageShiftOf(Landroidx/compose/foundation/pager/PagerState;FF…)I"`. This names an app
  composable (`DashboardPages`) and its lambda index, and it is the only app-level name evidence for composables
  that was found.

So every §3 rule that depends on strings yields **nothing** on Gretio. The evidence that survives is the int
constants (keys and masks) and the dataflow.

### 1.4 Roles by dataflow
Two independent signals were used.
- **(a) Restart-lambda callback.** In the class instantiated after `endRestartGroup`, the call back to `M`.
  - The argument at the Composer position is the lambda's own Composer arg (a `check-cast` of an `invoke` param).
  - A `$changed` argument is the result of `updateChangedFlags(x)`: `ucf(x|1)` or `ucf(const)` when R8 folded
    `$changed|1`.
  - A captured parameter or `$default` is an instance-field read, or a constant when R8 propagated it.
- **(b) In-method dataflow.** `$changed` feeds slot masks, `&1` into `shouldExecute`, and `6<<3s`/`14<<3s` tests.
  `$default` feeds single-bit tests that select the default value.

| | result |
|---|---|
| restart lambda found | **552/552** (for 38 restartables, the lambda class has >1 call back to `M`: a restart lambda merged with a content lambda that calls the same composable, e.g. `z44`; the right site is the one with `ucf`) |
| `$composer` = lambda's own Composer arg | 552/552 |
| `$changed` via `ucf` | 552/552 methods have ≥1 (508 with one, 44 with two); 596 ints; 256 of them with a folded const argument |
| (b) agrees exactly with (a) on the `$changed` set | 480; (b) misses one in 66; disagrees in 6. **Dataflow alone ≈ 87 %; the lambda signal is decisive** |
| `$default` (lambda pass-through int with single-bit tests) | 95 methods |
| **Library truth (201):** `$changed` never over-counted, `$default` never over-counted, no residual param misclassified as synthetic | 0 errors |

Bit 0 is ambiguous. `$changed & 1` (force) and `$default & 1` (param 0 has a default) are the same instruction
(`ik8.g`: `and-int/lit8 v0, v8, #1` where `v8` is `$default`). Only the lambda signal, or "feeds `shouldExecute` /
`$dirty`" as against "selects a value", tells them apart. `compose.md` R-roles treats `x&1` as `$changed` evidence,
which is wrong for `$default`.

### 1.5 What R8 did to composable signatures (library truth, n = 201)
| effect | count |
|---|---|
| ≥1 real param removed | **80 (40 %)**. Removed per method: 1×39, 2×19, 3×16, 4×1, 5×5 |
| a `$changedK` int removed (constant) | 11 (5.5 %), e.g. `TextField` `IIII` → `II`; `ModalBottomSheet` `III` → `I` |
| `$default` removed entirely (constant at every call site) | **73 of 120 (61 %)** that had one |
| residual params not in canonical order `real* C ch+ df*` | 11 (all 552: 26, e.g. `ComposeView.a(ILbl4;)V`, and fixture `Greeting (IILq0;Z)V`) |

Fixture `compose_basic_r94` shows the extreme case. `Greeting(name,times=1,enabled=true)` becomes `(IILq0;Z)V`:
- `name` is constant-propagated into `"Hello a "`, and `$default` is removed.
- The defaults prologue became unconditional on the execute path.
- The restart lambda `c4` stores `times` in a **boolean** field.
- `Many` loses a1..a10: `(ILjava/lang/String;Lq0;III)V`.

### 1.6 New evidence: parameter-to-original-position binding (validated)
- **Slot binding.** `if (($changed_q & (6<<3s | 14<<3s)) == 0) dirty |= composer.changed(p) ? 4<<3s : 2<<3s` ties
  the residual param `p` to original slot `10·k(q) + s`.
- **Default binding.** `bit = $default & (1<<i); … v = bit ? <default> : p` ties `p` to original **regular** index
  `i`. After R8 this is a phi: one arm defines `r`, and the other arm does `move r, p`.

| check | result |
|---|---|
| D8 library (unminified = exact truth), slot binding | static 3034/3058 (99.2 %); **members 363/365 only if the dispatch receiver is the LAST slot** (see §4 C1) |
| D8 library, default binding | 3223/3425 (94 %) match with `p − i` ∈ {0,1} = #extension/context receivers, 3000 with shift 0. The error mode is duplicate bindings from overlapping windows (`1#1 1#2`). `slot − bit` **reveals the receiver count**, which resolves compose.md §5.3 |
| Gretio vs library truth: types at bound positions | slot 503/521 (96.5 %), default 158/160 |
| Fixture r94 | `Many`: p0@slot0/q=`$changed1`, p0#bit10, p1#bit11 → a11/a12 at original 10/11 ✓; `Label`/`Screen` ✓ |
| Gretio coverage | 1232 of 2671 residual real params (46 %) placed at their exact original position; 134 methods fully placed; 93 with default bindings |

The **arity lower bound** is the largest of:
- `10·(c−1)+1` for `c` `$changed` ints;
- the top slot of the skip mask (also `const`+`and-int/2addr` masks such as `0x12492493`);
- the top default bit + 1, plus the receiver.

On the library truth this bound is **sound: 0 violations in 201**, and it detects a removal in 57 of the 80 methods
that truly lost params. On Gretio, **109/552 composables provably lost params.** Fixture: Greeting gap 1, Many gap 10,
both exact.

### 1.7 Lambdas, singletons, merging
- **Restart-lambda classes.** 290 distinct classes. 199 serve one composable; **91 are horizontally merged groups**
  serving 2–23 composables. Their constructors are `(Object…, I, B)`: byte class id, captures typed `Object`.
  Groups mix restart lambdas with composable-lambda bodies and ordinary `Function2` lambdas.
- **After 8R Phase 2** (`r8/split-merged-class`, 1274 splits on Gretio), the restart lambda sits in its own
  specialized subclass for only 404 of 552 restartables. For the other 148, the call back stays in an unspecialized
  switch in the base class. **281/1274 splits specialize 0 methods.**
- Cause, verified on `pe` and likely general: `merged.rs::is_this` (line 135) requires the id read to use the `this` register itself. In big merged
  `invoke`s, R8 copies `this` first (`move-object/from16 v0, v20; iget-byte v1, v0, …`; e.g. `pe.invoke`), so
  dispatch is not detected. This is an `r8/` bug with a direct Compose payoff.
- **Composable-lambda block classes:** 76, of which 27 are merged groups that host most of the bodies (`ut1`/`kt1`
  with 30 sites each).
- **Singletons.** 152 `new ComposableLambdaImpl(K,false,block)` + `sput` in 55 `<clinit>`s. The hosts are
  **mixed classes**, e.g. 25 static fields plus other methods; only 1 host is a pure holder. `ComposableSingletons$FileKt`
  is **not recoverable as a class** after R8 9.4. The field name `lambda$K` is: the fixture's
  `ComposableSingletons$EntryKt.lambda$-402384942` matches its `const #e80417d2`.
- **Fixture hosts.** `MainKt` composables are hosted in `kotlin.jvm.internal.TypeIntrinsics` (`c9`). The instance
  composable `Screens.Member` lives in `ContinuationInterceptor$Key` (`y0`), which also absorbed `Holder` and
  `Wrapper`. The singletons field is in `CollectionsKt__CollectionsKt`.

### 1.8 Keys
- All 7 fixture keys recompute exactly from `fun-Name(SimpleTypes)Unit[/class-X]/pkg-…/file-….kt`, including
  `Many`, whose key lists all 12 **original** parameter types although only 2 survive.
- On Gretio, keys are usable **only with a candidate set**, and for app composables there is none: no strings. The
  library DB is the key payoff. A match gives the original name and signature, plus the original type of every bound
  residual param. That gives class names for `uu6` = `Modifier`, `l0a` = `Shape`, `ti4` = `Function2`,
  `bl4` = `Composer`…, which feeds the `r8/` class naming.

---

## 2. What "regenerating" app composables can achieve (Gretio, R8 9.4)

| Output | Class | Gretio yield | Where it goes |
|---|---|---|---|
| `$composer` param name | **S** | 552 restartables (plus lambda bodies: the `check-cast` target of `invoke` arg 1, D) | debug-info param names |
| `$changed` / `$changed1` names | **S** role; **K index D** (residual order) unless a default binding and a slot binding meet on one param (then `10K+s = i+recv` fixes K: S) | 552 methods / 596 ints; K ambiguous in 44 | debug-info |
| `$default` name | **S** where present (lambda pass-through + bit tests) | 95 | debug-info |
| "param had a default", its original index | **S** | 93 methods (95 have a `$default` role; in 2 of them no bit could be bound to a param). Library recall 47/120: R8 removes `$default` when constant) | sidecar `= <default>`; default expression D |
| original position of residual params, gaps where params were removed | **S** per bound param | 46 % of real params; 109 methods proven to have removed params | Kotlin-shaped signature sidecar (`fun ?(p0: T, /*removed*/, p2: T = …)`) |
| extension/context receiver count | **S** when one param has both a slot and a default binding (`slot − bit`) | subset of 93 | sidecar |
| `@Composable` marker | **S** for restartables; D for non-restartable and lambda bodies | 552 + ~534 | build-visibility annotation, like `@eightr.Inlined`, e.g. `@eightr.Composable(key=…, restartable=true, changed=[…], defaults=[…], removedParams≥n)`. Optionally also the real `Landroidx/compose/runtime/Composable;` (BINARY in source, so absent from DEX; restoring it with BUILD visibility is faithful to the source) |
| restart-lambda labelling ("restart scope of F") | **S** role; name `F$lambda$N` D | 552 | mapping/report; class names after the split |
| function names | **only via the library key DB** (key + `$changed`-count shape check; 190/201 shapes agree and the 11 misses are R8 `$changedK` removal, so the shape check must allow removal) | 201 (library); **0 app** | mapping (S-grade if the DB is versioned and the check passes; otherwise D hint) |
| singletons `lambda$K` field names | S name if era ≥2.1.20 is established (`shouldExecute` present means ≥2.1 with the flag, ≥2.2 by default: treat as D unless ≥2.2 is proven) | 152 | mapping |

**Code simplification.** Nothing worth doing is behaviour-preserving *and* useful:
- The Compose lowering must stay, for the runtime contract.
- `updateChangedFlags(const)` → literal would be a D pure-function fold, but it has no readability value.
- `getClass()` null checks change NPE behaviour.
- Re-inserting removed params is impossible, because the values and types are unknown.
- Restoring the canonical param order is a signature rewrite. It would be D and safe only with all roles and slots
  bound. It concerns 26/552 methods: low value.

Output stays annotative: debug-info names, the build annotation, the mapping and the sidecar. The one structural
change with Compose value is fixing `split-merged-class` for copied `this`, which affects 148 restart lambdas and
the 281 no-dispatch splits generally.

---

## 3. Prioritized rule plan (value per effort on Gretio)

1. **`compose/detect` (structural, D).** Composer = the argmax `T.x(I)T` const-on-param (552 vs 15). Also
   identify `endRestartGroup` (followed by `iput-object` into the scope), `updateChangedFlags` (masks),
   `ComposableLambdaImpl` (`<init>(IZ…)` const/const) and `shouldExecute` (Z return, `dirty&1` arg).
   - Replaces the name-based detection in `sources.rs`, which is blind on Gretio. Cheap.
2. **`compose/restart-lambda` + `compose/synthetic-param-names` (S).** Roles come from the restart-lambda call
   back: `$composer` and `$changed` for 552/552, `$default` for 95. Precondition: the `ucf`/pass-through pattern,
   **not** the `c = ceil((n+t)/10)` count, which fails for 5.5 % of methods and for all with R8-removed ints.
   Handle merged groups per switch case (it works on raw input). Low–medium effort; 0 errors on the 201-method truth.
3. **`compose/composable-marker` (S/D) as a build annotation**, carrying key, roles, default bits and the arity
   bound. It shares plumbing with `@eightr.Inlined`. Low effort, 552 plus lambda bodies.
4. **`compose/param-slot` (new, S) + `compose/default-args` binding + `compose/original-arity-bound`.**
   - Places 46 % of real params at their exact original index.
   - Proves removal in 109 methods, with 0 unsound bounds.
   - Recovers the receiver count.
   - Feeds the Kotlin-shaped sidecar. Medium effort: pattern matching over ReachingDefs, and the default phi needs
     two-arm matching.
5. **`compose/lib-key` (D hint → S with DB + shape check).** Names 201 library composables with full signatures and
   types, and gives exact class-name evidence for ~dozens of renamed Compose types. Medium effort: needs a versioned
   key DB built from AARs, which fits DESIGN §5.4's SigDB.
6. **`r8/split-merged-class` fix** (copied `this`). Needed so that restart lambdas and composable-lambda bodies land
   one per class: 148 of 552, and 281 splits in general.
7. **`compose/singletons` (field names only).** 152 fields. The class-name part is dead after R8 9.4.
8. **Zero yield on Gretio; keep only for D8/debug/tracer builds, lowest priority:** `fn-name/sourceinfo`,
   `fn-name/trace`, `trace-parse`, `fqname-split`, `param-names`, `param-order`, `source-file`, `source-line`,
   `package`, `inline-region`, `lambda-name`, `stable-field`, `live-literals`, `group-offset`, `key-confirm` for app
   functions (no candidate set). Fixture caveat: `compose_basic` is built without `sourceInformation=true` (the
   kotlinc CLI default). So even its D8 output has only trace strings (13), and the sourceInfo rules have no oracle.

---

## 4. Corrections to `docs/sources/compose.md` (R8 9.4.24, runtime 1.10–1.12)

- **C1 (§1.1 `$changed` encoding, "receivers first").** The **dispatch receiver `this` takes the LAST slot**, after
  the regular params. Extension and context receivers come first.
  - Evidence: slot bindings in unminified library dex, 363/365 member methods (e.g.
    `LazyListItemProviderImpl.Item(I,Object)`: `this`→slot 2, `I`→0, `Object`→1).
  - Evidence: the fixture `Member` mask `0x13` (with `this` unused). This is [exp] evidence; it was not checked against CFBT source.
  - Also, `$default` bits exclude receivers, so `slot − bit` = receiver count. §5.3 should use this.
- **C2 (§2 fingerprint 1).**
  - "result written back to the same parameter register": wrong. R8 discards the `startRestartGroup` result (0/552
    `move-result`).
  - "`S.z(LFunction2;)V` (updateScope)": wrong on 9.4. It is `iput-object lambda, Scope.<field>` (584 of ~590 sites).
  - "Almost every such method ends with…": the tail is `endRestartGroup; if-eqz; new-instance L; <init>; iput-object`.
- **C3 (§1.2, §2).** "After R8 inlines `updateChangedFlags`, these constants appear literally": not observed in
  either fixture (8.10: `d/f.h`, 9.4: hosted in `Intrinsics`' class `b3.g`) or in Gretio (0 of 596 sites). It is a
  static call; with a constant `$changed`, the argument is a folded const (`ucf(7)`).
- **C4 (§1.2, §2 "Restart lambdas").** R8 9.4 names them `Outer$N` (from `Outer$$InternalSyntheticLambda$<k>$<hash>$<i>`),
  not `$$ExternalSyntheticLambdaN`. They are horizontally merged **together with composable-lambda bodies and other
  lambdas**: the ctor is `(Object…, I, B)` with a byte class id, and invoke switches on it. The lambda's own int arg
  gets only a `getClass()` null check.
- **C5 (§2 "Synthetic params", §3 `synthetic-param-names`, §5.1).**
  - R8 also removes **`$default` (61 % of those present)** and **`$changedK` (5.5 %)** when they are constant, and
    makes the defaults prologue unconditional.
  - The precondition "count of `$changed` = c, `$default` ∈ {0,d}" therefore rejects correct cases. Replace it with
    the restart-lambda role proof (§1.4).
  - `compose/default-args` recall is ~40 % on real code, not "S when a bit test exists" with high coverage.
- **C6 (§3 R-roles).** "`$changed` = the ints that flow into `x&1`" conflicts with `$default` bit 0 (`$default & 1`).
  Use the lambda signal (`updateChangedFlags` as against pass-through), or `shouldExecute`/`$dirty` flow as against
  value selection.
- **C7 (§2 `$default` fingerprint 5, "single-bit tests guarding a store to a parameter register").** After R8 it is a
  **phi**: `v = bit ? const : p` into a fresh register. The parameter register is not stored to.
- **C8 (§2 ComposableSingletons row, §3 `compose/singletons`).** In R8 9.4, singleton fields and `<clinit>`s are
  relocated into **mixed host classes** (55 hosts on Gretio, 1 pure). A `ComposableSingletons$FileKt` class cannot be
  reconstructed. Only the `lambda$K` **field** name is recoverable.
- **C9 (DESIGN §1.1 detection, `sources.rs`).** Detecting Compose by a `Landroidx/compose/runtime/Composer;`
  reference fails on real R8 apps (Gretio: not detected). It must be structural (§3 rule 1).
- **C10 (§2 table header, §0).** The runtime shape was checked against 1.10.6 only. Gretio is **1.12.1**, and the same
  fingerprints hold. `shouldExecute` exists and is heavily used (1082 calls), which implies compiler ≥2.1 with the
  flag, or ≥2.2.
- **C11 (§2 "Non-restartable composables").** Add: without `sourceInformation` markers and with
  OptimizeNonSkippingGroups, a non-restartable composable carries **no function key at all** after R8. On Gretio,
  167 such methods have no key, and 94 lost `$changed`. `key-confirm`/`lib-key` apply to restartables only.
- **C12 (§5.5, open question "R8 inlining a restartable").** On Gretio there are 0 mid-body `startRestartGroup`
  calls. Every one is on the Composer parameter at the entry. The restart lambda's self-call keeps a second call
  site, so inlining does not happen in practice.
- **C13 (§6 fixtures).** `compose_basic` is built without `-P plugin:…:sourceInformation=true`. The D8 twin has only
  trace strings (trace markers default on). To make the sourceInfo rules testable, add a variant with the Gradle
  defaults.
- **Confirmed as written:**
  - key scheme and values (Greeting −1918547072, Member 1115589987, plus 5 more recomputed);
  - sourceInformation and trace removed in release builds;
  - `new ComposableLambdaImpl(K,false,block)` in `<clinit>`;
  - `Composer` merged into `ComposerImpl`;
  - no `$stable` name survives (removal itself not verified);
  - positional stripping is unsafe (26/552 reordered).
