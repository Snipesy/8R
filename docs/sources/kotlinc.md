# Source: kotlinc JVM backend lowerings, and Kotlin metadata as evidence

Status: research notes. Input for `crates/eightr-rules` under the source id `kotlinc`.
Contract: DESIGN.md §0 (S / D / N, α-invariance) and §1 (rulebook). The input is **no mapping**
throughout: only a release DEX, usually R8-minified.

Undo order: `r8` → desugaring (`d8`) → compiler plugins (`kxs`, …) → **`kotlinc`**. So the
`kotlinc` rules run last, on a program whose R8 renaming has already been labelled S/D per
attribute. A `kotlinc` rule that needs "this call is `Intrinsics.checkNotNullParameter`" depends on
the anchor rule `kotlinc/anchor-stdlib` (§2), because `kotlin-stdlib` is *program* code in an
APK and R8 renames it too.

What "undo" means here. Most kotlinc lowerings produce the JVM form itself, and 8R emits DEX, so
8R cannot re-sugar a coroutine or a `when`. For kotlinc, "undo" mostly means three things:
(a) **restoring compiler-given names** (`component1`, `copy$default`, `label`, `L$0`,
`$EnumSwitchMapping$0`, `box-impl`, …), (b) **harvesting original names that the lowering leaked
into strings** (parameter names, property names, class FQNs, JVM signatures), and (c) **annotating
Kotlin-level structure** in the report ("data class with properties name, age, tags").
Re-synthesising a `@kotlin.Metadata` annotation from the recovered facts is an option for later
work (§14, Q5).

## Provenance

| Artifact | Revision |
|---|---|
| JetBrains/kotlin (sparse: `compiler/ir/backend.jvm`, `compiler/backend/src`, `compiler/ir/backend.common/.../lower`, `core/metadata*`, `libraries/kotlinx-metadata`, `libraries/stdlib/jvm/{runtime,src}`) | `06003680c56d09dffcf82b3817372c5aaea66b50` (2026-09-24) |
| r8 (`https://r8.googlesource.com/r8`, full history, blobless) | `fa9ef658c12419cc989fef454d3ee7d07b93a376` (2026-09-25) |
| kotlinc release | **2.4.20** (`kotlin-compiler-2.4.20.zip` from GitHub releases), run on JRE 26 |
| R8 "old" | **8.10.9-dev** (`~/Android/Sdk/build-tools/36.0.0/lib/d8.jar`) |
| R8 "new" | **9.5.20-dev** (`com.android.tools:r8:9.5.20-dev` from Google Maven) |
| Consumer rules checked | `kotlin-stdlib-2.4.20.jar` (Maven Central), `kotlin-reflect-2.4.20.jar`, `kotlinx-coroutines-core-jvm-1.10.2.jar` |

Unless a path says otherwise, Kotlin paths are relative to the kotlin repo root. `L/` is short for
`compiler/ir/backend.jvm/lower/src/org/jetbrains/kotlin/backend/jvm/lower/`, and `CO/` is short
for `compiler/backend/src/org/jetbrains/kotlin/codegen/coroutines/`. R8 paths are relative to
`src/main/java/com/android/tools/r8/`.

The experiments live in the agent scratchpad
(`/tmp/claude-1000/-home-snipesy-8R/4b02079c-3b0c-464c-bf8a-78f254b90fa3/scratchpad/agents/kotlinc/`).
The sources are `src/com/example/{Samples,Main}.kt`, `src2/p/Coro.kt`, `src3/q/Misc.kt`,
`src4/r/P.kt`, `src5/`, and `src6/s/{A,B}.kt`. The R8 outputs are `r8old/`, `r8new/`,
`r8new-keep/`, `r8new-rm/`, `r8meta/`, `r8cf/`, `r8misc/`, `r8coro-*`, `r8c5-*` and `r8p/`, each
with `dump.txt` from `dexdump`. `tool/Dump.kt` is a `kotlin-metadata-jvm` pretty-printer used to
decode `d1`/`d2` before and after R8 (`meta-orig.txt`).

Legend: **[V]** = verified by compiling with kotlinc 2.4.20, running R8, and dumping
(javap / dexdump / metadata decoder). **[S]** = read in source at the revision above. **[I]** =
inferred (older versions from memory, or reasoning not yet tested). Every [I] item needs a fixture
before a rule depends on it.

---

## 0. Headlines

1. **The `checkNotNullParameter(p, "name")` string is no longer reliable evidence.** Since R8
   **9.0.26** (commit `51dd41e7e6`, 2025-11-24, "Add support for -processkotlinnullchecks"), R8's
   *default* is `remove_message`. Every Kotlin null-check intrinsic whose argument can't be proven
   non-null is rewritten to `receiver.getClass()`, and the name string disappears [V: R8 9.5.20
   drops `"who"`, `"<set-?>"`, `"toUpperCase(...)"`, while R8 8.10.9 keeps them; S:
   `shaking/ProguardConfiguration.java` `ProcessKotlinNullChecks.isRemoveMessage()` returns true
   for `DEFAULT`, and `ir/optimize/CheckNotNullConverter.java`]. DESIGN.md §1's row "Kotlin
   intrinsics parameter names: **S** without mapping" holds only for R8 < 9.0.26, or when an app
   sets `-processkotlinnullchecks keep`. The rule stays S where the string exists, but coverage on
   modern APKs will be near zero. The `getClass()` residue is **not** proof of a Kotlin check,
   because R8 emits the same instruction for its own null checks (§3.3).
2. **Lateinit names survive R8 in every configuration tested.** `throwUninitializedPropertyAccessException("db")`
   is either kept as a call with `"db"`, or inlined and constant-folded into the literal
   `"lateinit property db has not been initialized"` [V both forms: `r8new/`, `r8misc/`].
   `-processkotlinnullchecks` does not cover it (it is not in `Kotlin.Intrinsics.isNullCheck`) [S].
3. **Callable-reference and delegated-property strings carry original names *and* original JVM
   descriptors.** Examples are `FunctionReferenceImpl(…, "load", "load(Ljava/lang/String;Lkotlin/coroutines/Continuation;)Ljava/lang/Object;", …)`
   and `MutablePropertyReference1Impl(…, "observed", "getObserved()I", …)`. R8 does not rewrite
   them [V: `r8new/`, `r8misc/`; S: no string rewriting for these in `kotlin/`]. The descriptor
   names the *original FQNs* of every parameter and return type. That is S evidence for other
   classes too.
4. **The value-class mangling hash is a verifiable 40-bit fingerprint of the parameter types.**
   The suffix is `md5base64(sig)`, where `sig` is built only from the value-class FQNs, `_`
   placeholders, `?` nullability and `:`return. The function name is **not** hashed [S:
   `compiler/backend/src/org/jetbrains/kotlin/codegen/state/inlineClassManglingUtils.kt`; V: 7 of
   7 names recomputed exactly]. Wherever a mangled name survives (signature strings, kept APIs),
   8R can *verify* candidate value-class FQNs. It can't *recover* the function name.
5. **`@kotlin.coroutines.jvm.internal.DebugMetadata` leaks the original class FQN, method name,
   file name and line table of every suspend function.** R8 does not rewrite it (no reference to
   `DebugMetadata` anywhere in `r8/src/main/java`). It survives in R8 **compat** mode when the
   annotation type is live, which it is whenever `BaseContinuationImpl.getStackTraceElement()` is
   reachable, as with kotlinx.coroutines stack-trace recovery. It is stripped in full mode [V:
   `r8c5-compat/` has `c="p.Loader" f="Coro.kt" l={4 4} m="load"`, and `r8c5-full/` has none].
6. **Kotlin metadata, when it survives, keeps original property names, parameter names and
   modifier flags, but minified function and class names.** R8 rewrites `KmFunction` names and
   every class reference. It copies `KmProperty.name` and `KmValueParameter.name` verbatim [S:
   `kotlin/ConcreteKotlinPropertyInfo.java:133`, `kotlin/KotlinValueParameterInfo.java:66`,
   `kotlin/KotlinFunctionInfo.java:119-128`; V: `r8cf/` decode]. It only survives on **pinned**
   classes, and only when `kotlin.Metadata` is kept (for example by kotlin-reflect's consumer
   rules) [S: `kotlin/KotlinMetadataRewriter.java:118-129`].

---

## 1. Evidence strength vocabulary (used below)

