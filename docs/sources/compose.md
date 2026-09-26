# Source: Jetpack Compose compiler plugin (`compose/`), and how R8 interacts with it

Status: research notes · Undo order: 3 (after `r8/` and `desugar/`, before `kxs/` … `kotlinc/`)

## 0. Scope, provenance, legend

**What was read.**

| Artifact | Revision | Used for |
|---|---|---|
| `JetBrains/kotlin` `plugins/compose/compiler-hosted` (+ `group-mapping`, golden test data) | HEAD `06003680c56d09dffcf82b3817372c5aaea66b50` (2026-09-25) | current compiler (K2) |
| Same files at tags `v2.0.0`, `v2.0.20`, `v2.1.0`, `v2.1.20`, `v2.2.0`, `v2.2.20`, `v2.3.0` | raw.githubusercontent | version history (§1.14) |
| `androidx.compose.compiler:compiler-hosted:1.5.14` sources jar (Google Maven) | last androidx-hosted release | "old" compiler (Kotlin 1.9, K1) |
| `androidx/androidx` `compose/runtime/runtime` | HEAD `c772ca87cac06fbf47d34f86ab0b1202c71fb559` (2026-09-25) | runtime API and consumer keep rules |
| `JetBrains/kotlin` `libraries/tools/kotlin-compose-compiler` | same HEAD | Gradle plugin defaults |
| `docs/research/compose-app.md` | 2026-09-25 | R8 9.4 corrections (C1–C13), role/slot/default binding measured on Gretio and a 201-method library truth |
| `docs/research/compose-keys.md` | 2026-09-25 | key stability over 103 library jars (compose 1.5–1.12, material3 1.1–1.4), collisions, R8 survival, version resolution |
| `docs/research/compiler-eras.md` | 2026-09-25 | `compose_shapes` compiled with Kotlin 2.1.21 / 2.2.21 / 2.3.10 / 2.4.20: era markers, key identity |
| `docs/research/PLAN-compose-sigdb.md` | 2026-09-25 | implementation plan (M0–M7) |

**What was run (experiment).** `kotlinc` 2.3.20 with `kotlin-compose-compiler-plugin-embeddable` 2.3.20,
`sourceInformation=true` and trace markers on (both are Gradle-plugin defaults), against
`androidx.compose.runtime:runtime-android:1.10.6`. The classes were then shrunk with **R8 8.10.9-dev**
(`--release --min-api 24`), using the runtime AAR's own `proguard.txt`, with runtime, collection, stdlib and
coroutines all as program input (so the runtime is obfuscated as it would be in a real app). Three
variants were built: `base` (plain), `trace` (the app also calls `Composer.setTracer(...)`, as the
`runtime-tracing` artifact does), and `noopt` (`-dontoptimize`). The mapping file was only used as an
oracle to interpret the results. The sample (`Main.kt`: defaults, 12-param function, member function,
`remember`, `key`, `@ReadOnlyComposable`, `@NonRestartableComposable`, inline composable, composable
lambdas) lived in the scratchpad. §6 turns it into fixtures.

**File abbreviations** (all under `plugins/compose/compiler-hosted/src/main/java/androidx/compose/compiler/plugins/kotlin/`):
`CFBT` = `lower/ComposableFunctionBodyTransformer.kt`, `CPT` = `lower/ComposerParamTransformer.kt`,
`CLM` = `lower/ComposerLambdaMemoization.kt`, `ACL` = `lower/AbstractComposeLowering.kt`,
`DKT` = `lower/DurableKeyTransformer.kt`, `DFKT` = `lower/DurableFunctionKeyTransformer.kt`,
`DKV` = `lower/DurableKeyVisitor.kt`, `CST` = `lower/ClassStabilityTransformer.kt`,
`LLT` = `lower/LiveLiteralTransformer.kt`, `CIGE` = `ComposeIrGenerationExtension.kt`,
`RT/` = androidx `compose/runtime/runtime/src/`.

**Later experiments (2026-09-25).** Fixtures `compose_basic`, `compose_basic_r94` and `compose_shapes` (kotlinc 2.4.20,
runtime 1.10.6; `compose_shapes` with `sourceInformation=true`; R8 9.4.24 with runtime consumer rules), a probe app using
material3 1.4.0 / compose 1.10.6 shrunk by R8 9.4.24 with its mapping as oracle, and **Gretio** (a real R8 9.4.24
full-mode app, compose 1.12.1, material3 1.4.0, no mapping; α-invariant structural analysis only). Several §1–§3 claims
from the R8 8.10.9 run did not hold on 9.4; they are corrected in place and tagged.

**Legend.** **[src]** = verified by reading source. **[exp]** = verified by the original compile/R8 8.10.9 experiment.
**[exp 9.4.24]** = verified on the 2.4.20 / R8 9.4.24 fixtures, the probe app, compose-app's unminified D8 library dex, or
compose-keys' library-jar scans. **[exp Gretio]** = measured on Gretio
(counts are α-invariant; library truth = the 201 composables whose key matches a library key DB).
**[exp eras]** = the Kotlin 2.1.21–2.4.20 comparison in `docs/research/compiler-eras.md`.
**[inf]** = inferred, not verified; treat it as a hypothesis for a fixture to confirm.

**Two contract notes for the rule registry.**

1. *N rules.* DESIGN §0.1 forbids N in the pipeline. The brief for this document allows N when the rule
   enumerates **every** candidate in canonical order. Every N rule below does two things. It
   (a) reports the exhaustive candidate set. It (b) writes only a D or identity result into `dex'`,
   unless a filter (usually a compiler-emitted hash) leaves **exactly one** candidate. The true preimage
   is guaranteed to be in the set, and it is the only survivor, so the rule is promoted to S.
2. *Semantics (§0.2.1).* The Compose lowering **cannot be physically undone in an executable `dex'`**.
   Removing `$composer`/`$changed`, groups or restart lambdas would break the runtime contract.
   So `compose/` rules are **annotative**. They set names and parameter names, label synthetic
   parameters and groups, record the Kotlin-level ("source") signature, default arguments, file and
   lines in the output mapping and the report, and they may attach a marker annotation. Body rewrites
   are limited to what R8-undo already allows. Any "de-lowered" view belongs in the report or a
   decompiler-hint sidecar, not in `dex'`.

---

## 1. Transformations catalog

The plugin runs these IR lowerings in this order [src, `CIGE` 65–235]: ComposableLambdaAnnotator →
ClassStabilityTransformer → (LiveLiteralTransformer if enabled) → ComposableFunInterfaceLowering →
**DurableFunctionKeyTransformer** → ComposableVersionOverloadsLowering → ComposableDefaultParamLowering →
AdaptedComposableReferenceTypePatcher → **ComposerLambdaMemoization** → **ComposerParamTransformer** →
ComposableTargetAnnotationsTransformer → ComposerIntrinsicTransformer → **ComposableFunctionBodyTransformer**
→ ComposableAnnotationRemover → (FunctionKeyMeta annotations, if requested).

### 1.1 Synthetic parameters: `$composer`, `$changed[N]`, `$default[N]`  [src, exp]

Every `@Composable` function gets parameters appended after its regular parameters ([src] `CPT` 645–755):

```
JVM order:  [this] [context params…] [ext receiver] p1..pn  $composer:Composer  $changed:int ×c  [$default:int ×d]
c = changedParamCount(n, t) = max(1, ceil((n + t) / 10))        t = #dispatch + #context + #extension receivers
d = defaultParamCount(n)    = ceil(n / 31)   (only if requiresDefaultParameter(); n counts Regular params only)
```

- Constants: `BITS_PER_INT = 31`, `SLOTS_PER_INT = 10`, `BITS_PER_SLOT = 3` ([src] `CFBT` 108–110). Formulas:
  `CFBT` 137–174. Names: `$composer`, `$changed`, `$changed1`, …, `$default`, `$default1`, …
  ([src] `CPT` 705–723; `ComposeNames.kt` 6–9).
- The compiler **asserts** this exact arity when it builds the restart lambda
  (`composerIndex = real + thisParamCount; changedIndex = composerIndex+1; defaultIndex = changedIndex + c`,
  `require(parameterCount == …)`, [src] `CFBT` 1708–1731). This is why the shape is a proof when it is
  intact.
- `requiresDefaultParameter()` ([src] `CPT` 545–557): true for regular functions with any default,
  for default stubs, and for legacy open functions. **False for open/abstract virtual functions**, which instead
  get a static wrapper `<name>$default` in a nested `ComposeDefaultImpls` class ([src]
  `lower/ComposableDefaultParamLowering.kt` 30–80, 233, 294; `ComposeNames.DefaultImpls`). The wrapper is itself
  **restartable**; it takes the receiver as a trailing regular param (null-checked as `"$this$"`; `Render$default(I, Base, Composer, I, I)`,
  `this` in slot 1), carries `C(Render$default)N(depth)` and the trace string `"….Base.ComposeDefaultImpls.Render$default (Shapes.kt:-1)"`
  (line **−1**), and its key is the fallback hash (§1.5) [exp 9.4.24].
- `$composer` has type `androidx/compose/runtime/Composer` (nullable at IR level). Regular param names are
  sanitized with `dexSafeName` ([src] `ACL` 1215–1224, `CPT` 688).
- Composable **property getters** get `@JvmName("getX")` added, so the name is unchanged even with extra params ([src] `CPT` 668–680).
- Composable **function types** `@Composable (A) -> R` are lowered to `FunctionN+2`: `(A, Composer, Int) -> R`.
  This is also true of lambdas (`Function2<Composer,Integer,Unit>` for `@Composable () -> Unit`) [exp, javap].
  Calls go through `FunctionN.invoke(…, composer, Integer.valueOf(bits))`. Lambdas with more than 22 params
  use `ComposableLambdaN` ([src] `CLM` 1219).
- Verified examples [exp]: `Greeting(String,int,boolean,Composer,int,int)` (n=3, c=1, d=1);
  `Many(int×11,String,Composer,int,int,int)` (n=12, c=2, d=1);
  `Screens.Member(Point,Wrapper,Composer,int)` (instance member: t=1, c=1).

**`$changed` encoding** ([src] `CFBT` 66–115, 4575–4700). Bit 0 is the "force/restart" bit. Slot order is
extension/context receivers, then regular params, then the **dispatch receiver `this` in the LAST slot** (10 slots per int)
[exp 9.4.24: `Card.Show(times, suffix)` tests `times` with `&6`, `suffix` with `&48`, `this` with `&384`, skip mask
`147/146`; D8 library dex: 363/365 members; not yet checked against `CFBT`]. Slot *s* uses bits `3s+1 .. 3s+3`:
low two bits `ParamState` = Uncertain `00` / Same `01` / Different `10` / Static `11`, and the high bit
`0b100` = Unstable. The body copies it into `$dirty` and fills uncertain slots with
`if ($changed & (0b110<<(3s+1)) == 0) $dirty |= $composer.changed(p) ? 0b100<<(3s) : 0b010<<(3s)`. The
concrete values seen in the experiment [exp] were `changed(x)?4:2`, `?32:16`, `?256:128` for slots 0–2, and
callers passed `6` = Static slot 0. The skip test is `$dirty & M1 != M2`, where `M1`/`M2` have `0b001` in every
**used** parameter's slot, plus bit 0 (`irHasDifferences`, [src] `CFBT` 4619–4700; e.g. `0x13/0x12`,
`147/146`, `0x12492493/0x12492492` [exp]). Callers pass per-slot certainty bits computed at the call
site ("comparison propagation").

**`$default` encoding** ([src] `CFBT` 219–236 doc, 1337–1420). Bit *i* (= regular-param index,
`defaultIndexForSlotIndex = slot − valueArgsStart`, [src] `CFBT` 4196) is 1 when the caller omitted
argument *i*. Receivers have no bit, so for a param bound both ways `slot − bit` = number of leading
extension/context receivers (D8 library dex: 3223/3425 bindings have shift ∈ {0,1} [exp 9.4.24]). The prologue emits `if ($default & (1<<i) != 0) p_i = <default expr>`. For a static default in a
skippable function it also emits `$dirty |= Static<<slot`. For non-static defaults
(e.g. `remember {}`, theme reads) the prologue is wrapped in
`$composer.startDefaults(); if ($changed&1==0 || $composer.defaultsInvalid) {…} else {skipToGroupEnd; …}; $composer.endDefaults()`
([src] `CFBT` 1620–1660, 2143–2153). Seen in the experiment: `$default & 1024` / `& 2048` for params a11/a12 [exp].

### 1.2 Restart groups, skipping, `updateScope` (restartable functions)  [src, exp]

For a Unit-returning, non-inline, not-`@NonRestartableComposable` function ([src] `CFBT` 1055–1195):

```
$composer = $composer.startRestartGroup(KEY_fn)               // returns Composer
[sourceInformation($composer, "C(Name)…")]                      // §1.6
$dirty = $changed; <per-param dirty checks>; [defaults prologue]
if ($composer.shouldExecute(<differences>, $dirty & 1)) {       // ≥2.1.0 with PausableComposition; else
    if (isTraceInProgress()) traceEventStart(KEY_fn, $dirty, $dirty1|-1, "fq.Name (File.kt:L)")   //   `(<diff>) || !$composer.skipping`
    <body>
    if (isTraceInProgress()) traceEventEnd()
} else $composer.skipToGroupEnd()
$composer.endRestartGroup()?.updateScope { c, _ -> Fn(p…, c, updateChangedFlags($changed | 1), $default) }
```

- **Restart lambda.** In K2 it is an indy lambda over a private static synthetic `Fn$lambda$N(captures…, $changed…, $default…, Composer, int)`.
  D8/R8 8.10 desugar it to `…$$ExternalSyntheticLambdaN` [exp]; D8/R8 9.4 name it `Outer$N` (from
  `Outer$$InternalSyntheticLambda$<k>$<hash>$<i>`; e.g. `Card$0` for `Card.Show$lambda$1`) [exp 9.4.24]. In K1 (androidx ≤1.5.x)
  it is an inner class extending `kotlin.jvm.internal.Lambda` [inf]. Its body passes every captured param through unchanged, puts
  its own Composer arg in the `$composer` slot, `updateChangedFlags(x|1)` in every `$changed` slot, and the
  captured `$default` unchanged ([src] `CFBT` 1710–1770; [exp] javap of `Greeting$lambda$1`).
  `updateChangedFlags` ([src] `RT/commonMain/.../RecomposeScopeImpl.kt` 44–60) uses the masks
  `0x12492492`, `0x24924924`, `~0x36DB6DB6`. **R8 does not inline it**: it stays a static call, and with a constant
  `$changed` the argument is folded (`ucf(7)`) [exp 9.4.24: `r.f` in `compose_shapes`; exp Gretio: 596/596 sites]. The
  earlier "constants appear literally" claim was not reproduced on 8.10 or 9.4 fixtures.
- Members: `this` is saved to a temp and captured ([src] `CFBT` 1703–1708).
- `shouldExecute(boolean,int)` replaced `(diff) || !skipping` when the runtime has it and
  `PausableComposition` is on ([src] `CFBT` 476–487, 1805–1822). Observed: 2.1.21 uses `getSkipping()`, 2.2.21+ use
  `shouldExecute(ZI)Z` [exp eras]. See §1.14 for defaults by version.

### 1.3 Non-restartable, read-only, inline, lambdas  [src, exp]

- **Non-restartable** (non-Unit return, `@NonRestartableComposable`, inline lambdas, …): `visitNonRestartableComposableFunction`
  ([src] `CFBT` 722–870). With `OptimizeNonSkippingGroups` (default on since 2.2.0, §1.14) it gets **no outer
  group**, only `sourceInformationMarkerStart(composer, KEY_fn, "C(Name)…")`/`…End` when source info is on.
  Without it, it gets `startReplaceGroup(KEY_fn)` … `endReplaceGroup()`. `@ReadOnlyComposable` gets markers only.
  [exp]: `NonRestart`, `readOnly` and `rememberDouble` have only marker calls. 2.1.21 still wraps them in
  `startReplaceGroup(KEY_fn)` [exp eras].
  **Exception: virtual (open/override) composables** are non-restartable but keep `startReplaceGroup(KEY_fn)` …
  `endReplaceGroup()` in 2.4.20, and that key survives R8 (`Base.Render` `0x5d098203`, `Derived.Render` `0x756f888b`)
  [exp 9.4.24].
- **Composable lambdas** get no root group, because `ComposableLambdaImpl.invoke` owns it. They get
  `sourceInformation(c,"C<locs>:File#pkg")` (no name), skipping logic and trace markers ([src] `CFBT` 875–1050; [exp]).
- **Inline composables** (`CC(Name)`) get only markers, and no trace markers ([src] `CFBT` 752–754; [exp] `InlineRow`).
  Their bodies are inlined by kotlinc into callers, **including library inline composables** such as
  `Column`/`Row`/`Box`, with their marker strings `CC(Column)…:Column.kt#<pkghash>` [inf, from the mechanism; seen for
  runtime `remember`: `CC(remember):Main.kt#9igjgp` [exp]].

### 1.4 Replace, movable and reusable groups (control flow, `key`, `remember`)  [src, exp]

- Each composable-containing branch of `if`/`when`, early return or loop gets
  `startReplaceGroup(KEY_elem)`/`endReplaceGroup()`. Runtime < 1.7 uses `startReplaceableGroup`/`endReplaceableGroup`.
  The compiler picks whichever the runtime on the compile classpath has ([src] `ACL` 1479–1495).
- `key(k1, …) { }` becomes `startMovableGroup(KEY_elem, k)`, or `joinKey(k1,k2)` for multiple keys ([src] `CFBT` 2239–2280,
  3458–3530; [exp] `startMovableGroup(-1133656910, i)`).
- **Intrinsic remember**: `remember(k…) { calc }` is inlined to `changed(k)… || rememberedValue()===Composer.Empty → updateRememberedValue(calc())`.
  In cases where a group is needed it is wrapped in `startReplaceGroup(KEY(remember fn))`, where the key is computed with
  `functionSourceKey(remember)` ([src] `CFBT` 3290–3310). For an external function that has no durable key, this is
  `("$fqName$jvmDescriptor").hashCode()` ([src] `ACL` 1264–1275). Its marker string is forged as
  `CC(remember)<locs>:<callerFile>#9igjgp`, using the runtime package hash ([src] `CFBT` 3322–3346; [exp]).
- Early returns use `currentMarker`/`endToMarker(marker)` ([src] `CFBT` 448–466, 2263–2270).

### 1.5 Group keys: `KEY_fn` and `KEY_elem` (durable keys)  [src, exp: exact]

**Function key.** `DurableFunctionKeyTransformer` always runs ([src] `CIGE` 118–128; also in 1.5.14 [src]).
`sourceKey()` returns `durableFunctionKey.key` ([src] `ACL` 1264–1275) = `String.hashCode()` of:

```
"fun-" + [ExtRecvSimpleName "."] + jvmFriendly(name) + "(" + join(",", simpleName(type) for regular+context params) + ")" + simpleName(returnType)
       + PATH
PATH   = for node from innermost enclosing scope to root: "/" + node.key [ + ":" + siblingIndex if >0 ]
node.key ∈ { "class-<Name>", "pkg-<fqName or <root>>", "file-<basename>", "fun-<sig>", "call-<name>", "arg-<i>", "$this", "$$this",
             "val-<n>", "set-<n>", "loop", "cond", "body", "branch", "else", "try", "catch", "finally", "str", "vararg", "<i>", "param-<n>", "entry-<n>", "get", "set" }
```