| Tag | Meaning | Can support |
|---|---|---|
| **proof-string** | A compiler-emitted string whose content is a deterministic function of an original identifier. Hand-written code producing the same string at the same structural position is ruled out by the anchor (e.g. the callee is the stdlib intrinsic, identified by its body). | S |
| **template** | An exact bytecode template the compiler emits (e.g. `copy$default`'s mask ladder). The template's *existence* proves the lowering happened, so the compiler-fixed names are S. The **template assumption** applies: a byte-identical hand-written replica is treated as impossible. | S (compiler-fixed names only) |
| **hint** | A string that correlates with a name but can be overridden by the author (`@JvmName`, `@SerialName`, a custom `toString`, a file name versus `@file:JvmName`). | D `{hint}_{hash}` |

The template assumption is already implicit in DESIGN.md (e.g. "backport templates are S"). It is
stated here because some kotlinc templates can be written by hand in Kotlin, notably a data-class
`toString`. For those, S needs the *ensemble* of templates (§5).

---

## 2. Anchor: finding the (renamed) stdlib runtime

In an APK, `kotlin-stdlib` is program input, so `kotlin.jvm.internal.Intrinsics` becomes e.g.
`Lh/e;` or `Lkotlin/jvm/internal/h;`. R8 may also **reorder arguments**: R8 8.10.9 emitted
`h.e(String, Object)` for `checkNotNullParameter(Object, String)` [V: `r8old/dump.txt`,
`greet`]. Every kotlinc rule therefore depends on:

**`kotlinc/anchor-stdlib`** (S). This rule identifies the residual stdlib methods by body
template plus their stdlib-owned message strings. The strings come from
`libraries/stdlib/jvm/runtime/kotlin/jvm/internal/Intrinsics.java` [S]:

| Method | Anchoring string(s) in its (possibly inlined) body |
|---|---|
| `checkNotNullParameter` / `checkParameterIsNotNull` → `throwParameterIsNullNPE/IAE` | `"Parameter specified as non-null is null: method "`, `", parameter "` (L157) |
| `checkNotNullExpressionValue` / `checkExpressionValueIsNotNull` | `" must not be null"` (L87, L93) |
| `throwUninitializedPropertyAccessException` → `throwUninitializedProperty` | `"lateinit property "`, `" has not been initialized"` (L58) |
| `ContinuationImpl`-family state machines | `"call to 'resume' before 'invoke' with coroutine"` (emitted inline by kotlinc) |
| `BaseContinuationImpl.toString` | `"Continuation at "` |

- Preconditions: exactly one program method matches each template, and it is `static` with the
  expected arity and types (arguments may be permuted).
- α-invariance: the match uses only structure plus stdlib strings.
- Output: S names for these *stdlib* members. The same machinery belongs to a separate
  `stdlib`/SigDB effort. Here it is used only as the anchor.
- If the anchor fails (e.g. the intrinsic was inlined everywhere and removed), the rules below
  match the *inlined* shapes instead (e.g. the folded lateinit literal in §4).

---

## 3. `Intrinsics.checkNotNull*` null checks

### 3.1 Shape [V, S]
- Parameters: `aload p; ldc "<name>"; invokestatic Intrinsics.checkNotNullParameter(Object,String)`
  at method entry, for every non-null, non-primitive parameter of a non-private function. Private
  functions get checks only for operator functions, lambda bodies and indy impl methods
  [S: `compiler/ir/backend.jvm/codegen/src/.../ExpressionCodegen.kt:345-370`].
  - `<name>` is the Kotlin parameter name. Special names occur: `"<this>"` for an extension
    receiver, `"<set-?>"` for a default setter parameter [V].
  - LV ≥ 1.4 uses `checkNotNullParameter` (NPE). Older code used `checkParameterIsNotNull` (IAE),
    selected by `config.unifiedNullChecks` [S: ExpressionCodegen.kt:368].
- Platform-type expressions: `checkNotNullExpressionValue(v, "<text>")`. With
  `noSourceCodeInNotNullAssertionExceptions` (the default in recent versions [I]; we observed
  `"toUpperCase(...)"` and `"toString(...)"` [V]), `<text>` is `"<calleeName>(...)"` for a call,
  or the field name for a field read. Otherwise it is the **source text** of the expression
  [S: `L/TypeOperatorLowering.kt:232-255`].
- `x!!` → `Intrinsics.checkNotNull(Object)`, which has **no string** [V].

### 3.2 What survives R8
| R8 | `-processkotlinnullchecks` | Result [V unless noted] |
|---|---|---|
| < 9.0.26 (e.g. 8.10.9) | n/a (option doesn't exist) | Call and string kept, unless the argument is proven non-null. Apps' own `-assumenosideeffects class kotlin.jvm.internal.Intrinsics {…}` (a widespread copy-paste) removes them entirely [I]. |
| ≥ 9.0.26 | none (DEFAULT) = `remove_message` | Rewritten to `invoke-virtual {p}, Object.getClass()`. **String gone.** |
| ≥ 9.0.26 | `keep` | Identical to old R8. |
| ≥ 9.0.26 | `remove` | Call removed entirely. |

The first R8 tag containing `51dd41e7e6` is `9.0.26`. The multiple-occurrence semantics is
"strongest wins" (`ProcessKotlinNullChecks.meet`) [S]. kotlin-stdlib 2.4.20 ships **no** consumer
rules, and the kotlin repo contains no stdlib `.pro` file [V]. So nothing in the Kotlin toolchain
opts back in.

### 3.3 Rules
- **`kotlinc/nullcheck/param-name`** (S). Precondition: `invoke-static` to the anchored
  `checkNotNullParameter`/`checkParameterIsNotNull`; its object argument is parameter register
  `pᵢ` (not reassigned before the call); its string argument is a `const-string` literal.
  Evidence: proof-string. Output: the debug-info parameter name of `pᵢ` is S. `"<this>"` and
  `"<set-?>"` are *also* S (they are the compiler's names). They additionally prove "extension
  receiver" and "property setter" respectively. The second is a structural fact usable by §9.
- **`kotlinc/nullcheck/expr-callee`** (S for the callee name when the pattern matches).
  Preconditions: the `checkNotNullExpressionValue` string matches `^([^()]+)\(\.\.\.\)$`, the
  checked register is the `move-result` of an `invoke` immediately before it (in the same basic
  block, with no other invoke in between), and that invoke's target is a **program** method.
  Output: the target's simple name = `$1`. Otherwise (source-text form, field form, or ambiguous
  data flow) it's a hint.
  Caveat: the callee is always a *Java* method, because only Java declarations have platform
  types, so this names Java members in mixed apps.
- **`kotlinc/nullcheck/getclass-residue`** (D·id). An `Object.getClass()` whose result is unused,
  on a parameter at method entry, is *consistent with* a removed Kotlin check. R8 also emits the
  same instruction for its own null checks (inlining receivers, `requireNonNull` backports). It
  never yields a name. It can be recorded as a "Kotlin-origin" hint.

### 3.4 Ambiguity
None when the string exists. When it doesn't, the name is unrecoverable from this lowering: do
not enumerate.

### 3.5 Fixtures
`nullcheck-param` (same app built with R8 8.10.9, R8 ≥ 9.0.26 default, `keep`, `remove`: expect
S / absent / S / absent), `nullcheck-extension-receiver` (`"<this>"`), `nullcheck-setter`
(`"<set-?>"`), `nullcheck-platform-call` (Java callee in the app plus a Kotlin caller: expect S
for the Java method name), `nullcheck-argreorder` (R8 8.x reordering: anchor still matches).

---

## 4. `lateinit`

### 4.1 Shape [V, S: `L/JvmLateinitLowering.kt`, `compiler/ir/backend.common/.../LateinitLowering.kt`]
The getter is `v = this.f; if (v == null) Intrinsics.throwUninitializedPropertyAccessException("<prop>"); return v`.
The same pattern appears at every *direct* field read in the class, and for top-level
(`getTopLate` → `"topLate"`) and **local** lateinit variables (`"loc"`) [V: `src3`]. The
default setter carries `checkNotNullParameter(v, "<set-?>")`.

### 4.2 Survival [V]
- Not a null-check intrinsic, so `-processkotlinnullchecks` doesn't touch it.
- R8 either keeps the call with `"db"` (several callers: `r8misc/`, where the pieces
  `"lateinit property "` and `" has not been initialized"` also appear separately), or inlines it
  and folds the constants into `"lateinit property db has not been initialized"` (`r8new/`,
  `r8old/`, `r8coro-*`).

### 4.3 Rules
- **`kotlinc/lateinit/property-name`** (S). Preconditions: anchored intrinsic call with a literal,
  *or* a `new RuntimeException`-subclass/`UninitializedPropertyAccessException`-shaped throw
  whose message literal matches `^lateinit property (.+) has not been initialized$`; and the
  guarded value is an `iget/sget` of field `F` (property lateinit) or a local (local lateinit).
  Output: the Kotlin property name `N` is S (report-level fact).
- **`kotlinc/lateinit/field-name`** (S with precondition, else D). The backing field `F` is named
  `N`, except when `RenameFieldsLowering` resolved a JVM clash (e.g. with a companion field moved
  into the outer class: `N$1`). S requires that the class has no static fields moved from a
  companion (§11) whose names are not S. Otherwise D `N_{hash}`.
- **`kotlinc/lateinit/accessor-names`** (D hint → S if corroborated). The getter is `get`+cap(N)
  and the setter `set`+cap(N) ([S] `JvmAbi.getterName/setterName`; `is`-prefixed names follow the
  `isX`/`setX` rule). But `@get:JvmName` can override them, so the name alone is a **hint**. It is
  promoted to S when a KProperty/metadata signature string (§10, §15) names the same JVM getter.

### 4.4 Ambiguity
A getter can only be identified by the "return F" shape. If two methods both read F and return it
(e.g. a user-written `fun db2() = db`), pick the one with the lateinit guard *and* no other
statements. If that still ties, it's D·id.

### 4.5 Fixtures
`lateinit-member`, `lateinit-toplevel`, `lateinit-local`, `lateinit-many-readers` (non-folded
form), `lateinit-jvmname-getter` (must stay D), `lateinit-companion-clash` (field `db$1`).

---

## 5. Data classes (`componentN`, `copy`, `copy$default`, `toString`/`hashCode`/`equals`)

### 5.1 Shape [V: `User`, `Shape$Circle`, `r/P`; S: `compiler/ir/ir.tree/src/org/jetbrains/kotlin/ir/util/DataClassMembersGenerator.kt`, `compiler/fir/fir2ir/.../Fir2IrDataClassMembersGenerator.kt`]
- `componentI()`: `return this.fᵢ`, in primary-constructor property order.
- `copy(p1..pn)`: `new C(p1..pn)`, with a `checkNotNullParameter` for each non-null ref param.
- `static copy$default(C, p1..pn, int mask, Object)`: for each i, `if (mask & (1<<i)) pᵢ = this.fᵢ`,
  then `invokevirtual copy`.
- `toString`: `"Simple(" + "p1=" + f1 + ", p2=" + f2 + … + ")"`. `Simple` is the **simple name
  only**, even for nested classes (`"Circle(r="`). With jvmTarget ≥ 9 the default is
  `StringConcatFactory.makeConcatWithConstants`, but D8/R8 desugars it back to `StringBuilder`
  with the constants split at the same boundaries (`"P(first="`, `", second="`, …) [V: `r8p/`].
- `hashCode`: `h = f1.hashCode(); h = h*31 + Integer.hashCode(f2); …`. A nullable field uses
  `f == null ? 0 : f.hashCode()`.
- `equals`: `this == o` → true; `!(o instanceof C)` → false; then field by field, `Intrinsics.areEqual` for
  refs, `if_icmp`/`Double.compare` for primitives.
- `data object` (1.9+): `toString()` returns the literal `"Name"`, `hashCode` returns a constant,
  and `equals` is an instanceof check [I].

### 5.2 Survival [V]
Without keep rules, `toString` is often removed or inlined if unused. When it's kept, the literals
survive (`"User(name="`, `", age="`, `", tags="`, `"Circle(r="`, `"Email(s="`, `"Id(v="`). R8's
StringBuilder optimiser can fold adjacent *constant* appends, but it can't fold a field read, so
the `name=` boundaries remain, except where it propagates a constant field value into the template
(`", y=q)"`, M7 review): such a template no longer parses and yields nothing.

**Implemented (M7, `passes/data_class.rs`):**
- `kotlinc/data-class-name` (S) needs `hashCode` and `equals` both reading the template's fields in
  declaration order.
- `kotlinc/data-class-property` (S) additionally needs a surviving `copy`/`copy$default`. Otherwise it
  falls back to `-property-hint` (D). Hand-written Kotlin templates, IDE-generated members and Lombok
  `@Data` produce the same `toString`/`hashCode`/`equals`; Gretio's `PlatformTextStyle` has a typo in its
  hand-written template.

### 5.3 Rules
- **`kotlinc/data/detect`** (S by template). Preconditions: `toString` matches the template with
  field reads f₁..fₙ in the literal order, *and* at least one of `hashCode`/`equals` matches its
  template over the same field sequence (the ensemble guards against a hand-written `toString`).
  Output: "C is a Kotlin data class with properties (N₁..Nₙ)". This is a report fact.
- **`kotlinc/data/property-names`** (S). `Nᵢ` from the literal. Field `fᵢ` is named `Nᵢ` (S,
  with the §4.3 clash caveat). If only `toString` matched (no ensemble), D hint.
- **`kotlinc/data/simple-name`** (S for the simple name). The class's simple name is `Simple`.
  The package and outer class are D (DESIGN §1 agrees). For a nested class, the simple name is S
  and the `Outer$` prefix comes from elsewhere.
- **`kotlinc/data/component-names`** (S by template). A method `()Tᵢ` returning `this.fᵢ` for the
  i-th property, in a class passing `data/detect`, is `componentI`. `componentN` can't be
  `@JvmName`d. Where both a getter and `componentI` survive with identical bodies, which is which
  is **ambiguous** → N: {m₁=getX, m₂=component1} or the swap. There are two candidates, and the
  pipeline uses D (see §5.4).
- **`kotlinc/data/copy-names`** (S by template). The instance method `(T₁..Tₙ)C` doing
  `new C(args)` is `copy`. The static `(C,T₁..Tₙ,I,Object)C` with the mask ladder calling it is
  `copy$default`. The bit i ↔ property i mapping is fixed.
- **`kotlinc/data/getter-names`** (D hint `getN`, since `@get:JvmName` is possible).

### 5.4 Ambiguity: getter vs `componentI`
They are byte-identical (`iget; return`). Enumeration: for each property i whose getter and
component both survive, the candidate set is {(a=get, b=component), (a=component, b=get)}, and
the product over i is exhaustive. Resolution to D: rename both with a structural hash, and tag the
pair "one of {getNᵢ, componentI}". Promotion to S: a call site in a destructuring pattern
(consecutive calls `component1`, `component2`, … on the same receiver) is only a hint, since R8
may inline. A KProperty/metadata getter signature (§10, §15) proves it (S).

### 5.5 Fixtures
`data-basic` (3 props incl. nullable List), `data-nested` (simple-name only), `data-jvm17`
(indy concat → desugared), `data-custom-tostring` (must not be S), `data-getter-jvmname`,
`data-object`, `data-unused-tostring` (removed → no evidence).

---

## 6. `$default` stubs, `DefaultConstructorMarker`, `@JvmOverloads`

### 6.1 Shape [V: `greet$default`, `over$default`; S: `compiler/ir/backend.common/src/org/jetbrains/kotlin/backend/common/lower/DefaultArgumentStubGenerator.kt:84-96,368-397`, `L/JvmDefaultArgumentStubGenerator.kt`, `L/JvmOverloadsAnnotationLowering.kt`]
- For a function `f(p₀..pₙ₋₁)` with defaults: `static f$default([Owner this,] p₀..pₙ₋₁, int mask₀[, mask₁…], Object handler)`.
  There is one mask int per 32 **defaultable** parameters. The bit for the k-th defaultable
  parameter is `1 << (k % 32)` in `mask[k/32]`. Receivers never take a bit.
  - Body: `if ((maskⱼ & bit) != 0) pₖ = <default expr>` for each defaulted parameter, then call
    `f`. For open members, a leading `if (handler != null) throw UnsupportedOperationException("Super calls with default arguments not supported in this target, function: f")`
    names `f` (its Kotlin name) [S: `L/JvmDefaultArgumentStubGenerator.kt:64`]. **That is a proof-string for `f`'s
    name.**
- Constructors: the synthetic `<init>(p…, int mask, DefaultConstructorMarker)`. Sealed and other
  private-ctor classes get `<init>(…, DefaultConstructorMarker)` accessor constructors [V:
  `Expr`, `Op`, `Repo$Companion`].
- `@JvmOverloads`: for each trailing prefix, an overload `f(p₀..pₖ)` doing
  `f$default(this, p₀..pₖ, <zeros>, MASK, null)` with a constant mask [V: `over(String)` → mask
  6, `over(String,int)` → mask 4].

### 6.2 Survival
Kept stubs retain the mask ladder [V: `r8misc/`, `n2.j`]. In unpinned code R8 typically removes
the always-null `Object` parameter (unused-argument removal) and often inlines the stub into
callers [I]. `DefaultConstructorMarker` gets renamed but remains the only never-instantiated class
used as a trailing constructor parameter type. It is identifiable structurally [I].

### 6.3 Rules
- **`kotlinc/default/stub`** (S by template for the relation, S for the name if `f` is S).
  Preconditions: a static method whose trailing params are k ints (plus an optional Object) and
  whose body is exactly a mask-test ladder followed by one call to a method `f` with the leading
  params. Output: the name is `name(f) + "$default"` (S iff `name(f)` is S, else D with the same
  hash suffix as `f` plus `$default`). The mask param names are `mask0`, …, and the handler is
  `handler` (compiler-synthesized, S when debug info is emitted).
- **`kotlinc/default/super-message`** (S). The `"… function: <f>"` literal proves `f`'s name.
- **`kotlinc/default/ctor-marker`** (S for the class name
  `kotlin.jvm.internal.DefaultConstructorMarker` via the anchor; the relation is S).
- **`kotlinc/jvmoverloads/detect`** (S relation). Overloads sharing a name: an equality constraint
  `name(overload) = name(f)`. It gives a name only if one member of the group is S.

### 6.4 Ambiguity
Which default *values* were in the source is fully visible (they're in the ladder). Nothing is
N. If R8 inlined the stub into all callers, it's D·id.

### 6.5 Fixtures
`default-member`, `default-toplevel`, `default-33-params` (two masks), `default-open-member`
(super-call message), `jvmoverloads`, `default-ctor-marker`.

---

## 7. `when`: `$WhenMappings` and strings

### 7.1 Enum `when` [V, S: `L/MappedEnumWhenLowering.kt:66-175`]
- A nested class `Container$WhenMappings` in the **innermost class** using the `when` (a file
  facade counts). It holds `static final int[] $EnumSwitchMapping$i`, with one field per enum
  class, where i is the order in which enums are first encountered while lowering that container.
- `<clinit>`: `arr = new int[E.values().length]`, then for each entry
  `try { arr[E.X.ordinal()] = k } catch (NoSuchFieldError) {}`. Here k = 1, 2, … in first-use
  order across all `when`s of the container (0 is reserved for "unknown").
- R8 recognizes switch maps by the **input** field prefix `$EnumSwitchMapping$` and removes them
  when it can [S: `ir/optimize/SwitchMapCollector.java:85`]. So in output they're usually gone.
  If they survive (e.g. the enum is a library/kept class), the shape does too.

Rules:
- **`kotlinc/when/mappings-class`** (S by template for the simple name `WhenMappings` and the
  outer relation). The distinguishing features against javac's `Outer$1`/`$SwitchMap$pkg$Enum`:
  javac uses one field per enum *named after the enum FQN*, and an anonymous synthetic class.
  Kotlin's class has no ctor calls, is final, and its fields are only int arrays with this
  `<clinit>`.
- **`kotlinc/when/mapping-field-index`** (S when the `<clinit>` order is intact, else N). The
  field initialised i-th in `<clinit>` is `$EnumSwitchMapping$i`. `<clinit>` statements are
  side-effecting static stores, so R8 doesn't reorder them in practice [I]. The N enumeration if
  unordered: all bijections {fields} → {0..m-1}; for m fields that's m! candidates in the
  canonical order given by the structural hash of each field's enum type. Pipeline fallback: D.
- Javac `$SwitchMap$<pkg>$<Enum>` field names embed the **enum FQN**. That belongs to a javac
  source doc, not here.

### 7.2 String `when` [V: `parse`]
`s.hashCode()` → `lookupswitch` on hash → `s.equals("lit")` → branch. R8 may re-emit this
through its string-switch IR [I]. The literals are case labels, not identifiers.
- **`kotlinc/when/string`** (D). Canonical re-sugar annotation only. No names.

### 7.3 Fixtures
`when-enum-same-file`, `when-enum-two-enums` (indices 0/1), `when-enum-library` (mapping
survives), `when-string`, `javac-switchmap` (must be classified as javac, not kotlinc).

---

## 8. Inline (value) classes and the mangling hash

### 8.1 Shape [V: `UserId`, `Email`, `q.Id`, `q.Name`, `q.Holder`; S: `compiler/ir/backend.jvm/src/org/jetbrains/kotlin/backend/jvm/InlineClassAbi.kt`, `…/MemoizedInlineClassReplacements.kt`, `L/JvmInlineClassLowering.kt`]
- Class members with fixed names: `constructor-impl(U)U` (static; returns its arg after the init
  block), `box-impl(U)C` (static, `new C(u)`), `unbox-impl()U`, `equals-impl0(U,U)Z`, and the
  static `toString-impl(U)`, `hashCode-impl(U)` and `equals-impl(U,Object)`. The instance
  `toString`/`hashCode`/`equals` delegate to them.
- `toString-impl`: `"Simple(" + "prop=" + u + ")"` [V: `"Id(v="`, `"Name(s="`, `"Email(s="`].
- **Mangled names** for functions taking or returning value classes:
  `name-<suffix>`, where suffix = `md5base64(sig)` = the first 5 bytes of `MD5(sig)` in URL-safe
  base64 without padding, which is always 7 chars from `[A-Za-z0-9_-]`
  [S: `inlineClassManglingUtils.kt` `md5base64`].
  - New rules (LV ≥ 1.4.30 [I]): `sig = concat(elem(pᵢ)) [+ ":" + elem(ret)]`, where
    `elem(t) = "L" + fqName(erasedUpperBound(t)) + ("?" if nullable) + ";"` if `t` is a value
    class, and `"_"` otherwise. For suspend functions a trailing `"_"` for the continuation is
    appended. The return part is present only for members (not file-facade functions) returning
    a value class [S: `InlineClassAbi.hashSuffix`, `hasMangledReturnType`].
  - Old rules: every parameter is `L<fq>[?];`, joined with `", "`. The return is only hashed if
    no parameter needs mangling.
  - Getters and setters use `getX-<suffix>` / `setX-<suffix>`. The function name is **not** part
    of `sig`.
  - Members *of* the value class without a suffix become `name-impl`.
- Verified recomputations [V, python `hashlib.md5` + `urlsafe_b64encode`]:

| JVM name | `sig` | md5base64 |
|---|---|---|
| `find-liAVW5U(J)` | `Lcom.example.UserId;` | `liAVW5U` |
| `lookupBoth-arMxkCQ(JLjava/lang/String;I)` | `Lcom.example.UserId;Lcom.example.Email;_` | `arMxkCQ` |
| `mk-3jrDLb0(J)J` (member returning `Id`) | `_:Lq.Id;` | `3jrDLb0` |
| `getCur-BOgQTYc()J` | `:Lq.Id;` | `BOgQTYc` |
| `both-VqirUTY(Lq/Id;Ljava/lang/String;Ljava/lang/String;)J` | `Lq.Id?;Lq.Name;_:Lq.Id;` | `VqirUTY` |
| `sus-MRxVC5U(JLkotlin/coroutines/Continuation;)` | `Lq.Id;_` | `MRxVC5U` |

  Note `a: Id?` is passed **boxed** (`Lq/Id;`) yet hashed as `Lq.Id?;`. The top-level
  `makeId(Long): UserId` is **not** mangled (file-class return).

### 8.2 Survival [V]
R8 renames `find-liAVW5U` like any other method (`r8misc/`), so the hash is lost from method
names. It survives:
- in **callable-reference signature strings** (`FunctionReferenceImpl(…, "find", "find-liAVW5U(J)Ljava/lang/String;")`, §15),
- in kept/pinned APIs (library, `-keep`),
- in stale (un-rewritten) Kotlin metadata (§16.4).

The `toString-impl` template literals survive when `toString` is reachable. R8 retains the
`@JvmInline` annotation only through the first tree-shaking round
[S: `shaking/AnnotationRemover.java:406-415`], so it is absent in output.

### 8.3 Rules
- **`kotlinc/valueclass/detect`** (S by template). Preconditions: a final class with exactly one
  instance field `v`; a static `box(U)C = new C(u)`; an instance `unbox()U = this.v`; a static
  `equals0(U,U)Z`; and a static `toString(U)` matching the `"Simple(prop="` template. Output: S
  names for `box-impl`, `unbox-impl`, `constructor-impl`, `equals-impl0`, `toString-impl`,
  `hashCode-impl` and `equals-impl`, plus the fact "C is a value class over U".
- **`kotlinc/valueclass/simple-and-prop-name`** (S). From the `toString-impl` literal:
  `Simple(prop=` gives the simple class name and the underlying property name, so field `v`'s
  name is `prop` (S).
- **`kotlinc/valueclass/hash-verify`** (S, cryptographic). Input: a surviving mangled name
  `n-h` with its JVM descriptor, and a **finite candidate set** for each parameter position:
  `{"_"} ∪ { "L"+fq+"?"?+";" : fq ∈ FQ(U) }`. Here FQ(U) = the value classes (with S or
  candidate FQNs) whose underlying type erases to that JVM type (for `J`: all value classes over
  `Long`; for a reference type R: value classes over R, *and* value classes passed boxed when
  nullable). Also try the return variants and the old-rules scheme. Compute `md5base64` for every
  combination (the product is small in practice). Output:
  - exactly one match → S for which parameters are value classes, their FQNs, and nullability.
    The false-positive probability is ≈ (#combinations)·2⁻⁴⁰; the rule must record the
    combination count and refuse S above a threshold (say 2²⁰ combinations).
  - zero matches → the candidate FQN set was incomplete, so D (report it).
  - More than one match is practically impossible. It would be N, listing the matches.
- **`kotlinc/valueclass/fqn-from-hash`** (S). A special case of hash-verify used to recover a
  value class's **package** when only the simple name is S (from `toString-impl`). The candidates
  are `{p + "." + Simple : p ∈ P}`, where P is the set of S-known packages in the app plus the
  default package. A match is S. No match gives D. This is not exhaustive over all strings, so
  "no match" never yields N.
- **`kotlinc/valueclass/rename-mangled`** (S if the base name and all hashed FQNs are S, else D).
  When 8R knows the base name `n` (S from another rule) and the value-class parameter set (S from
  metadata, hash-verify or the §15 descriptor), it regenerates `n-md5base64(sig)` exactly. This is
  a deterministic function of S inputs, so the regenerated name is S.
- **`kotlinc/valueclass/param-kind`** (N when no hash is available). Whether a `long` parameter
  was `Long` or `UserId` is invisible after erasure. The exhaustive candidate set for a method
  with JVM params `t₁..tₙ` is `∏ᵢ ({plain} ∪ {V : V value class, erase(V)=tᵢ} ∪ {V? boxed : V
  value class, Lcls(V)=tᵢ})`, in canonical order: plain first, then value classes by (S FQN, then
  structural hash). This is N. The pipeline must fall back to D·id (unmangled JVM signature kept).

### 8.4 Fixtures
`vc-basic` (detect + toString), `vc-hash-member` / `vc-hash-return` / `vc-hash-nullable` /
`vc-hash-suspend` (the table above), `vc-hash-oldrules` (compile with `-language-version 1.4`
[I]), `vc-fqn-recovery` (package known from another class), `vc-generic-underlying` (erased upper
bound in the FQN), `vc-collision-guard` (≥ 2²⁰ candidates → refuse S).

---

## 9. Coroutines: the `ContinuationImpl` state machine

### 9.1 Shape [V: `Repo.load`, `Repo$load$1`, `AKt$g$1`; S: `CO/CoroutineTransformerMethodVisitor.kt`, `L/AddContinuationLowering.kt`]
- For `suspend fun f(...)` in container `K`, a continuation class `K$f$1 extends ContinuationImpl`.
  The class is always `$1`: anonymous objects in `f` get `$2`, `$3`, …, or `$f$<val>$1` when
  assigned to a `val` [V: `BKt$h$1` is the continuation while `BKt$h$2`/`$3` are
  `Runnable`s; `AKt$f$o$1`]. For a mangled function the base name is used (`Holder$sus$1` for
  `sus-MRxVC5U`) [V].
- Fields: `result: Object`, `label: int`, `this$0: K` for member functions (plus captured
  receivers), and spill fields named `<P>$<i>` with P ∈ {`L`,`I`,`J`,`F`,`D`,`Z`,`B`,`C`,`S`}
  (refs and arrays → `L`) [V: `Z$0 D$0 J$0 L$0 L$1 I$0`; S: `fieldNameForVar` L1589,
  `Type.normalize` L1599].
- `invokeSuspend(Object r)`: `this.result = r; this.label |= Integer.MIN_VALUE; return outer.f(<zeros/nulls>, this);`
  [V: survives R8 verbatim in shape: `r8coro-full/`, class `l`].
- `f` entry: `if (c instanceof K$f$1 && (c.label & MIN_VALUE) != 0) c.label -= MIN_VALUE else c = new K$f$1(this, c)`;
  `res = c.result; S = getCOROUTINE_SUSPENDED(); switch (c.label) { … default: throw new IllegalStateException("call to 'resume' before 'invoke' with coroutine") }`.
- At each suspension point: spill stores `c.P$i = v` in **ascending local-slot order within each
  prefix**, `c.label = k`, call, `if (ret == S) return S`. On resume: unspill and
  `ResultKt.throwOnFailure(res)`.
- Index rule [S: `calculateVariablesToSpill` L1079-1144]: at each suspension point, the live (or
  debugger-visible) variables of normalized type P, in slot order, get indices 0, 1, 2, … So the
  set of P-fields stored at a point is always a prefix `{P$0..P$(k-1)}`. Higher-index fields that
  were live at the previous point are nulled out.
- Version drift:
  - Kotlin 2.x emits `SpillingKt.nullOutSpilledVariable(v)` for visible-dead refs, gated by
    `LanguageFeature.JvmNullOutSpilledCoroutineLocals` [S: `JvmBackendConfig.kt:105`]. In
    release it returns `null` [S: `libraries/stdlib/jvm/src/kotlin/coroutines/jvm/internal/Spilling.kt`],
    so after R8 inlining it's `iput null`.
  - Older compilers skipped dead variables entirely [I].
  - `DebugMetadata.v=2` with `nl`/`i` fields is new, while v=1 had only `f l n s m c` [I].
- `@DebugMetadata(f="Samples.kt", l=[27,29], nl=[28,30], i=[0,1,1,1], s=["L$0","L$0","L$1","I$0"], n=["key","key","a","n"], m="load", c="com.example.Repo", v=2)`
  is emitted on the continuation class [V]. Here `c` is the original FQN of the container, `m`
  the original function name, `f` the source file, `l` the line per suspension point, and `n`
  the original local variable name for each spilled field `s`.

### 9.2 Survival [V]
- The class, state machine shape, `MIN_VALUE` arithmetic and the `"call to 'resume' before 'invoke' with coroutine"`
  literal all survive. Field names are minified. R8 narrows `L$0: Object` to `String` when only
  strings are stored (map: `residualsignature Ljava/lang/String;`).
- `@DebugMetadata`: in **full mode** it's stripped (both with and without stack-trace
  reachability). In **compat mode** (`--pg-compat` / `android.enableR8.fullMode=false`), when
  `BaseContinuationImpl.getStackTraceElement` is reachable, it survives with the type renamed
  (`Lj;`). R8 prunes annotation elements nobody reads, so `n`, `s`, `i` and `nl` were gone, but
  `c`, `f`, `l`, `m` and `v` stayed with **original values**
  (`c="p.Loader" f="Coro.kt" l={4 4} m="load"`). With an explicit
  `-keep class kotlin.coroutines.jvm.internal.DebugMetadata { *; }` and a pinned holder, `n`/`s`
  survived too (`r8meta/`) [V]. The keep logic is in `shaking/KeepInfo.java:90-116`: full mode
  needs the holder's keep info to keep annotations, and compat keeps any live annotation when the
  attribute is kept [S].

### 9.3 Rules
- **`kotlinc/coroutine/detect`** (S by template). Precondition: a class extending the anchored
  `ContinuationImpl` (or `RestrictedContinuationImpl`), whose `invokeSuspend` matches the
  template, plus a method `f` with the entry template and the literal. Output: `f` is a suspend
  function state machine, its continuation class is `Kf$1`, and `invokeSuspend` is S (an
  override of a stdlib abstract; it's also S through the anchor).
- **`kotlinc/coroutine/fixed-fields`** (S by template). `result` is the Object field assigned
  from the `invokeSuspend` argument. `label` is the int field or-ed with `MIN_VALUE`. `this$0` is
  the field holding `f`'s receiver.
- **`kotlinc/coroutine/spill-names`** (S under the order precondition, else N). For each prefix P
  (from the field's JVM type; any reference type → `L`), consider every suspension point s, and
  the ordered list of P-fields *stored* there before `label = k` (null stores included).
  - If every such list is `[x₀, x₁, …]` consistent with a single assignment field→index (each
    field at the same position everywhere), then field xᵢ = `P$i`. That's S by template, relying
    on the index rule and on R8 not reordering `iput`s to the same object (true in practice [I]).
  - If positions conflict, or stores are reordered: the set-nesting property still constrains
    the result. `uses(P$0) ⊇ uses(P$1) ⊇ …`. Fields with strictly decreasing use-sets are
    ordered. Each group with *identical* use-sets is a tie. The N enumeration is every ordering
    within each tie group, ∏ |gⱼ|! candidates, emitted in canonical order (lexicographic over
    the structural hash of each field's first unspill use). The pipeline falls back to D names
    `P$?_{hash}`.
- **`kotlinc/coroutine/debugmetadata`** (S, proof-string). Precondition: a surviving annotation
  whose type is the anchored `DebugMetadata`. `ContinuationImpl` subclasses are the only
  annotated classes, and element names are fixed (`c m f l s n i nl v`). Output:
  - the container class FQN `c` (S for class name and package of `K`; `K` is identified as the
    owner of the method the continuation's `invokeSuspend` calls),
  - the function name `m` (S for `f`; for a suspend lambda, `m="invokeSuspend"` and
    `c` = the lambda class FQN, e.g. `"com.example.SamplesFacade$suspendLambda$1"`, which is S
    for the lambda class name),
  - `f` (S for `K`'s SourceFile attribute),
  - `l` (S line numbers of suspension calls: the only lines provably original in an R8 build
    that stripped lines),
  - and, when present, `s`/`n` (S local variable names for the unspilled registers at each
    resume label).
  This is the single richest name source in coroutine-heavy apps built in compat mode.
- **`kotlinc/coroutine/continuation-class-name`** (S if `K` and `f` are S). `K + "$" + base(f) + "$1"`.

### 9.4 Fixtures
`coro-basic` (two suspension points, L/I spills), `coro-all-prims` (Z D J F I L), `coro-tie`
(two refs always live together → tie group), `coro-dead-visible` (nullOut spill), `coro-full-vs-compat`
(DebugMetadata presence), `coro-mangled` (`sus-…` → `Holder$sus$1`), `coro-anon-before-suspend`
(`$1` is still the continuation).

---

## 10. Suspend lambdas

### 10.1 Shape [V: `SamplesFacade$suspendLambda$1`; S: `L/SuspendLambdaLowering.kt`]
`final class K$fn$N extends SuspendLambda implements FunctionM`, with a ctor `(captures…, Continuation)`
calling `SuspendLambda.<init>(arity, completion)`. It has `create(Object…, Continuation)`,
`invoke(…)` → `create(…).invokeSuspend(Unit)`, a bridge `invoke(Object…)`, and `invokeSuspend`
containing the state machine *inline* (not in a separate method). Lambda parameters are stored in
spill fields (`I$0` for `x: Int`) in `create`. Metadata `k=3` with `KmLambda` `<anonymous>(x)`.

### 10.2 Rules
- **`kotlinc/suspendlambda/detect`** (S by template): `create`, `invoke`, `invokeSuspend` (S
  names), `label`, and spill fields as in §9.3. Parameter spill fields assigned in `create` in
  parameter order get P-indices by the same per-prefix rank.
- **`kotlinc/suspendlambda/class-name`**: `K$fn$N`, where N is the index among local classes of
  `fn`. The enumeration is N ∈ {1..m} (m = number of classes whose EnclosingMethod is `fn`, and
  the `EnclosingMethod` system annotation survives in DEX [V: `r8meta` dump]). N-class. The
  pipeline uses D. `DebugMetadata.c` (when present) proves it (S).

### 10.3 Fixtures
`suspend-lambda-capture`, `suspend-lambda-two-in-one-fn` (index ambiguity), `suspend-lambda-debugmetadata`.

---

## 11. Companion objects, `@JvmStatic`, `@JvmField`, `const`, `object`

### 11.1 Shape [V: `Repo`, `Repo$Companion`, `Singleton`; S: `L/MoveCompanionObjectFieldsLowering.kt`, `L/JvmStaticAnnotationLowering.kt`, `L/ObjectClassLowering.kt`]
- `Outer.Companion`: a `public static final Outer$Companion Companion` field, a private ctor, and
  a synthetic `(DefaultConstructorMarker)` ctor. With a named companion (`companion object Foo`),
  both the field and the class are `Foo`.
- Companion properties' backing fields (`TAG`, `shared`) move to `Outer` as static fields. A
  `const val` is also inlined at use sites.
- `@JvmStatic fun create()` in a companion gives a `static Outer.create()` that calls
  `Companion.create()` (a bridge).
- `@JvmField`: a public field without accessors.
- `object X`: `public static final X INSTANCE`, a private ctor, and a `<clinit>` doing `INSTANCE = new X()`.
  Non-capturing lambda and reference classes also use `INSTANCE` [V: `SamplesFacade$fnRef$1`].
- R8 commonly staticizes or inlines companions and singletons [I]. The remaining shape varies.

### 11.2 Rules
- **`kotlinc/object/instance-field`** (S by template, conditional on Kotlin origin). This names
  the static self-typed field `INSTANCE`, initialised once in `<clinit>`. Javac singletons have
  the same shape with arbitrary names. The precondition is a Kotlin-origin proof for the class
  (metadata, a kotlinc template in the same class, or a `DebugMetadata`/intrinsic use). Otherwise
  it's a D hint `INSTANCE`.
- **`kotlinc/companion/field`** (D hint `Companion`, S if metadata says `companion=Companion`).
  The name is user-choosable, so without metadata it's only a hint. The *class* name is
  `Outer$<fieldName>`, an equality constraint.
- **`kotlinc/jvmstatic/bridge`** (S relation). The static bridge and the companion method have
  **equal names** (a constraint, not a name source).
- `@JvmField` and `const`: D·id (nothing distinguishes them after R8).

### 11.3 Fixtures
`companion-default`, `companion-named`, `jvmstatic-bridge`, `object-singleton`, `javac-singleton`
(must stay D).

---

## 12. Lambdas and inline functions

### 12.1 Lambdas [V; S: `L/IndyLambdaMetafactoryLowering.kt`, `L/JvmInventNamesForLocalFunctions.kt`, `compiler/ir/backend.common/.../InventNamesForLocalFunctions.kt`]
- Kotlin 2.x default (`-Xlambdas=indy`): the body goes into `private static <fn>$lambda$<i>` in
  the container, plus `invokedynamic LambdaMetafactory`. Nested lambdas give `lambdas$lambda$0$0`,
  and property-delegate lambdas give `lazyName_delegate$lambda$0`. In multifile parts a
  `$<PartClass>` suffix is added (`twoConts$lambda$0$Utils__MiscKt`) [V].
- D8/R8 desugar indy into `-$$ExternalSyntheticLambdaN` classes, and R8 renames everything. No
  string survives.
- `-Xlambdas=class` (and pre-2.0 defaults) give classes `K$fn$1` extending `kotlin.jvm.internal.Lambda`,
  with `INSTANCE` when non-capturing [I].

Rules:
- **`kotlinc/lambda/body-name`** (D). The name is `<enclosingFn>$lambda$<i>`. The `enclosingFn`
  part is S only if the enclosing function is S. The index i is the order of lambdas in the
  source, which after R8 is only weakly reflected by the order of `invoke-custom`/synthetic-class
  instantiation in the enclosing body. N enumeration: all bijections between the m lambda
  bodies attributed to `fn` and {0..m-1}. The pipeline uses D `fn$lambda$_{hash}`.

### 12.2 Inline functions: `$i$f$` / `$i$a$` markers [V; S: `L/FakeLocalVariablesForBytecodeInlinerLowering.kt:35,43`, `compiler/backend/src/org/jetbrains/kotlin/codegen/inline/`]
- Inlining a call to `inline fun measure` produces a fake local `$i$f$measure: int = 0` whose LVT
  range covers the inlined body. An inlined lambda argument produces
  `$i$a$-<inlineFn>-<ContainerSimpleName>$<fn>$<k>[$<k2>]`
  (e.g. `$i$a$-map-SamplesFacade$useInline$1$1`). The class-file LVT entry holds the **original
  container class simple name and function name**. An SMAP (`SourceDebugExtension`) records the
  inlined function's file and class.
- Survival: they exist only in the LocalVariableTable (in DEX: debug-info local names). R8 strips
  local variable info unless `-keepattributes LocalVariableTable` is set, and dead-code-eliminates
  the `const 0 / store`. `SourceDebugExtension` is dropped unless kept
  (`AnnotationRemover.java:117-123`) [S]. D8 debug builds keep everything [I].

Rules:
- **`kotlinc/inline/markers`** (S when present). `$i$f$N` proves an inlined call to a function
  named N covering that pc-range (a report fact, and a D-hint for R8 un-inlining §4.6).
  `$i$a$-N-C$fn$k` proves the original simple name `C` of the class containing the lambda's
  declaration site and the name `fn`. For the *current* method (the lambda is declared in it):
  this S-names the method (`fn`) and the class simple name (`C`).
- Fixtures: `inline-markers-debug` (D8 debug), `inline-markers-r8-keeplvt`, `inline-markers-r8`
  (expect none).

---

## 13. Enums, sealed classes, string templates, `@JvmName` and file facades

### 13.1 Enums [V: `Color`, `Op`; S: `L/EnumClassLowering.kt:40-65`]
- Fields `RED`, `GREEN`, `BLUE` (names equal the entry names), `private static final $VALUES`,
  and `private static final EnumEntries $ENTRIES` (LV ≥ 1.9 [I]). Methods are `values()`,
  `valueOf(String)`, `getEntries()` and `private static $values()`. The `<clinit>` does
  `new E("RED", 0, …extra)`. Entries with bodies are subclasses named **`E$RED`** (not javac's
  `E$1`) [V: `Op$PLUS`].
- Rules:
  - **`kotlinc/enum/entry-names`** (S; already in DESIGN §1). The field stored after
    `new E("X", ordinal, …)` is named `X`.
  - **`kotlinc/enum/synthetic-names`** (S by template): `$VALUES` (the array field filled from
    `$values()`), `$ENTRIES` (from `EnumEntriesKt.enumEntries($VALUES)`), `$values`, `values`,
    `valueOf` and `getEntries`.
  - **`kotlinc/enum/entry-class-name`** (S if E is S and the enum is Kotlin-origin). The subclass
    instantiated for entry "X" is `E$X`.
- Fixtures: `enum-plain`, `enum-with-bodies`, `enum-entries-lv18-vs-19`, `enum-unboxed-by-r8`
  (coordinate with the r8 source).

### 13.2 Sealed classes [V: `Expr`, `Shape`]
- A private ctor plus a synthetic `(…, DefaultConstructorMarker)` ctor for subclasses.
  `PermittedSubclasses` (jvmTarget ≥ 17 [I]) doesn't exist in DEX. Metadata has `sealedSubclasses`.
- **`kotlinc/sealed/detect`** (D hint without metadata, S with metadata).

### 13.3 String templates [V]
StringBuilder chains, or `makeConcatWithConstants` desugared by D8/R8 into StringBuilder. R8
constant-folds known values (`"User b is 3 years"` in `r8out/`). D·id. The literal fragments are
at most log-message hints (DESIGN §1 "Log tags, exception messages").

### 13.4 `@JvmName`, file facades, multifile classes [V: `SamplesFacade`, `Utils`, `Utils__MiscKt`; S: `L/FileClassLowering.kt`, `L/GenerateMultifileFacades.kt`]
- Default facade name: `<Capitalized file basename>Kt`. `@file:JvmName("X")` replaces it.
  `@JvmMultifileClass` gives a facade `X` delegating to the part `X__<File>Kt`.
- `@JvmName` on a function changes only the JVM name. Kotlin-level strings keep the original
  (e.g. `FunctionReferenceImpl(…, "original", "renamedFn(I)I", …)`) [V].
- Rules:
  - **`kotlinc/facade/name`** (D hint). From a SourceFile/`DebugMetadata.f` value `Foo.kt` the
    hint is `FooKt`. `@file:JvmName` can't be excluded, so it's never S from the file name alone.
    It's S when a callable-reference owner or a `DebugMetadata.c` names the class.
  - **`kotlinc/jvmname/detect`** (S fact). A callable-reference string pair whose Kotlin name ≠
    JVM name proves `@JvmName`, and both names are S.

---

## 14. Delegated properties (`$$delegatedProperties`, `$delegate`)

### 14.1 Shape [V: `Repo`; S: `L/PropertyReferenceLowering.kt:54,271`]
- `static final KProperty[] $$delegatedProperties`, filled in `<clinit>` with
  `new MutablePropertyReference1Impl(Owner.class, "observed", "getObserved()I", 0)`. The flags
  low bit is isTopLevel [S: `libraries/stdlib/jvm/runtime/kotlin/jvm/internal/FunctionReference.java:28-29`,
  `AdaptedFunctionReference.java:43-75`]. Entries are generated only if used: `by lazy` and other
  "optimizable" delegates get **no** entry [V: `lazyName` absent] [S: L271].
- The backing field is `<prop>$delegate` [V: `lazyName$delegate`, `observed$delegate`].
  Accessors call `delegate.getValue(this, $$delegatedProperties[i])`.
- Local delegated properties use `<v#i>`-style signatures [I].

### 14.2 Survival [V]
`"observed"` and `"getObserved()I"` survive R8 (all configs tested). The array field and the
`$delegate` field names are minified.

### 14.3 Rules
- **`kotlinc/delegated/property-name`** (S, proof-string). The string `name` (2nd arg) is the
  Kotlin property name.
- **`kotlinc/delegated/getter-signature`** (S). The signature string `getX(…)R` is the
  **original JVM name and descriptor** of the getter, including original FQNs of class types in
  `R` and parameters (extension receivers). Link it to the residual method by: the accessor that
  loads `$$delegatedProperties[i]` for that i, which is the getter (or setter) of that property.
  Output: S getter name, S setter name `set…` only if the setter's own reference appears
  (otherwise D hint), and S **class names** for every `L…;` in the descriptor, matched
  positionally against the residual descriptor. This is a powerful type-name source.
- **`kotlinc/delegated/fields`** (S by template). `$$delegatedProperties` is the static `KProperty[]`
  filled in `<clinit>` with property-reference ctors. The delegate field read in the getter is
  `<N>$delegate`, where N is S from the string.

### 14.4 Fixtures
`delegated-observable`, `delegated-lazy` (no entry), `delegated-toplevel` (isTopLevel flag),
`delegated-extension` (receiver in descriptor), `delegated-local`, `delegated-type-in-descriptor`
(recover the FQN of the return class).

---

## 15. Callable references (`::f`, `obj::f`, `C::prop`)

### 15.1 Shape [V: `MainKt$main$1`, `SamplesFacade$fnRef$1`, `Utils__MiscKt$refs$1`]
In 2.4.20, references still compile to **classes**, even with indy lambdas:
`FunctionReferenceImpl(arity, [receiver,] Owner.class, "kotlinName", "jvmName(desc)ret", flags)`
or `PropertyReference{0,1,2}Impl(Owner.class, "name", "getName()T", flags)`, with
`invoke`/`get` calling the target directly. Suspend references add `SuspendFunction`.

### 15.2 Survival [V]
All strings survive R8 unchanged (`"load"`, `"load(Ljava/lang/String;Lkotlin/coroutines/Continuation;)Ljava/lang/Object;"`,
`"original"` + `"renamedFn(I)I"`, `"length"` + `"length()I"`). R8's Kotlin support doesn't
touch them.

### 15.3 Rules
- **`kotlinc/callableref/names`** (S, proof-string). Precondition: the class's ctor passes
  literals to the anchored `FunctionReferenceImpl`/`PropertyReference*Impl`/`AdaptedFunctionReference`
  ctor, and `invoke`/`get` contains exactly one call/field access to target `T` (a program
  member). Output: `T`'s JVM name = the name part of the signature string (S). The Kotlin name
  (S) is recorded, and if the two differ, `@JvmName` (§13.4). `T`'s owner is the `Owner.class`
  constant, and a multifile part owner proves the part class name pattern.
- **`kotlinc/callableref/descriptor-types`** (S). Positional match of the original descriptor
  against `T`'s residual descriptor gives S FQNs for renamed class types. Descriptor strings for
  value-class functions include the mangled name (feeds §8.3 `hash-verify`).
- Ambiguity: none, as long as `invoke` has a single target. With R8 inlining, `invoke` may
  contain the *inlined body* instead of a call. Then the signature can still be matched against
  surviving methods by (arity, descriptor shape modulo renaming, owner). If more than one
  candidate method in `Owner` has a matching shape, it's N (enumerate the candidates in canonical
  order), and the pipeline uses D.
- Fixtures: `ref-toplevel`, `ref-bound-member`, `ref-suspend`, `ref-jvmname`, `ref-property`,
  `ref-valueclass` (mangled signature), `ref-inlined-target`.

---

## 16. Kotlin metadata (`@kotlin.Metadata`)

### 16.1 Contents [V decode; S: `libraries/stdlib/jvm/runtime/kotlin/Metadata.kt`, `core/metadata`, `core/metadata.jvm`]
- `k` = kind: 1 class, 2 file facade, 3 synthetic class (lambda, `WhenMappings`, continuation,
  reference), 4 multifile facade, 5 multifile part.
- `mv` = metadata version (`[2,4,0]`).
- `xi` = flags: bit 0 multifile-parts-inherit, 1 pre-release, 2 script, 3 strict version, 4 JVM
  IR, 5 stable ABI, 6 FIR (pre-2.0), 7 inline-scope, 8-10 synthetic-class visibility. Observed
  values: 48, 560, 1328.
- `d1` = protobuf (`metadata.proto` + `jvm_metadata.proto`) with string indices into `d2`.
  Contents: class FQN, kind (CLASS, OBJECT, COMPANION_OBJECT, ENUM_CLASS, …), modifier flags
  (data, value, sealed, inner, fun, expect, visibility, modality), supertypes with full generic
  Kotlin types and nullability, type parameters with names, constructors, functions (name, flags
  incl. suspend/inline/operator/infix/tailrec, value parameters **with names**, default-value
  flags, return type, receiver, contracts, JVM signature), properties (name, flags incl.
  lateinit/const/var/delegated, getter/setter/field JVM signatures, `syntheticMethodForDelegate`),
  nested class names, companion name, enum entry names, sealed subclasses, the value-class
  underlying property name, module name, and local delegated properties.
- `pn` = package name for file facades with `@JvmPackageName`. `xs` = the multifile facade name
  for parts.

### 16.2 What R8 does [S: `kotlin/`; V: `r8cf/` decode]
- It is kept only if `kotlin.Metadata` itself is kept (`-keep class kotlin.Metadata { *; }`,
  shipped by **kotlin-reflect**'s consumer rules [V], or `-keepkotlinmetadata`), **and** the
  class is pinned by some keep rule (allowobfuscation counts) [S: `KotlinMetadataRewriter.java:118-129`].
  Otherwise the annotation is removed.
- Rewrites: class references (`Lcom/example/g;`), function names **when the method was renamed**
  (`component1` → `a`, `copy` → `a`), JVM signatures, companion/nested names (`Companion` → `a`),
  and enum entry names (they follow the renamed fields: `[b, c, d]`) [V].
- **Not rewritten**: `KmProperty.name` (`name`, `age`, `tags`, `db`, `lazyName`, `observed`,
  `counter`, `rgb`, `r`), `KmValueParameter.name` (`who`, `times`, `loud`, `key`, `k`,
  `property`, `oldValue`, `newValue`, lambda `x`), type parameter names (`T`), the value-class
  underlying property name (`raw`, `s`), `moduleName` (`main`), and `xi` [V]
  [S: `ConcreteKotlinPropertyInfo.java:133`, `KotlinValueParameterInfo.java:66`].
- `.kotlin_module` files are dropped and re-synthesised from surviving facades with residual
  names [S: `naming/KotlinModuleSynthesizer.java`, `dex/ApplicationWriter.java:627`].

### 16.3 Rules
- **`kotlinc/metadata/read`** (S for untouched fields). Preconditions: the annotation type is the
  anchored `kotlin.Metadata` (the element names `k mv d1 d2 xi` are fixed), `d1` parses, and every
  JVM signature in it resolves to a residual member. Output:
  - property names: S. Field and getter names are *not* taken from here, because R8 rewrote the
    JVM signatures; they come from the linkage. For a property whose getter signature maps to a
    residual method: the property name is S, and the getter's JVM name is a D hint unless the
    getter was not renamed.
  - value parameter names: S (DEX debug-info parameter names).
  - modifier flags (data, value, sealed, suspend, inline, lateinit, const, companion, object,
    enum): S facts that satisfy the "Kotlin-origin" preconditions elsewhere and upgrade the §5, §8,
    §11 and §13 detections from template-S to proof-S.
  - `inlineClassUnderlyingPropertyName`: S.
  - Function names equal to their residual JVM names were not renamed (R8 only rewrites renamed
    ones), so they are consistent but add nothing.
- **`kotlinc/metadata/stale`** (S, strong). If the metadata's JVM signatures reference names that
  **don't exist** in the residual class, but match residual members one-to-one by descriptor
  shape (modulo the class renaming map already inferred), then the metadata was *not rewritten*.
  That happens with ProGuard ≤ 6, early R8, or some obfuscators [I]. Its function names,
  class names and nested names are then **original** (S), subject to a successful bijective
  match. A non-bijective match is N: enumerate the matchings in canonical order, and the pipeline
  uses D.
- **`kotlinc/metadata/module-name`** (D hint). The Gradle module name (e.g. `main`, `app_release`)
  is useful for report grouping only.

### 16.4 Ambiguity
Overloads with identical residual descriptors after R8 class merging can make signature linkage
many-to-one. Resolve by exact descriptor match. Otherwise it's N over the permutations of
same-descriptor members, and the pipeline uses D.

### 16.5 Fixtures
`metadata-reflect-pinned` (kotlin-reflect + `-keep,allowobfuscation`), `metadata-not-pinned`
(stripped), `metadata-stale` (hand-crafted: metadata copied from the unshrunk jar into the
shrunk one), `metadata-enum-rewrite`, `metadata-lambda-params`.

---

## 17. Rule summary

| Rule id | Class | Evidence | Survives default R8 full mode (no metadata keep)? |
|---|---|---|---|
| `kotlinc/anchor-stdlib` | S | stdlib body + message strings | yes (when referenced) |
| `kotlinc/nullcheck/param-name` | S | proof-string | **R8 < 9.0.26 only**, or `keep` |
| `kotlinc/nullcheck/expr-callee` | S / hint | proof-string `name(...)` | same as above |
| `kotlinc/nullcheck/getclass-residue` | D·id | — | yes |
| `kotlinc/lateinit/property-name` | S | proof-string (call or folded literal) | **yes** |
| `kotlinc/lateinit/field-name` | S (precond.) / D | template | yes |
| `kotlinc/lateinit/accessor-names` | D → S if corroborated | hint | yes |
| `kotlinc/data/detect`, `property-names`, `simple-name` | S | template ensemble + literals | when `toString` is live |
| `kotlinc/data/component-names`, `copy-names` | S | template | when live |
| `kotlinc/data/getter-names` | D | hint | — |
| `kotlinc/default/stub`, `super-message`, `ctor-marker` | S | template / proof-string | partly (often inlined) |
| `kotlinc/jvmoverloads/detect` | S (constraint) | template | partly |
| `kotlinc/when/mappings-class`, `mapping-field-index` | S / N→D | template | rarely (R8 removes) |
| `kotlinc/when/string` | D | — | yes |
| `kotlinc/valueclass/detect`, `simple-and-prop-name` | S | template + literal | when live |
| `kotlinc/valueclass/hash-verify`, `fqn-from-hash` | S (crypto) | md5 fingerprint | only where a mangled name survives |
| `kotlinc/valueclass/rename-mangled` | S / D | deterministic recomputation | n/a (output side) |
| `kotlinc/valueclass/param-kind` | **N** → D·id | — | — |
| `kotlinc/coroutine/detect`, `fixed-fields` | S | template + literal | **yes** |
| `kotlinc/coroutine/spill-names` | S (ordered) / **N** → D | template | yes |
| `kotlinc/coroutine/debugmetadata` | S | proof-string | **compat mode only** |
| `kotlinc/coroutine/continuation-class-name` | S if K, f are S | template | yes |
| `kotlinc/suspendlambda/detect` | S | template | yes |
| `kotlinc/suspendlambda/class-name` | **N** → D | — | — |
| `kotlinc/object/instance-field` | S (Kotlin-origin) / D | template | if not staticized |
| `kotlinc/companion/field` | D / S (metadata) | hint | — |
| `kotlinc/jvmstatic/bridge` | S (constraint) | template | partly |
| `kotlinc/lambda/body-name` | **N** → D | — | — |
| `kotlinc/inline/markers` | S | LVT names | **no** (needs LVT) |
| `kotlinc/enum/entry-names`, `synthetic-names`, `entry-class-name` | S | literal + template | yes (unless enum unboxed) |
| `kotlinc/sealed/detect` | D / S | metadata | — |
| `kotlinc/facade/name` | D | hint | — |
| `kotlinc/jvmname/detect` | S | callable-ref pair | yes |
| `kotlinc/delegated/property-name`, `getter-signature`, `fields` | S | proof-string | **yes** |
| `kotlinc/callableref/names`, `descriptor-types` | S | proof-string | **yes** |
| `kotlinc/metadata/read` | S (props, params, flags) | metadata | only pinned + `kotlin.Metadata` kept |
| `kotlinc/metadata/stale` | S / **N** → D | un-rewritten metadata | legacy shrinkers |
| `kotlinc/metadata/module-name` | D | hint | as above |

N rows follow DESIGN §0.1: N is never shipped in the pipeline. The rule computes the exhaustive
candidate set for the **report** (and for tests), and the pipeline applies the listed D
fallback. The task framing allows "N with exhaustive enumeration", but DESIGN.md §0.4 has no `N`
variant, so the registry should model these as `Class::Deterministic` with a
`candidates: Vec<…>` report payload.

---

## 18. Open questions for the design

- **Q1.** DESIGN.md §1 "Kotlin intrinsics parameter names: S (no mapping)" needs a coverage note:
  it's dead by default for R8 ≥ 9.0.26 (AGP 9-era). Should 8R read the `~~R8{"version":…}`
  marker to predict absence and suppress "missing evidence" noise?
- **Q2.** Template-S for hand-writable templates (data-class `toString`). Is the ensemble
  requirement (§5.3) acceptable, or should single-template matches be D?
- **Q3.** The value-class hash gives S with a stated false-positive bound (≈ n·2⁻⁴⁰). DESIGN §0.1
  says "the evidence determines the preimage uniquely". Do we accept a cryptographic bound as
  "unique", with a combination cap?
- **Q4.** Assumption: R8 doesn't reorder `iput`s at a suspension point, or `putstatic`s in
  `<clinit>` (§7.1, §9.3). It needs an R8-side fixture and a check of `ir/optimize` code-motion
  passes before these become S.
- **Q5.** Should 8R *re-synthesise* `@kotlin.Metadata` (D) from recovered facts, so
  Kotlin-aware decompilers (jadx's Kotlin mode) show properties, data classes and suspend
  functions?
- **Q6.** DebugMetadata's `c` gives an FQN for classes that R8 *repackaged*. This is the rare
  source of S **package** names without a mapping, so it should feed the §5 naming module early.