(`DFKT` 128–145; `DKT` 57–363; `DKV` 19–185 [src]). `jvmFriendly` turns specials into `$…$` (`<anonymous>` → `$anonymous$`).
`simpleName` is the classifier's **simple** name. In K2, composable function types are named `ComposableFunctionN`;
the **K1 frontend names them `FunctionN` of the pre-lowering arity**. So every composable with a composable-lambda
parameter has two keys, one per frontend (material3 `Button`: `…,Function1)Unit/…` = 650121315 in 1.3.2,
`…,ComposableFunction1)Unit/…` = −1310015664 in 1.4.0; recomputed). Non-composable function params (`onClick: () -> Unit`)
keep their key. The androidx libraries switched frontend at compose 1.9 / material3 1.4 [exp 9.4.24, compose-keys §3].
Within one frontend the key is identical across Kotlin 2.1.21–2.4.20 for all 13 restartables of `compose_shapes` [exp eras].
Other key breaks: a **file rename** (`X.kt` → `X.android.kt`) or a function rename. The JVM facade name is not in the key.
Verified by recomputation [exp]:

| key string | hash | where seen |
|---|---|---|
| `fun-Test()Unit/pkg-<root>/file-Test.kt` | −1794342280 | golden `DefaultParamTransformTests/testComposableWithAndWithoutDefaultParams.txt` |
| `fun-B(Int)Unit/class-A/pkg-<root>/file-Test.kt` | 666085442 | golden `TraceInformationTest/testBasicComposableFunctions.txt` |
| `fun-Foo(ComposableFunction0)Unit/pkg-<root>/file-Test.kt` | −239871899 | golden `FunctionKeyMetaAnnotationsTests/testComposableLambda.txt` |
| `fun-$anonymous$()Unit/arg-0/call-Foo/fun-Bar()Unit/pkg-<root>/file-Test.kt` | −420233864 | same golden (lambda) |
| `fun-$anonymous$()Unit/arg-0/call-Foo:1/fun-Bar()Unit/pkg-<root>/file-Test.kt` | 511230191 | same (2nd sibling call) |
| `fun-Greeting(String,Int,Boolean)Unit/pkg-com.example.app/file-Main.kt` | −1918547072 | experiment, javap **and R8 dex** |
| `fun-Member(Point,Wrapper)Unit/class-Screens/pkg-com.example.app/file-Main.kt` | 1115589987 (`0x427e8d63`) | experiment, **R8 dex** |
| `fun-Defaults(Int,String,Int)Unit/pkg-com.example.shapes/file-Shapes.kt` | `0xdf96e8a4` | `compose_shapes`, 2.1.21–2.4.20 [exp eras] |
| `fun-Show(Int,String)Unit/class-Card/pkg-com.example.shapes/file-Shapes.kt` | `0x1b645be2` | `compose_shapes` D8 and R8 9.4 dex [exp 9.4.24] |
| `fun-Render(Int)Unit/class-Base/pkg-com.example.shapes/file-Shapes.kt` | `0x5d098203` | open fn, `startReplaceGroup`, R8 9.4 dex [exp 9.4.24] |
| `fun-$anonymous$()Unit/arg-0/call-Slot/fun-Lambdas(String)Unit/pkg-com.example.shapes/file-Shapes.kt` | −1188304595 | singleton field `lambda$-1188304595` [exp 9.4.24] |

The key is **α-invariant evidence that commits to the original simple name, simple parameter-type names,
enclosing class chain, package and file basename**. It is a 32-bit hash, so it can confirm candidates but it cannot be inverted.

**Element key** (inner groups, markers) ([src] `CFBT` 1969–1991):
`h = 31*(31*KEY_fn + (start−fnStart)) + (end−fnStart)`, then `31*h + value.hashCode()` for `IrConst`,
`+2` for `IrBlock`, `+3` for `IrComposite`. Offsets are file char offsets, so any edit earlier in the file changes
them. Across library minors only ~40–80% of inner/lambda keys are kept; across patches 100%. Inner keys are therefore
useless for identity but are a **minor-version fingerprint** (§3 `compose/lib-key`) [exp 9.4.24, compose-keys §3, §7].

**Lambda key** = the durable key of the lambda's function. It is passed as the first int to
`composableLambdaInstance`/`rememberComposableLambda`/`ComposableLambdaImpl(<init>)` ([src] `CLM` 996–1003).

**Old keys.** If no durable key exists (external functions, or configurations that skip the transformer),
the fallback is `"$fqName$jvmDescriptorWithoutName".hashCode()` ([src] `ACL` 1264–1275; the same code is in 1.5.14
`AbstractComposeLowering.kt` 1203–1216). For the generated `ComposeDefaultImpls.<name>$default` wrapper the descriptor is
the **post-lowering** one: `"com.example.shapes.Base.ComposeDefaultImpls.Render$default(ILcom/example/shapes/Base;Landroidx/compose/runtime/Composer;II)V"`
→ `0xb51dcc9e` [exp 9.4.24]. Which descriptor is used for external callees (e.g. the `remember` key) is unverified.

### 1.6 `sourceInformation*` strings  [src, exp]

Calls: `ComposerKt.sourceInformation(Composer,String)` after a group start, and
`sourceInformationMarkerStart(Composer,int KEY,String)`/`…End(Composer)` for group-less scopes ([src] `CFBT` 2025–2090).
Strings are filled in late by `applySourceFixups` ([src] `CFBT` 1300–1321). Grammar ([src] `CFBT` 4096–4114, 4339–4368,
4866–5020):

```
function-info := call-info [param-info] [locations] ":" file ["#" base36(abs(pkgHash))]
call-info     := "C" ["C"] ["(" simpleName ")"]            // "CC" = inline fn; no "(…)" for lambdas
param-info    := "P(" sorted-index-encoding ")"            // compiler ≤2.1.x or runtime <1.9
               | "N(" name[":" inlineClassFqn] ("," …)* ")" // compiler ≥2.2.2x (seen 2.2.21) && runtime ≥1.9 (JVM only)
locations     := loc ("," loc)*        loc := ["*"] line0 "@" offset ["L" length]   // line0 is 0-based
block-info    := [locations]                                // inner groups: e.g. "12@314L38"
pkgHash       := fold(0){h,c -> h*31 + c.code} over the package FqName; "c#" abbreviates "androidx.compose."
```

- `N(...)` era: absent in 2.1.21 (`P(i)`), present in 2.2.21, 2.3.10, 2.4.20 [exp eras]. The earlier reading of the tag
  diff (≥2.3.0) was wrong; 2.2.0–2.2.20 were not compiled, so the exact first version is open.
- Example [exp]: `C(Greeting)N(name,times,enabled)10@256L30:Main.kt#bw24ds`. Here `bw24ds` = base36 hash of `com.example.app`,
  offset 256 = `remember { mutableStateOf(0) }`, line 10 (0-based) = source line 11.
- `N(...)`/`P(...)` drop params whose name starts with `$`. So anonymous `_` lambda params, which are sanitized to `$…`,
  are **omitted** ([src] `CFBT` 4913, 4967; `ACL` 1215).
- With `sourceInformation=false`, **public** functions still get `C(Name)P(...)` with no file ([src] `CFBT` 4110–4113).
- Gradle default: `includeSourceInformation = true` ([src] `libraries/tools/kotlin-compose-compiler/.../ComposeCompilerGradlePluginExtension.kt` 61;
  `ComposeCompilerSubplugin.kt` 71–75, which can be suppressed when AGP configures it).

### 1.7 Trace markers  [src, exp]

`if (isTraceInProgress()) traceEventStart(KEY_fn, dirty1|-1, dirty2|-1, info)` / `traceEventEnd()` on
restartable, non-restartable (non-inline) functions and composable lambdas ([src] `CFBT` 2098–2141).
`info = "${kotlinFqName} (${file.name}:${line0})"`, where `line0 = getLineNumber(body.startOffset)`, which is **0-based**.
Example: `com.example.app.Greeting (Main.kt:9)` for a body that starts on source line 10 [exp]. Lambdas:
`com.example.app.Screen.<anonymous> (Main.kt:31)` and
`com.example.app.ComposableSingletons$MainKt.lambda$-1495921124.<anonymous> (Main.kt:30)` [exp]. The format is the same in 1.5.14 [src].
Gradle default `includeTraceMarkers = true` ([src] `ComposeCompilerGradlePluginExtension.kt` 158).

### 1.8 Composable lambdas and `ComposableSingletons$<FileKt>`  [src, exp]

- A composable lambda **with captures** becomes `rememberComposableLambda(KEY, tracked=true, block, $composer, $changed)`
  (runtime ≥1.7). Older runtimes use `composableLambda($composer, KEY, tracked, block)` ([src] `CLM` 965–1010).
  Arity >22 uses the `…N` variants with an arity arg.
- A composable lambda **without captures** is hoisted to a static field of `object ComposableSingletons$<FileKt>`
  (`FileKt` = `PackagePartClassUtils.getFilePartShortName(fileName)`, which comes from the file name and **not** from `@file:JvmName`)
  and initialized in `<clinit>` with `composableLambdaInstance(KEY, false, block)` ([src] `CLM` 317–345, 834–905; [exp]).
  Field name: **`lambda$<KEY>`** since Kotlin 2.1.20 (`$0`,`$1`… appended on collision); **`lambda-<i>`** before
  that ([src] `CLM` 835–846; tag diff §1.14). The internal getter is `getLambda$<KEY>$<module>`, which leaks the Gradle
  module name ([exp] `getLambda$-1495921124$main`). The old name is still emitted for public inline scopes ([src] `CLM` 893–910).
- `tracked` = "has captures" ([src] `CLM` 1001–1002).
- Lambda **body** codegen: 2.1.21 emits anonymous classes (`ComposableSingletons$FileKt$lambda$K$1`); 2.2.21+ emit indy
  lambdas over static bodies named `lambda__K$lambda$0` (`-` in the key becomes `_`, e.g. `lambda__1188304595$lambda$0`)
  [exp eras; exp 9.4.24]. The `lambda$K` field and `getLambda$K$<module>` getter appear in all of 2.1.21–2.4.20 [exp eras].

### 1.9 Non-composable lambda memoization  [src]

Inside composable functions, non-composable lambdas (with or without captures on JVM/K2) are wrapped in an
intrinsic-remember keyed on their captures: `changed(capture)… || rememberedValue()===Empty` → `updateRememberedValue(lambda)`
([src] `CLM` 690–730, 1036–1110).

### 1.10 Stability: `$stable` and `@StabilityInferred`  [src, exp]

For public/internal, non-enum, non-interface, non-inline, non-inner classes: `public static final int $stable`
(`@JvmField`) with values `0` = stable, `8` = `UNSTABLE(0b100)<<1` = unstable, or an OR-expression of other
classes' `$stable` fields for generic or external dependencies ([src] `CST` 45–50, 100–215; `ACL` 780–930;
[exp] `Holder(var)` → `8`, `Point` → `0`). `@StabilityInferred(parameters=mask)` has **BINARY** retention
([src] `RT/commonMain/.../internal/StabilityInferred.kt` 33), so it is dropped from DEX.
Call sites read `Other.$stable` only for classes from other modules whose stability is only known at runtime ([src] `CFBT` 3590–3600).

### 1.11 Live literals  [src]

These are only generated with `liveLiterals`/`liveLiteralsEnabled` (IDE Live Edit, debug). The plugin makes an
`object LiveLiterals$<FileKt>` with `@LiveLiteralFileInfo(file="/path/File.kt")` (**RUNTIME** retention) and one
getter per literal, annotated `@LiveLiteralInfo(key, offset)` (RUNTIME). The getter calls
`liveLiteral("<Type>$<durable path>", value)` when `isLiveLiteralsEnabled` is true. Example key:
`Int$fun-bar$class-$no-name-provided$$fun-a` ([src] `LLT`; golden `LiveLiteralTransformTests/testAnonymousClass.txt`;
`RT/commonMain/.../internal/LiveLiteral.kt` 33–62).

### 1.12 Other lowering

- `@FunctionKeyMeta(key,startOffset,endOffset)`: BINARY retention, and only when requested or when the runtime
  annotation is not RUNTIME ([src] `CIGE` 236–241; `RT/.../internal/FunctionKeyMeta.kt` 33). **Not in DEX.**
- `@ComposableTarget`/`@ComposableInferredTarget`/`…Constraints`/`@Composable` itself: BINARY, so they are gone from DEX
  ([src] runtime sources). `ComposableAnnotationRemover` strips `@Composable` from IR types.
- Composable fun-interfaces, function references (adapted into `AdaptedFunctionReference`), and value-class
  default stubs (`makeStubsForDefaultValueClassIfNeeded`, [src] `CPT` 796–950) are all covered by fixtures §6.
- **Compose mapping (Kotlin ≥2.3 + AGP).** The Gradle plugin runs `group-mapping`, which scans bytecode for groups
  and appends `ComposeStackTrace -> $$compose:` entries (`1:1:<method>:<line>:<line> -> m$<key>`) to the R8
  mapping ([src] `plugins/compose/group-mapping/.../ComposeMapping.kt`; `kotlin-compose-compiler/.../ComposeAgpMappingFile.kt`).
  This is useless to 8R, since there is no mapping, but it confirms that the ecosystem treats keys as function identifiers.

### 1.13 Kotlin-level vs JVM shape summary

| Original | Emitted (K2, runtime ≥1.7) |
|---|---|
| `@Composable fun F(p1..pn)` | `static void F(p1..pn, Composer, int×c [, int×d])` + restart lambda + groups |
| `@Composable fun F(): R` | no restart group; `R F(…, Composer, int×c [, int×d])` |
| `@Composable () -> Unit` value | `Function2<Composer,Integer,Unit>`; `ComposableLambdaImpl` at runtime |
| no-capture composable lambda | static field `ComposableSingletons$FileKt.lambda$KEY` |
| class `C` | `+ public static final int $stable` |

### 1.14 Version matrix (compiler × runtime)

| Feature | androidx 1.0–1.5.x (K1, Kotlin ≤1.9) | Kotlin 2.0.x | 2.1.0–2.1.x | 2.2.x | 2.3.x / 2.4.x / HEAD |
|---|---|---|---|---|---|
| Durable `KEY_fn` string scheme | same scheme [src 1.5.14], but K1 frontend names composable fn types `FunctionN` (§1.5) | same | same; keys identical 2.1.21→2.4.20 [exp eras] | same | same [exp] |
| Replace groups | `startReplaceableGroup` | `startReplaceGroup` if runtime has it, else fallback [src] | same | same | same |
| Composable lambda factory | `composableLambda(c,KEY,…)` | `rememberComposableLambda` if runtime has it [src] | same | same | same |
| Composable lambda body | anonymous class [inf] | ? | anonymous class `…$lambda$K$1` [exp eras] | indy, static `lambda__K$lambda$0` [exp eras] | same [exp 9.4.24] |
| Singleton field names | `lambda-<i>` [src] | `lambda-<i>` [src] | `lambda-<i>` (2.1.0) / **`lambda$KEY` (2.1.20+)** [src; exp eras 2.1.21] | `lambda$KEY` | `lambda$KEY` |
| Skip condition | `(dirty&M1!=M2) \|\| !skipping` | same [src] | `shouldExecute(…)` if flag on (default **off**) [src]; `getSkipping()` seen in 2.1.21 [exp eras] | flag default **on** [src]; `shouldExecute(ZI)Z` seen in 2.2.21 [exp eras] | on |
| OptimizeNonSkippingGroups | n/a | default off | default off (non-restartables wrapped in `startReplaceGroup(KEY_fn)` [exp eras]) | **default on** [src] (markers only [exp eras]) | on; virtual fns keep a replace group [exp 9.4.24] |
| Strong skipping | opt-in | option; flag default on at 2.0.20 [src] | always | always | always (flag removed) |
| Param info | `P(...)` | `P(...)` | `P(...)` [exp eras] | **`N(names)`** when runtime ≥1.9; seen in 2.2.21 [exp eras] | `N(names)` [exp] |
| Restart lambda codegen | inner class (K1) [inf] | indy → D8 synthetic [inf] | same [inf] | same [inf] | indy → `$$ExternalSyntheticLambdaN` (D8/R8 8.10) [exp]; `Outer$N` (D8/R8 9.4) [exp 9.4.24] |
| Trace string | `fq (File:line0)` [src] | same | same | same | same; `:-1` for generated `$default` wrappers [exp 9.4.24] |
| Library AARs (observed) | compose ≤1.6, material3 ≤1.2: non-restartables have `startReplaceableGroup(KEY_fn)` | | | | compose ≥1.7, material3 ≥1.3: markers only; compose ≥1.9, material3 ≥1.4: K2 keys [exp 9.4.24, compose-keys §2–3] |

"Runtime has it" matters: the **runtime version on the compile classpath** selects the shape, not just the compiler version
([src] `ACL` 1479–1495; `CFBT` 476–487, 545–552, 586–590). A single APK can mix shapes, because libraries were compiled by
different compilers.

---

## 2. What survives R8 (no mapping)

Experiment results ([exp], R8 8.10.9, runtime 1.10.6, consumer rules applied), re-checked on R8 9.4.24 fixtures
([exp 9.4.24]) and on Gretio (R8 9.4.24, runtime 1.12.1, [exp Gretio]). The same fingerprints hold on runtime 1.12.1.

| Transformation | After R8 | Evidence |
|---|---|---|
| `sourceInformation*` calls and strings | **Removed in all three variants, including `-dontoptimize`.** | runtime `proguard.txt` has `-assumenosideeffects` on the three `ComposerKt.sourceInformation*` ([src] `RT/androidMain/keepRules/rules.keep` 1–5). 0 hits for `bw24ds`/`C(` in all dumps. Gretio: 0 `C(`/`N(`/`P(` strings [exp Gretio]. |
| Trace `info` strings | **Removed** when `compositionTracer` is never written (R8 folds `isTraceInProgress()` → false). **Kept** when anything reachable calls `Composer.setTracer` (e.g. `runtime-tracing`'s initializer [inf]) or with `-dontoptimize`. | `base`: 0 strings; `trace` and `noopt`: all 12 `"… (Main.kt:N)"` strings present, each next to `const KEY_fn`. Gretio (no runtime-tracing): 0 [exp Gretio]. |
| `KEY_fn` of **restartable** fns, `KEY_elem`, lambda keys | **Survive as `const` ints.** `startRestartGroup` is renamed (e.g. `Ld/k;.B:(I)Ld/k;`). The entry key is the const argument of the method's first `C.x(I)C` call. | `const #427e8d63` in `Member`; lambda keys in `new ComposableLambdaImpl(IZ…)` and `<clinit>` sequences. Probe app: 96/96 reachable restartable library composables keep it; Gretio: 552 restartables, all keys distinct [exp 9.4.24; exp Gretio]. |
| `KEY_fn` of **non-restartable / inline** fns | **Lost**: with OptimizeNonSkippingGroups the only carrier is `sourceInformationMarkerStart`, which the consumer rule strips. Exceptions: virtual (open/override) composables keep `startReplaceGroup(KEY_fn)`; code compiled without OptimizeNonSkippingGroups (≤2.1.x default [src; exp eras]) keeps `startReplace(able)Group(KEY_fn)`; so do compose ≤1.6 / material3 ≤1.2 AARs [exp 9.4.24] (compose 1.7/1.8 AARs are already markers-only, presumably because androidx enabled the flag before the 2.2 default [inf]). | Gretio: 167 non-restartables with a Composer param, no key [exp Gretio]; `NonRestart` key gone, `Render` key kept [exp 9.4.24]; ~30–80% of current library composables per artifact are marker-only [exp 9.4.24, compose-keys §2]. |
| Bitmask constants (`$changed`/`$dirty`/`$default`) | Survive. Masks index the *original* slots. | `and-int/lit8 …,#19` / `#18`; `0x36db6db6` sits in the static `updateChangedFlags`, which is **not inlined** (§1.2). |
| Synthetic params | Present but **R8 removes or reorders params**, including synthetic ones: `$default` is removed when constant at every call site (73/120 = 61%), a `$changedK` int in 5.5%, and the defaults prologue becomes unconditional. ≥1 real param removed in 40%. Residual order ≠ `real* C ch+ df*` in 26/552. `Greeting (String,int,boolean,C,int,int)` → `(I I Ld/k; Z)V` (8.10) / `(IILq0;Z)V` (9.4); `Many` → `(ILjava/lang/String;Ld/k;III)V`. | mapping `residualsignature` [exp]; library truth n=201 [exp Gretio]; `compose_shapes`: `Ten`/`Eleven`/`Constants`/`Remembers`/`Lambdas` → `(I, C)V` [exp eras]. |
| `Composer` type | Renamed. The interface gets **merged into `ComposerImpl`** (`Ld/k;`), as the keep rule intends ([src] rules.keep 7–15). Runtime methods can have **reordered params** (`shouldExecute` is `(ZI)Z` in the fixture, `(IZ)Z` on Gretio). | every residual signature has `Ld/k;`; Gretio `Lbl4;` (super `Object`, no interfaces) [exp Gretio]. |
| `startRestartGroup` / `endRestartGroup` / `updateScope` | R8 9.4 **discards the `startRestartGroup` result** (0/552 `move-result`) and inlines `updateScope` into `iput-object lambda, Scope.<field>` after `endRestartGroup` + `if-eqz` (584 of ~590 sites). 8.10 kept the `updateScope(Function2)` call. | `compose_shapes` `Card.Show` [exp 9.4.24]; [exp Gretio]. |
| Non-restartable composables | Often **inlined** into callers (`rememberDouble`, `NonRestart`, `readOnly` → into `Screen`). | mapping inline frames |
| Restartable composables | Survive as methods (≥2 call sites: caller plus own restart lambda); **never R8-inlined** in practice (0/96 probe, 0/552 Gretio: every `startRestartGroup` is on the Composer param at the entry), but **moved to unrelated host classes** (MainKt statics → `ComposableSingletons$EntryKt`, 8.10; `kotlin.jvm.internal.TypeIntrinsics`, `ContinuationInterceptor$Key`, 9.4). | mapping [exp; exp 9.4.24; exp Gretio] |
| Restart lambdas | 8.10: `$$ExternalSyntheticLambdaN` classes, **horizontally merged** (`m.c` = 3 lambdas). 9.4: `Outer$N`, merged **together with composable-lambda bodies and ordinary lambdas**: ctor `(Object…, I, B)` with a byte class id, `invoke` switches on it; the own int arg gets only a `getClass()` null check. Gretio: 290 classes, 91 merged groups serving 2–23 composables. | mapping [exp]; [exp 9.4.24]; [exp Gretio] |
| `ComposableSingletons$FileKt` | Lambda body classes merged with runtime synthetics; `composableLambdaInstance` inlined to `new ComposableLambdaImpl(key, false, block)` in some class's `<clinit>`. On 9.4 the fields and `<clinit>`s land in **mixed host classes** (Gretio: 152 fields in 55 hosts, 1 pure holder), so the class is **not recoverable**; only the field name `lambda$K` is. | dump `m.a.<clinit>` [exp]; [exp Gretio] |
| `$stable` fields | **Removed** (never read). | `Static fields -` on all app classes. Gretio: no field named `$stable`, but R8 renames fields anyway [exp Gretio]. |
| `@StabilityInferred`, `@FunctionKeyMeta`, `@Composable` | Not in DEX at all (BINARY). | [src] |
| Live literals | `LiveLiterals$…` and key strings folded away when `isLiveLiteralsEnabled` is never set [inf]. The `@LiveLiteralInfo` annotations are RUNTIME but go with their removed methods [inf]. | — |
| Kotlin metadata | Stripped unless kept; if kept, it holds the **pre-lowering** signature [inf]. | — |

**Structural fingerprints that survive** (all α-invariant):

1. **Composer class `C`.** The class `T` with the most `T.x(I)T` calls made with a constant on a `T`-typed parameter
   (Gretio: 552 vs 15 for the runner-up) [exp Gretio]. An equivalent check is the type most often followed by trailing
   `int` params (579 vs 263). Parameter **position is not reliable** (R8 reorders, e.g. `(IILze;…Lbl4;…Z)V`). The
   `startRestartGroup` result is **not** written back on 9.4. Restartable methods end with `invoke C.y()LS;` + `if-eqz` +
   `new-instance L` / `<init>` + either `S.z(LFunction2;)V` (8.10) or `iput-object L, S.<field>` (9.4)
   (`endRestartGroup`/`updateScope`). A runtime SigDB (§5.4 of DESIGN) can also pin `ComposerImpl` by body shape.
   Detection by the name `Landroidx/compose/runtime/Composer;` fails on real R8 apps (Gretio is not detected today).
2. **Restartable composable.** An entry `startRestartGroup(const)` on a `C`-typed parameter, plus a restart-lambda
   (switch case) whose invoke calls this method back with the pass-through pattern (§1.2). Found for 552/552 on Gretio.
   In 38 cases the (merged) lambda class calls `M` more than once; the restart call is the one with `updateChangedFlags`
   args [exp Gretio].
3. **Composable lambda.** `new ComposableLambdaImpl(const int, const boolean, <lambda>)` (inlined
   `composableLambdaInstance`) or the `rememberedValue`/`Empty`/`new ComposableLambdaImpl`/`updateRememberedValue` sequence
   (inlined `rememberComposableLambda`) [exp]. Gretio: 259 sites, 152 in `<clinit>` (`false`), 107 inlined remember (`true`).
4. **Group constants.** `startReplaceGroup(const)`, `startMovableGroup(const, x)`, `C.shouldExecute(Z,I)Z` (any param
   order), `skipToGroupEnd`.
5. **`$changed` slot masks** of the forms `0b110<<(3s+1)`, `4<<3s|2<<3s`, `0x12492492…`, and **`$default` single-bit tests**
   selecting a parameter's value. After R8 the selection is a **phi** (`v = bit ? <default> : p` into a fresh register; one
   arm is `move r, p`), not a store into the parameter register [exp Gretio].
6. **`updateChangedFlags`**: a static `(I)I` holding the masks `0x12492492`/`0x24924924`, called from every restart lambda.

**What does not survive.** Function names, file names and lines (except via trace strings when they survive),
`N()`/`P()` param info, the `$stable` stability table, the Kotlin-level arity if R8 removed params, and any function key of a
non-virtual non-restartable composable (above). On Gretio the only app-level name evidence for composables was Kotlin
function-reference signature strings (`"DashboardPages$lambda$33$pageShiftOf(…)I"`, a `kotlinc/` concern) [exp Gretio].

**Non-minified builds** (D8 only, `~~D8{…}` marker): everything in §1 survives, including all strings, names and
`$stable`. The `compose/` rules still apply and are mostly S there.

---

## 3. Undo rules

Preconditions are checked per item at runtime. "roles" means `$composer`/`$changed`/`$default` identified by dataflow
(R-roles below), not by position.

**R-roles (shared analysis).** The **restart-lambda callback is the proof** (S for restartables). In the lambda
(switch case) instantiated after `endRestartGroup`, take the call back to the method `M` whose argument list contains
`updateChangedFlags(…)` results (a merged lambda class may call `M` more than once; the restart call is the one with `ucf`):
- `$composer` = the parameter of `M` receiving the lambda's own Composer arg (a `check-cast` of an `invoke` param).
- `$changed` = each parameter of `M` receiving `updateChangedFlags(x|1)`, or `updateChangedFlags(const)` when R8 folded `x|1`.
- `$default` = a parameter receiving a pass-through int (instance-field read, or a constant after R8 propagation) that is
  used in `M` only in single-bit tests `x & (1<<i)` which **select a value** (phi, fingerprint 5) or OR Static bits into `$dirty`.
- Remaining pass-through params are real params.

Gretio: `$composer` and `$changed` for 552/552 restartables (596 ints), `$default` for 95; on the 201-method library truth
0 over-counts and 0 real params misclassified [exp Gretio]. In-method dataflow alone (slot masks, `&1` into
`shouldExecute`) agrees exactly in only 480/552 (87%) and is **not** proof. In particular **`x & 1` is ambiguous**: it is
the force bit of `$changed` *and* bit 0 of `$default` (same instruction); only the lambda signal, or "feeds
`shouldExecute`/`$dirty`" versus "selects a value", separates them. Non-restartable composables and lambda bodies have no
restart lambda: their roles are D (dataflow only).

Classification is by role, so it does not care about R8 argument reordering. It is α-invariant because it uses no names.

| id | class | preconditions | evidence / proof |
|---|---|---|---|
| `compose/detect` | D | C found structurally (fingerprint 1: argmax of `T.x(I)T` const-on-`T`-param, with a clear margin) and ≥1 method matches fingerprint 2 | positive structural evidence (DESIGN §1.1). **Must not** depend on the name `Landroidx/compose/runtime/Composer;` (R8 renames it; Gretio undetected today). Also identifies `endRestartGroup`, `updateChangedFlags` (masks), `ComposableLambdaImpl` (`<init>(IZ…)` const/const) and `shouldExecute` (`Z` return, `dirty&1` arg) by shape, never by descriptor. |
| `compose/runtime-api` (implemented, `passes/compose_api.rs`) | **S** (member names) | C found (as `compose/detect`); per role a clear winner (≥4× the runner-up) among the composer's methods voting over the restartable composables | names from the plugin's call shapes (`compose.rs::roles`): `startRestartGroup` (the entry call); `shouldExecute(ZI)Z` (skips when false) or `getSkipping()Z` (compiler ≤2.1, skips when true), with the skip path's first composer `()V` call as `skipToGroupEnd` (a `()Z` whose path starts otherwise, e.g. `getInserting`→`createNode`, gets no vote); `rememberedValue()Object` compared, then `updateRememberedValue(Object)V`; `(prim)Z` results branched on → `changed` (every overload); `(Object)Z` → `changed`, split from `changedInstance` by body (`equals`/`(Object,Object)Z` call vs identity `if-eq`); `endRestartGroup` = the object result null-tested; `updateChangedFlags` = the static `(I)I` whose result is an argument of a call to a restartable composable (the restart lambda). Override groups (composer supertypes/subtypes) are named together. Graded against compose_basic, compose_basic_r94, compose_shapes, compose_shapes_k21 (Kotlin 2.1.21) mappings, and on the D8 twins every role names a method already called that. Gretio: 13 members. Not yet: replace/movable groups (R8 folds `startMovableGroup`/`endReplaceGroup` into helpers), `updateScope`, `ComposableLambdaImpl`, `Composer.Empty`, the class names (needs S class-name values in the renamer). |
| `compose/composable-marker` | **S** (restartable) / D (others) | fingerprint 2 holds for this method | only the Compose compiler emits `startRestartGroup(K)` + a self-calling restart lambda. Output: report flag and a build-visibility annotation sharing the `@eightr.Inlined` plumbing, e.g. `@eightr.Composable(key=K, restartable=true, changed=[…], defaults=[…], removedParams≥n)`. Gretio: 552 S + ~534 D lambda bodies [exp Gretio]. |
| `compose/synthetic-param-names` | **S** (role names); index `K` of `$changedK` D unless fixed by `param-slot` | roles proven by the restart-lambda callback (R-roles) | names are compiler constants `$composer`, `$changed`, `$changedK`, `$default`, `$defaultK` ([src] `CPT` 705–723). Written as parameter names / debug-info names. The count formula (`c = ceil((n+t)/10)`, `d`) is **not** a precondition: R8 removes constant `$default` (61%) and `$changedK` (5.5%) [exp Gretio], so the count check rejects correct cases; it is only a consistency check. With two `$changed` ints the residual order does not prove which is `$changed1` (44 Gretio methods) unless a slot binding and a default binding meet on one param (`10K+s = i+recv`). |
| `compose/source-signature` | **S** if an `N(...)` string is bound to the method and its count equals the remaining real params (after removing receivers); **D** otherwise | roles identified | `N()` lists every named param ([src] `CFBT` 4964–4990). Without it R8 may have removed params (seen [exp]), so arity is not provable. D output = remaining params in residual order, with names from other rules. Recorded in the mapping/report; **`dex'` keeps the synthetic params** (§0 note 2). |
| `compose/original-arity-bound` | D (annotation) | roles identified | lower bound on the original slot count = max of `10·(c−1)+1` for `c` `$changed` ints, the top slot of the skip mask (incl. `const`+`and-int/2addr` masks like `0x12492493`), and top `$default` bit + 1 + receivers. A gap (slot with no live param) proves R8 removed a param at that position. Sound on the library truth (0 violations / 201; detects 57 of 80 true removals); Gretio: 109/552 provably lost params [exp Gretio]. Fixture: `Greeting` gap 1, `Many` gap 10, exact [exp 9.4.24]. |
| `compose/param-slot` (new) | **D** per bound param; S only under a unique-window precondition once measured at 100% | roles identified | **slot binding:** `if (($changed_q & (6<<3s \| 14<<3s)) == 0) dirty \|= changed(p) ? 4<<3s : 2<<3s` ties residual param `p` to original slot `10·k(q)+s` (dispatch `this` = last slot, §1.1). **Default binding:** the phi `bit = $default & (1<<i); v = bit ? <default> : p` ties `p` to original regular index `i`. `slot − bit` = receiver count (resolves §5.3). Precision: D8 library slots 99.2% (members only with `this` last), defaults 94% (error mode: duplicate bindings from overlapping windows); Gretio vs library truth 96.5% / 158 of 160. Gretio coverage: 46% of residual real params placed, 134 methods fully placed [exp 9.4.24; exp Gretio]. Output: Kotlin-shaped signature sidecar (`fun ?(p0: T, /*removed*/, p2: T = …)`). |
| `compose/fn-name/sourceinfo` | **S** | a `sourceInformation(C, s)` or `sourceInformationMarkerStart(C, K, s)` call is the **first** such call after the method's entry group, `s` parses as function-info, has `(Name)`, and **for markers `K` equals the method's own entry key** | compiler-emitted exact simple name ([src] `CFBT` 4866–4875). Later `CC(Name)` strings in the body belong to **inlined inline composables** (kotlinc inlining). They must not name the method, but they label regions (`compose/inline-region`). |
| `compose/fn-name/trace` | **S** (simple name) | `traceEventStart(K, …, s)` with `K` == the method's entry-group key, and `s` parses uniquely (see `compose/trace-parse`) | `kotlinFqName` last segment ([src] `CFBT` 2107–2138). For lambdas the last segment is `<anonymous>`, which gives no name but does give the **enclosing function's fq name**. |
| `compose/trace-parse` | N→S | trace string present | Regex `^(.+) \((.+):(-?\d+)\)$` is ambiguous when the file name or a backticked function name contains `" ("`. **Enumerate** every split at each `" ("` occurrence whose suffix matches `(.+):(\d+)\)$`, ordered by split position. The set is exhaustive because the true split is one of those occurrences. **Filter**: file ∈ {sourceInfo file, SourceFile attr}, or the `KEY_fn` hash check. One survivor → S. Otherwise D: use the rightmost split and annotate. |
| `compose/fqname-split` | N→S | fq name from trace | Candidates: every split of the dotted fq name into `package` + `Class(.Nested)*` + `fn` (for top-level: `package` + `fn`). That is `k+1` candidates for `k` dots, which is exhaustive (JVM names cannot contain `.`). **Filters**: (a) `#pkgHash` from any sourceInformation in the same file, or (b) the `KEY_fn` hash with a known signature, or (c) Kotlin metadata if kept. Exactly one survivor → S package + class chain. |
| `compose/param-names` | **S** | `N(...)` bound to the method (as in `fn-name/sourceinfo`) | exact names in declaration order, receivers excluded. Inline-class params carry `:<fqn>` with `c#` = `androidx.compose.` → also S **type** names for those params. |
| `compose/param-order` | **S** (a permutation only) | `P(...)` bound to the method | gives each param's rank in name-sorted order. It cross-validates names from `checkNotNullParameter` (DESIGN §5 item 3), and inline-class entries give S type FQNs. |
| `compose/source-file` | **S** per method; class-level only if all methods hosted by the class agree **and** the class is not an R8 merge/host target | file from `…:File.kt#h` or `(File.kt:L)` | compiler-emitted `file.name`. The class-level SourceFile attr is D because R8 moves statics across classes ([exp]: MainKt methods hosted by `ComposableSingletons$EntryKt`). |
| `compose/source-line` | **S** for the function's body-start line (`L+1`); **S** for call-site lines listed in sourceInformation, D for mapping them to instructions | trace or sourceInfo | lines are 0-based ([src] `getLineNumber`; [exp]). Assigning the *k*-th location to the *k*-th composable call is D, because R8 reorders code. |
| `compose/package` | **S** when `fqname-split` resolves, or when a candidate package's hash equals `#pkgHash` and the candidate comes from an exhaustive set | — | `pkgHash` = 31-fold of the package string, abs, base36 ([src] `CFBT` 5008–5020; [exp] `com.example.app`→`bw24ds`). The hash alone is never inverted. |
| `compose/key-confirm` | **S** when used as a filter on an exhaustive finite candidate set with exactly one survivor. Otherwise it is only a consistency check: it can **refute** (demote S→D on mismatch). | all components of the durable key string are candidate-known (name, simple type names of params and return, receiver, class chain, package, file basename) | Applies only where a key survives: restartable and virtual composables (§2). `KEY_fn = hash("fun-…/…/pkg-…/file-…")` (§1.5, reproduced exactly [exp]). Mind the K1/K2 variant for composable-lambda params (§1.5). A mismatch **proves** some component is wrong. A match on the only enumerated candidate proves it (the true string is in the set and hashes to `K`). |
| `compose/default-args` | **S** "param *i* had a default value" when a `$default` bit-*i* test exists; **D** for the default expression (R8 may have optimized it); absence proves nothing | roles identified (bound to a param via `param-slot`'s default binding). Recall is low on real code: R8 removed `$default` in 61% of library composables that had one (library recall 47/120); Gretio: 93 methods [exp Gretio] | the prologue exists only for params with defaults ([src] `CFBT` 1356–1410). Record `(param i → default expr region)` in the report, and in the Kotlin-signature sidecar as `= <expr>`. |
| `compose/restart-lambda` | **S** (role) / D (name) | a synthetic lambda class, or one switch case of an R8-merged lambda class, whose invoke is the pass-through self-call with `updateChangedFlags` args (§1.2) | label it "restart scope of F"; it is also the proof for R-roles. The K2 original body name is `F$lambda$N`, where N is the per-function lambda index, so it is D (index order is not provable after R8). On 9.4 the case may stay in an unspecialized switch when `r8/split-merged-class` misses dispatch through a copy of `this` (148/552 on Gretio before that fix). |
| `compose/singletons` | **Field names only.** `lambda$K` is S when compiler ≥2.1.20 is established (see §5.7), else D | `<clinit>` builds `ComposableLambdaImpl(K,false,…)` into a static field | Gretio: 152 fields [exp Gretio]. The class name `ComposableSingletons$<FileKt>` is recoverable only in unminified / 8.10-style output where the class is a pure holder; on R8 9.4 the fields live in mixed host classes (55 hosts, 1 pure), so the class part is dead. Names from [src] `CLM` 317–345, 835. |
| `compose/lambda-name` | S (enclosing fq name only) | trace string of a lambda | `Outer.fn.<anonymous>` names the enclosing function. |
| `compose/inline-region` | S (region identity), D (bounds) | `sourceInformationMarkerStart(C, K, "CC(Name)…:File#h")` … `End` inside a body | an evidence source for kotlinc `inline`-function un-inlining (a `kotlinc/` concern): exact callee simple name, file, package hash. Only present when source info survived. |
| `compose/lib-key` | D hint (conditions 1–4); **S for the method name** with corroboration (5) | library **restartable** composables in the app (material3, foundation, ui…). Conditions: (1) C found structurally; (2) `K` is the const argument of the method's **first `C.x(I)C` call** (not "first int const": failed 2/194 on Gretio where R8 hoisted a small const); (3) `abs(K) ≥ 2²⁰` and `K` has DB role *entry-restartable* with exactly one normalized function `F`; (4) **order-free** shape check: `\|params(M)\| ≤ arity(F) + [this] + 1 + nch + ndef`, ≥1 int param, reference-type params a sub-multiset of `F`'s (never positions: R8 reorders, 10/192 failed a positional test); (5) a second key of `F` (inner RPG/MG/LAM key, or its content lambda's key) occurs in `M` or a lambda class `M` instantiates | DB `K → (owner, name, desc, role, first/last version)` from the last patch of every minor (compose 1.5–1.12, material3 1.1–1.4: ~7k survivable keys, 1,241 distinct restartable entry keys, 0 genuine collisions), with K1 and K2 variants (§1.5). Expected false positives with 1–4: ~2·10⁻⁴ per app (574 entry-position consts × 1,241 keys / 2³²), ~10⁻³ at a 50k-key DB. The earlier "~1%" was the any-constant × any-key rate (0.011), not the entry-position rate. Recovers name, owner and Kotlin signature for the report/sidecar; **never rename the host class** after `F`'s owner (R8 re-homes composables). Version: per artifact, argmax over minors of the recall of identified functions' inner/lambda key sets (resolves the minor; patches are key-identical) [exp 9.4.24; exp Gretio]. Gretio: 194 entry keys → 167 functions with a 10-artifact multi-version DB; 201/552 with a 25-AAR single-version DB (different DB scopes). Non-restartable library composables have no key (§2). |
| `compose/stable-field` | S (name `$stable`) where the field exists (non-minified or kept) | `public static final int` written only in `<clinit>` with `0`, `8`, or ORs of other such fields | value → stability annotation in the report (`0` stable, `8` unstable). |
| `compose/live-literals` | S | `liveLiteral("<Type>$<path>", v)` strings or `@LiveLiteralInfo`/`@LiveLiteralFileInfo` present | durable paths (§1.5 grammar) name functions, callees, arg indices and the file path. Rare in release builds. |
| `compose/group-offset` | N (reported only; not needed for libraries, whose inner keys the DB holds verbatim) | `KEY_fn` known **and** the function's source length `L` known (from sourceInfo offsets) | for an inner key `k` and each discriminator `c∈{none,2,3}`: `H=(k−c)·31⁻¹ mod 2³²`, `d=H−961·KEY_fn mod 2³²`; candidates `{(a,d−31a) : 0≤a≤d−31a≤L}`, ordered by `a`. Exhaustive by construction. [exp] gave 1–4 candidates per group with `L`=232. If `L` is unknown the set is unbounded, so the rule is identity. Low value. |

**Priorities (value per effort on R8 9.4 apps, compose-app §3; matches PLAN-compose-sigdb M1/M2/M4).** (1) `compose/detect`
plus naming the runtime API by call shape (startRestartGroup, endRestartGroup, shouldExecute/getSkipping, skipToGroupEnd,
replace/movable groups, rememberedValue/updateRememberedValue, changed*, updateChangedFlags, the updateScope field,
ComposableLambdaImpl, Composer.Empty); (2) `restart-lambda` + `synthetic-param-names`; (3) `composable-marker`;
(4) `param-slot` + `default-args` + `original-arity-bound`; (5) `lib-key`; (6) the `r8/split-merged-class` copied-`this`
fix; (7) `singletons` field names. **Zero yield on Gretio** (no strings survive), kept for D8/debug/tracer builds only:
`fn-name/*`, `trace-parse`, `fqname-split`, `param-names`, `param-order`, `source-file`, `source-line`, `package`,
`inline-region`, `lambda-name`, `stable-field`, `live-literals`, `group-offset`, and `key-confirm` for app functions (no
candidate set). App composable **names** are unrecoverable on such apps; keys only verify candidates. No code
simplification is both behaviour-preserving and useful (re-inserting removed params is impossible; restoring canonical
param order would affect 26/552 methods).

**Output placement.** S names go into the output mapping as clean names. Parameter names go into debug info. The
Kotlin-level signature, defaults, stability, lines and inline regions go into the report and the Kotlin-signature
sidecar. Nothing changes executable semantics.

---

## 4. Evidence for original names

| Evidence | Carries | Survives R8? | Class |
|---|---|---|---|
| `sourceInformation(c,"C(Greeting)N(name,times,enabled)10@256L30:Main.kt#bw24ds")` | fn simple name, param names (≥2.2.2x, §1.6) or order (`P`), inline-class param type FQNs, file basename, package hash, call-site lines/offsets | **No** (assumenosideeffects) — only in D8/no-consumer-rules builds | S |
| `sourceInformationMarkerStart(c, K, "CC(Column)…:Column.kt#<h>")` | inlined inline-composable name + file + package hash | No | S (region) |
| `traceEventStart(K, d1, d2, "com.example.app.Screens.Member (Main.kt:69)")` | full Kotlin fq name (package + class chain + fn), file basename, 0-based body line; lambdas: enclosing fq name | **Yes if a tracer is ever set** (runtime-tracing, perfetto integrations) or `-dontoptimize` [exp] | S (split ambiguity §5.2) |
| `KEY_fn` const | hash commits to simple name, *original* param/return simple type names (all 12 for `Many` although 2 survive), receiver, class chain, package, file | **Yes** for restartable and virtual composables [exp; exp 9.4.24]; **no** for other non-restartables | verification only; library names via `compose/lib-key` |
| `KEY` of lambdas | hash commits to the enclosing fn, callee simple name, arg index, sibling index | Yes | verification only |
| `#pkgHash` in sourceInfo | package | with sourceInfo | verification only |
| `ComposableSingletons$<FileKt>` class name, `lambda$<K>` / `lambda-<i>` field names, `getLambda$K$<module>` getter | file facade short name, lambda key, **Gradle module name** | No (renamed) unless kept | S (when unminified) |
| `$stable` field | "this class was compiled by Compose", stability | No (removed) | S (when present) |
| `liveLiteral("Int$arg-0$call-Label$fun-Greeting", …)`, `@LiveLiteralFileInfo("/abs/path/Main.kt")` | durable paths, absolute source path | debug/Live-Edit builds only | S |
| Kotlin metadata (if kept) | pre-lowering signature | only if kept | S |

---

## 5. Ambiguity analysis

1. **R8 changes composable signatures** (param removal, constant propagation, reordering, Composer→ComposerImpl merge).
   [exp] Positional stripping is unsafe (26/552 reordered on Gretio). R8 also removes synthetic ints (`$default` 61%,
   `$changedK` 5.5%) [exp Gretio]. *Resolution:* roles proven by the restart lambda (§3 R-roles), which are invariant to
   reordering and removal. Arity is S only via `N()`, and otherwise a D lower bound (`compose/original-arity-bound`); R8
   arity is only an **upper** bound for DB shape checks. The formula check alone is **not** proof in either direction:
   removing one param from an 11-param function (c=2→n=10 gives c=1) can accidentally satisfy `d∈{0,1}`, and removed
   synthetic ints make correct methods fail it.
2. **fq-name split** (package vs class vs nesting) in trace strings. There are `k+1` candidates. Filters: package hash,
   `KEY_fn`, Kotlin metadata. The Kotlin naming convention (lowercase packages) is **not** proof and is never used.
3. **Extension receiver vs first regular param.** They are indistinguishable in the JVM descriptor and both count toward `c`.
   Candidates are {receiver, not receiver}, plus context params for Kotlin ≥2.2 (0..m leading params may be
   context/extension). *Filters:* `N()`/`P()` length (receivers excluded), `KEY_fn` (`Recv.name(...)` prefix),
   `$default` count when `ceil(n/31)` differs, the `$$this` path segment in lambda keys, and — the one that survives R8 —
   **`slot − bit`** for any param bound by both a slot mask and a `$default` bit (receivers have slots but no default bit;
   `compose/param-slot`). The dispatch receiver is not a candidate here: it takes the last slot (§1.1). Fallback D: treat
   all as regular params.
4. **Trace-string parsing** with `" ("` in file names or backticked function names. Enumerated in `compose/trace-parse`.
5. **Which `sourceInformation` string belongs to the method.** Kotlin inlining of library inline composables, and R8
   inlining of non-restartable composables (seen [exp]), put foreign `C(...)`/`CC(...)` strings and foreign
   `startReplaceGroup(KEY_fn')` groups into a body. *Resolution:* bind only the string attached to the method's **entry**
   group, and require the trace/marker key to equal the entry key. R8-inlined **restartable** composables were never observed
   (0/96 in the probe app, 0/552 on Gretio [exp 9.4.24; exp Gretio]); the restart lambda's self-call keeps a second call site.
   If one did occur, its `startRestartGroup(K')` would appear mid-body and would itself be proof of inlining at that point
   (an `r8/` un-inline hint; with a DB, an *entry-restartable* key found mid-body reads "inlined F here"). Inline-copied
   library keys that survive (pre-1.7 RPLG keys, LAM keys such as `LazyDsl.items`' content lambda) likewise mark inline call
   sites in app code (D hint).
6. **Host class ≠ original class.** R8 moves static composables to unrelated classes and merges `Composer` into
   `ComposerImpl`. Names from trace strings name the *method*. Class names derived from them are D unless §3
   `compose/source-file` class-level conditions hold. Re-homing methods into a reconstructed `FileKt` class is a structural
   D transform (Relocation), and it belongs in the naming phase. `lib-key` names the method, never its host class.
7. **Compiler era** (`lambda-<i>` vs `lambda$K`, `P` vs `N`, `startReplaceableGroup` vs `startReplaceGroup`). A single
   dex can mix eras across libraries. Era is decided **per method** from positive shape evidence only: a
   `shouldExecute` call means ≥2.1.0 (≥2.2 by default; a `getSkipping` call with the same shape means the flag was off,
   typical of 2.1.x) [src; exp eras]; an `N(` string means ≥2.2.2x (§1.6); `rememberComposableLambda` means runtime ≥1.7;
   non-virtual non-restartables wrapped in `startReplaceGroup(KEY_fn)` mean OptimizeNonSkippingGroups off (≤2.1.x
   default). Composable-lambda bodies as anonymous `Lambda` subclasses (2.1.x) vs indy `lambda__K$lambda$0` (≥2.2) are an
   era marker before R8; whether it survives R8 class merging is unverified. Since `shouldExecute` alone does not prove
   ≥2.1.20, `lambda$K` field names on R8 9.4 apps like Gretio stay D unless ≥2.2 is otherwise established.
   Where the era is undetermined, singleton field names are N with candidates `{lambda$K} ∪ {lambda-<i> : i∈[0, #fields]}`. The
   fallback is D (`lambda_<hash>`). The K1/K2 frontend changes keys of functions with composable-lambda params (§1.5); a
   lib-key DB carries both variants.
8. **Hash evidence.** 32-bit hashes (`KEY_fn`, `pkgHash`) never *invert*. They only filter an exhaustive candidate set or refute.
   A match on a non-exhaustive guess (a SigDB hint, a name "that looks right") stays D, but is marked hash-confirmed.
   Accidental matches: any large app constant against any DB key ≈ 0.011 per app (6,520 × 7,060 / 2³²); an entry-position
   constant against restartable entry keys ≈ 2·10⁻⁴ [exp Gretio]. Exclude `abs(K) < 2²⁰` (2 of 7,399 DB keys). See
   `compose/lib-key`.
9. **Anonymous lambda params** (`_`) are missing from `N()` ([src]), so `|N| < arity` for lambdas is expected.
   Compare `|N|` with the lambda's real arity (FunctionN − 2) before using it for arity.
10. **Line numbers are 0-based** in both trace and sourceInfo, and the trace line is the *body* start (`{`), not the
    `fun` keyword ([src] FIXME at `CFBT` 2114–2117). Off-by-one bugs here would silently break S. Generated
    `$default` wrappers carry line `-1` [exp 9.4.24].
11. **`x & 1`: `$changed` force bit or `$default` bit 0.** Same instruction. Decide by the restart-lambda role
    (`updateChangedFlags` vs pass-through), or by use (feeds `shouldExecute`/`$dirty` vs selects a value) [exp Gretio].

All filters use strings and int constants only, and ordering is by candidate string or split position. So every rule is
α-invariant (DESIGN §0.3).

---

## 6. Fixture ideas

Each fixture is built at least twice: once plain D8 (the oracle for strings/names) and once R8 with runtime consumer rules
(and a third time with `Composer.setTracer` reachable where trace strings matter). Compile with Kotlin 2.3.x, and where it matters also
2.0.x/2.1.0 (pre-`lambda$K`, pre-`N()`) and androidx 1.5.14 (K1).

**Status (2026-09-25).**
- `compose_basic`, `compose_basic_r94` exist (kotlinc 2.4.20, runtime 1.10.6, R8 8.10.9 / 9.4.24). They are built
  **without** `sourceInformation=true` (the kotlinc CLI default), so their D8 twin has only trace strings; they cannot
  grade the sourceInfo rules.
- **`compose_shapes` exists** (R8 9.4.24, `sourceInformation=true`, `main` drives a real Composition for the ART harness)
  and covers **F1, F2, F4, F7, F9, F11, F12, F14, F19, F21**. Its D8 twin has `C(Name)N(...)` for every composable and is the
  ground truth for roles/param names.
- F22 (`era-mix`) was done out of tree only (compiler-eras.md: 2.1.21–2.4.20; 2.0.21 did not run on JDK 26).
- **Planned `compose_lib`** (PLAN M0): material3/foundation composables at pinned versions with a main, the oracle for
  `compose/lib-key` and version voting; it replaces F8 for library inline composables. Also add a restartable composable with an
  extension receiver + defaults (F5) to test `slot − bit`.

| id | program | isolates |
|---|---|---|
| F1 `compose-min` | `@Composable fun A() {}` | c=1 with n=0; restartable skeleton; `KEY_fn` for `fun-A()Unit/pkg-…/file-…` |
| F2 `params-10-11` | functions with 10 and 11 params, plus a member with 10 params (t=1) | `$changed1` boundary; `thisParamCount` effect |
| F3 `defaults-31-32` | 31 and 32 params with defaults (mirror `test31Parameters` golden) | `$default1` boundary; bit index = regular index |
| F4 `defaults-static-vs-dynamic` | `x: Int = 0` vs `x: Int = remember{…}` | static-default Static bits vs `startDefaults/defaultsInvalid` |
| F5 `ext-receiver` | `@Composable fun Foo.Bar(a: Int)` vs `fun Bar(f: Foo, a: Int)` with defaults near 31 | §5.3 ambiguity; `KEY_fn` `Foo.Bar(Int)Unit` resolves it |
| F6 `context-params` | Kotlin 2.2 context parameters on a composable | JVM order; `N()` content |
| F7 `nonrestartable` | `@NonRestartableComposable`, `@ReadOnlyComposable`, non-Unit return; with and without `-featureFlag=-OptimizeNonSkippingGroups` | outer replace group vs markers; R8 inlining |
| F8 `inline-lib` | caller of foundation `Column{}` (library inline composable) | `CC(Column)` markers in caller; `compose/inline-region`; binding rule §5.5 |
| F9 `lambdas` | capturing + non-capturing composable lambdas; two sibling calls to the same callee | `rememberComposableLambda`; `ComposableSingletons$FileKt.lambda$K`; sibling `:1` path; lambda trace names |
| F10 `lambda-arity-23` | 23-param composable lambda | `ComposableLambdaN` |
| F11 `control-flow` | if/when/loop/early return/`key(a,b)` | replace/movable groups, `joinKey`, `currentMarker/endToMarker`; `compose/group-offset` |
| F12 `remember-intrinsic` | `remember{}`, `remember(k){}`, `remember(k1,k2,k3){}` | intrinsic remember shapes; runtime `remember` key constants |
| F13 `stability` | `class A(val x:Int)`, `class B(var x:Int)`, `class G<T>(val t:T)`, class with external-module field | `$stable` values `0`/`8`/expr; R8 removal |
| F14 `r8-param-removal` | composable called once with constants; one with an unused param | the `Greeting`/`Many` residual-signature phenomena; `original-arity-bound`; role analysis under reordering |
| F15 `r8-host-move` | two files, top-level composables only | statics relocated into another file's class; `source-file` class-level refusal |
| F16 `trace-reachable` | F9 + `Composer.setTracer(...)` | trace strings survive; `trace-parse`, `fqname-split` |
| F17 `weird-names` | file `Main (1).kt`, backticked ``fun `a (b)`()`` | `trace-parse` enumeration |
| F18 `same-name-packages` | `a.b.C.f` vs package `a.b.c` + class `C` | `fqname-split` filter by pkgHash and `KEY_fn` |
| F19 `open-default` | abstract/open composable with defaults; interface default | `ComposeDefaultImpls.<name>$default` wrapper; `requiresDefaultParameter=false` |
| F20 `value-class-param` | composable with `Color`/`Dp` params and value-class defaults | `N(x:c#ui.graphics.Color)`; default stubs; long-packed params |
| F21 `property` | `val x: Int @Composable get()` | `@JvmName` getter keeps the name |
| F22 `era-mix` | the same source compiled with 1.5.14 (K1), 2.0.20, 2.1.20 and 2.3.x into four modules of one app | per-method era detection §5.7 |
| F23 `live-literals` | debug build with `liveLiteralsEnabled` | `compose/live-literals` strings and annotations |

---

## Appendix A — reference snippets

```text
java String.hashCode: h = 0; for each UTF-16 unit u: h = 31*h + u (int32 wrap)
KEY_fn   = hash("fun-" + sig + path)        // §1.5
pkgHash  = abs(fold31(packageFqName)) → base36    (Kotlin Int.absoluteValue; MIN_VALUE stays negative)
changedCount(n,t) = (n+t==0) ? 1 : ceil((n+t)/10);  defaultCount(n) = ceil(n/31)
slotBits(state, s) = state << (3*(s%10) + 1)
updateChangedFlags(f) = (f & ~0x36DB6DB6) | (lo | (hi >> 1)) | ((lo << 1) & hi), lo=f&0x12492492, hi=f&0x24924924
```

## Appendix B — open questions for fixtures

- K1 (1.5.x) restart-lambda class naming and field layout (`$changed`/`$default` captured field names). [inf]
- Whether AGP's own Compose configuration overrides `includeSourceInformation` for release variants
  (`isDisableIncludeSourceInformationForAgp`). [src shows the switch exists, but not its trigger]
- Whether `runtime-tracing`'s startup initializer is the usual reason trace strings survive in production apps. [inf]
- R8 behaviour when inlining a *restartable* composable: not seen in 0/96 probe and 0/552 Gretio restartables (§5.5);
  still possible for a single-call-site function whose restart lambda R8 folds away.
- First Kotlin version emitting `N(...)` (between 2.1.21 and 2.2.21; §1.6).
- Whether the slot order "dispatch `this` last" (§1.1) matches `CFBT` source; it is so far [exp] only.
- Whether era markers (anonymous vs indy composable-lambda bodies) survive R8 class merging (§5.7).
