# Source: R8 optimizations and D8/R8 desugaring, as seen without a mapping

Status: research notes. Input for `crates/eightr-rules` under the source ids `r8` and `desugar`.
Contract: DESIGN.md §0 (S / D / N, α-invariance) and §1 (rulebook). The input is **no mapping**
throughout. As the research brief specifies, an **N** entry is allowed only when it enumerates
*every* candidate preimage, exhaustively and in canonical order, so the original is always in the
set. (DESIGN §0.1 still says "N is forbidden in the pipeline", so these N entries become either a
report-only candidate list or a D rule that picks by structure. That's a design decision, not a
research one.)

Undo order: R8 (naming, merging, inlining, outlining, …) first, then desugaring (D8/R8 desugar),
then compiler plugins, then kotlinc.

## Provenance

| Artifact | Revision |
|---|---|
| r8.googlesource.com/r8, `main` (depth 1) | `fa9ef658c12419cc989fef454d3ee7d07b93a376` (2026-09-25) — called **HEAD** below |
| r8.googlesource.com/r8, tag `8.10.9-dev` (depth 1) | `a7ad18a70460b799d0482e497c109a75bf7f91de` (2025-02-18). This is the jar in `build-tools/36.0.0/lib/d8.jar`. |
| Release tags spot-checked through gitiles `?format=TEXT` | 8.10.40, 8.11.34, 8.12.31, 8.13.22, 9.0.45, 9.1.43, 9.2.25, 9.3.27, 9.4.23 |
| R8 9.4.23 jar (`dl.google.com/android/maven2/com/android/tools/r8/9.4.23/`) | build `25f587b70bc319f276b9979f2e2b0181d7fb5168` |
| Experiment toolchain | javac 26.0.2 `--release 17`, kotlinc 2.4.20 (borrowed from `scratchpad/agents/kotlinc`), android-36 `android.jar`, dexdump 36.0.0 |

Source paths are relative to `src/main/java/com/android/tools/r8/`. Unless marked "HEAD only" or
"8.10.9 only", they are the same at both revisions. The experiments live in
`…/scratchpad/agents/r8/exp/` (`build.sh <dir> <min_api>`: javac, then R8 `--release`, then
dexdump). Subdirectories: `naming/`, `syn/`, `opt/`, `itf/`, `bp/`, `dontobf/`, `native/`, `kt/`,
`kt2/`. They're throwaway.

Legend: **[V-src]** = verified by reading source. **[V-exp]** = verified by experiment
(R8 8.10.9 unless noted). **[I]** = inferred, needs a fixture before a rule relies on it.

---

## 0. Headline: the "not 1–3 ASCII letters ⇒ original" rule is unsound

`r8/kept-name` (crates/eightr-rules/src/lib.rs) treats these as original:
a class whose simple name is not 1–3 ASCII letters, a package where no segment is 1–3 lowercase
letters, and a member whose name is not 1–3 ASCII letters. There are counterexamples to all three.
Every one below was produced by stock R8 with no dictionary:

| # | Minified or R8-invented name that the rule calls "original" | Trigger | Status |
|---|---|---|---|
| C1 | Class `a.h0`, `a.a0` … `a.o0`, field `p0` … `z0` | R8's generator uses **digits** after the first character. This happens in any package with more than 26 classes (lowercase mode) or 52 (mixed mode), and in any class with more than 52 same-namespace members. | [V-exp] 8.10.9 and 9.4.23 |
| C2 | `com.ex.Outer$a`, `com.ex.Outer$a$a`, `com.ex.Outer$b` | `-keepattributes InnerClasses` or `Signature`. Both are in practically every real app, through library consumer rules. An inner class is named `<renamed outer>$<gen>`, and the outer may be kept. | [V-exp] 8.10.9 and 9.4.23 |
| C3 | `Rep1` (from `com.ex.sub.Rep`) | `-keep,allowrepackage`. The name is kept but repackaged, and a numeric suffix is added on collision. | [V-exp] 9.4.23 |
| C4 | `a.Rep`, `b.Rep`, and root-package `Rep` | `-keep,allowrepackage`. The simple name is original but the **package isn't**. | [V-exp] 8.10.9 and 9.4.23 |
| C5 | 4+ character minified names (`a00` is index 963 in lowercase) | A namespace with more than 34,658 classes in lowercase mode. R8 ≥ 9.0 repackages *everything* into the root package by default, so large apps hit this. | [V-src], [I] for real apps |
| C6 | `work$1` | `-dontobfuscate`. A fresh name is created after unused-argument removal collides. | [V-exp] |
| C7 | `Foo$$ExternalSyntheticLambda0`, `-$$Nest$fgetx`, `$r8$classId`, `Foo$-CC`, `…$EnumUnboxingLocalUtility` | `-dontobfuscate`, or any D8-built code. | [V-src], D8 [V-exp] |
| C8 | `j$.time.LocalDate` and similar | Core-library desugaring (L8). The name isn't the app's own original. It's `java.time.LocalDate` rewritten. | [V-src] `Minifier.L8MinificationClassNamingStrategy` |
| C9 | Classes `A`–`Z`, `Ab`, … | R8 **≤ 8.12** without `-dontusemixedcaseclassnames` uses mixed-case class names. The current class check is case-insensitive, so it handles this. It's listed so the version table (§1.2) is complete. | [V-exp] 8.10.9 |

Members of a single class rarely reach 52. But C1 for classes shows up in **every** non-trivial app,
because a package usually holds more than 26 classes. The replacement predicate is in §1.6.

---

## 1. Minifier naming

### 1.1 The generator [V-src]

`utils/SymbolGenerationUtils.java` `numberToIdentifier(n, casing)`, with `n` = 1, 2, 3, … per
namespace:

* Alphabet `IDENTIFIER_CHARACTERS = "0-9a-zA-Z"`. The first character is never a digit. In
  `DONT_USE_MIXED_CASE` mode, the first character is from `[a-z]` and later ones from `[0-9a-z]`.
  In `USE_MIXED_CASE` mode, the first character is from `[a-zA-Z]` and later ones from `[0-9a-zA-Z]`.
* The encoding is **little-endian**: the first character cycles fastest. Lowercase sequence:
  `a … z, a0, b0 … z0, a1 … z9, aa, ba … zz, a00 …` (index 27 = `a0`, 287 = `aa`,
  963 = `a00`). Mixed: `a … z, A … Z, a0 … Z0, … ZZ, a00`. Checked with a Python port.
* Capacity by length. Lowercase: 26 names of length 1, 962 of length ≤2, 34,658 of length ≤3.
  Mixed: 52 of length 1, 3,276 of length ≤2, 203,164 of length ≤3.
* `naming/Minifier.java` `BaseMinificationNamingStrategy.nextString`. The dictionary comes first
  (`-obfuscationdictionary` / `-classobfuscationdictionary` / `-packageobfuscationdictionary`),
  then the generator, skipping dictionary words. It always skips `RESERVED_NAMES`. Of that set,
  only the ≤3-character words matter: `it`, `by`, `do`, and `if` (**`if` from 8.11 on**; at
  8.10.9 the list ends at `"do"`). R8 never generates these 3–4 names, so if one appears it is
  original (a small S bonus).
* Classes (`MinificationClassNamingStrategy.next`): skip any candidate whose descriptor ends in
  `LR;` or `/R;`. The name `R` is never generated. The candidate must also be unused. At HEAD,
  "used" is case-insensitive (`usedTypeNames` stores `toLowerCase`), so no two classes differ only
  by case.
* Namespaces (`naming/ClassNameMinifier.java` `Namespace`). There is one counter per output
  package, and one per outer class when inner-class structure is kept (§1.3). Members get one
  counter per naming state (`MethodNamingState`, `FieldNamingState`,
  `InterfaceMethodNameMinifier`), and names are reused across unrelated classes and across
  different protos (plain overloading).

### 1.2 Version table (what the `~~R8{"version":…}` marker selects)

| Property | ≤ 8.10 | 8.11–8.12 | 8.13–8.x | ≥ 9.0 (through 9.4.23 and HEAD) |
|---|---|---|---|---|
| Class-name casing | mixed unless `-dontusemixedcaseclassnames` | same | **always lowercase** | always lowercase |
| Member-name casing | mixed | mixed | mixed | mixed |
| Package-name casing | lowercase | lowercase | lowercase | lowercase |
| `if` reserved | no | yes | yes | yes |
| Default package mode (DEX, no `-repackageclasses`, no `-flattenpackagehierarchy`) | `MINIFICATION`: each movable package becomes a fresh top-level single-segment package `a`, `b`, … | same | same | **`REPACKAGE` into the root package `""`** |

Sources: at 8.10.9, `shaking/ProguardConfiguration.java:367` changes NONE to MINIFICATION when
obfuscating. At HEAD, `utils/InternalOptions.java` `getPackageObfuscationMode()` changes DEFAULT to
REPACKAGE for DEX output. The tag spot-check grepped for `return PackageObfuscationMode.REPACKAGE;`
and `MixedCasing.DONT_USE_MIXED_CASE);`. Both behaviours were confirmed by experiment:
8.10.9 put classes in `a/…` and `b/…`, and 9.4.23 put them in the root package. [V-src] [V-exp]

AGP's `proguard-android-optimize.txt` contains `-dontusemixedcaseclassnames` [I, from memory].
So in practice class names are lowercase in every version, but a rule must not assume that for ≤ 8.12.

The marker also carries `"r8-mode":"full"|"compatibility"`, `min-api`, and `pg-map-id` (the first
7 hex characters of the map hash in 8.x; the full SHA-256 in 9.4). [V-exp] Example:
`~~R8{"backend":"dex","compilation-mode":"release","has-checksums":false,"min-api":21,"pg-map-id":"760d15e","r8-mode":"full","sha-1":"a7ad18a7…","version":"8.10.9-dev"}`.

### 1.3 Packages, repackaging, inner classes

* **Package modes** (`repackaging/Repackaging.java` `DefaultRepackagingConfiguration.getNewPackageDescriptor`) [V-src]:
  * `REPACKAGE` (`-repackageclasses 'p'`, or the ≥9.0 default with `p = ""`): every movable class
    goes to exactly `p`.
  * `FLATTEN` (`-flattenpackagehierarchy 'p'`): package → `p/<gen>`.
  * `MINIFICATION` (the ≤8.x default): package → `<gen>` (top level).
  * A package **stays** when it is the root package, or when it `mayHavePinnedPackagePrivateOrProtectedItem`
    (a kept package-private or protected class or member). Individual classes stay when repackaging
    is disallowed for them. [V-exp] `itf/`: with `-keep,allowobfuscation` on package-private members,
    everything stayed in `com.ex`.
* **Repackaging keeps the simple name.** Only the package changes. The minifier runs later, and
  it renames the simple name only if minification is allowed (`getRepackagedType`). A repackaged
  inner class is renamed to `<new outer simple name><rest>`. On collision a counter is appended:
  `addSuffix(i)` with i = 1, 2, …. So **name kept + package changed** happens exactly when
  repackaging is allowed but minification is not:
  * `-keep,allowrepackage` (`ProguardConfigurationParser` "repackage"). [V-exp C3/C4]
  * `-dontobfuscate` together with `-repackageclasses p` or `-flattenpackagehierarchy p`. Here the
    new package is `p + "/" + oldPackage`, the old path under the prefix, unless `p` is empty.
    [V-src]
  * Not by `-keepnames` or `-keep`: without `allowobfuscation`, `RootSetBuilder` calls
    `disallowRepackaging` unless `allowrepackage` is given. `setAllowsObfuscation(true)` implies
    `allowsRepackaging = true` (`shaking/ProguardKeepRuleModifiers.java`). [V-src]
  * `-keeppackagenames` pins packages (`repackaging/RepackagingUtils.isPackageNameKept`).
* **Inner-class structure** (`ClassNameMinifier.computeName`, `InternalOptions.keepInnerClassStructure()`
  = keep `Signature` **or** `InnerClasses`) [V-src] [V-exp C2]. A class that has an
  `InnerClasses` entry for itself gets its name in the namespace `<renamed outer><sep>`. Here
  `sep = DescriptorUtils.computeInnerClassSeparator(outer, inner, innerName)`, which is the original
  text between the outer name and the inner simple name. It must start with `$`, and falls back to
  `$`, so `$$` and `$1` are possible. The outer can be **kept** (`com.ex.Outer$a`) or **pruned**:
  R8 then assigns the pruned outer an implicit name that never appears in the output.
  Inner classes of a kept outer class stay in the outer class's package, so they are not
  repackaged. [V-exp]
* The L8 prefix applies to desugared-library classes (`j$…`).

### 1.4 What "kept" looks like, and which members are never renamed [V-src unless noted]

Never renamed:
* `<init>` and `<clinit>` (`MinifierMemberNamingStrategy.getReservedName`: `isConstructor`).
* **Annotation-interface methods** (`getHolder().isAnnotation()`).
* Anything that isn't a program member: library and classpath members, and program methods that
  **override or implement a library method**. They are reserved through the naming state.
  Verified: lambda class `apply`, record `equals`/`hashCode`/`toString`, `C1.toString`. [V-exp]
* Members whose keep info disallows minification: `-keep`, `-keepnames`, `-keepclassmembers`,
  `-keepclassmembernames`, any rule without `,allowobfuscation`, `@Keep` through AGP rules, and
  entries from `-if` rules.
* Enum `values()` when R8 traces reflective enum use. `EnumSet.allOf`, `Enum.valueOf(Class,String)`
  and similar call `shaking/Enqueuer.markEnumValuesAsReachable`, which runs
  `disallowMinification`. [V-exp `native/`: `values` kept its name]
* `R` classes are never *produced* as a minified name.
* With desugared library, callbacks whose signature has a rewritten type.

**Renamed** unless a keep rule says otherwise (this corrects DESIGN §5 item 1):
* **`native` methods.** There is no special case in the minifier. `Jni.nativeCompute` became `a`
  [V-exp `native/`]. AGP's default file keeps them with
  `-keepclasseswithmembernames class * { native <methods>; }` [I].
* Enum `valueOf`, and `values` when not reflectively used. Kept by AGP's
  `-keepclassmembers enum * {…}` [I].
* `serialVersionUID` and other `Serializable` members, again unless the AGP or app rules keep them.
* Every R8/D8 synthetic member (`$r8$classId` → `a`, `$VALUES` → `a`) and class
  (`$$ExternalSyntheticLambda3` → `a.b`). The `SyntheticItemsOptions.restrictRenaming` default is
  `false`. [V-exp]

### 1.5 Strings that look like names but prove nothing

* `-identifiernamestring` targets and `Class.forName("…")` / `AtomicFieldUpdater` string constants
  are **rewritten to minified names** (naming/IdentifierMinifier.java).
* Desugared `record` `toString` names string: see §2.8. It holds **minified** field names.
* `-adaptclassstrings` does the same.

### 1.6 Proposed sound predicate `may_be_minified(name, namespace)` (replaces the current rule)

Preconditions (unchanged, and still unverifiable from the DEX): no dictionaries, no
`-applymapping`, no `-dontobfuscate`, no `-keep,allowrepackage`. §1.7 detects the last two
heuristically.

Let `first` and `rest` be the character classes for the version and casing (§1.2). Let
`G = first rest*`.

```
tail(s)              = substring after the last '$' of s (all of s if there is no '$')
may_be_minified_class(simple, pkg) =
      tail(simple) ∈ G   ∧ len(tail) ≤ L(ns)         # C1, C2, C5
    ∨ tail(simple) ∈ RESERVED_THIS_VERSION → false   # it/by/do/(if) are never generated
may_be_minified_pkg_segment(seg)   = seg ∈ [a-z][0-9a-z]* ∧ len ≤ L(pkgs)
may_be_minified_member(name, cls)  = name ∈ G_mixed ∧ len ≤ L(members) ∧ name ∉ {<init>,<clinit>}
```

* Taking the **tail after the last `$`** is what makes C2 sound. The prefix can be any kept outer
  name plus any separator starting with `$`, and the generated part never contains `$`.
* **Length bound.** Every counter step consumes a name that is used or reserved in that namespace.
  Reserved names come from kept, classpath, missing and dangling-pruned types, and dangling types
  are named in the root namespace. So a sound bound is
  `L(ns) = len(numberToIdentifier(|type descriptors in ns across all dex files| + k))`. `k`
  covers `R`, the reserved words, and classpath or missing types that are not referenced. Take
  `k = 64`, and note it in the report. For members, use
  `|distinct member-name strings in the app| + |distinct library method names in supertypes|`.
  In practice `L = 3` for members and 2 or 3 for classes, but computing it avoids C5.
  [V-src reasoning, I for the slack]
* A name can be "maybe minified" and still be original, for example a kept class `a`. That only
  costs S and moves the name to D, so it's safe.
* **Package S needs an extra check (C4).** Even when the simple name is non-minifiable, the package
  is S only if the class is not in a repackaging target. The targets are the root package, a
  package where every segment is in `[a-z][0-9a-z]*`, and the package that holds the most
  minified classes (a guess at the `-repackageclasses` target; if there are ties, all of them).
  Otherwise the package is D, or N (§6.4).
* **Synthetic and fresh-name exclusions** (these matter only with `-dontobfuscate` or D8 input,
  C6/C7). Never S if the name contains `$$ExternalSynthetic`, `$$InternalSynthetic`, `-$$Nest$`,
  `$r8$`, `$-CC`, `-IA`, `$EnumUnboxing`, `$Wrapper`, `$VivifiedWrapper`, or matches
  `.*\$[0-9]+$` produced by `createFreshMethodNameWithoutHolder` (`name$N`,
  `name$holder$N`; graph/DexItemFactory.java).

### 1.7 Detecting the unverifiable preconditions

* `-dontobfuscate`: no class or member in any non-root program package matches `G` except kept
  ones, or synthetic names like `$$ExternalSynthetic` are visible. Then treat every `…$N` member
  name as N (§6.2) and synthetic names as D.
* A dictionary: a large share of names outside `G` that are not English camelCase, or unusual
  Unicode (Java keywords, `Il1`, and so on). Fall back to the manifest/JNI/layout-only S described
  in DESIGN §5.
* `allowrepackage`: a class with a non-minifiable simple name inside a repackaging target, next
  to minified classes. The name stays S, the package becomes D, and a trailing-digit suffix
  becomes N (§6.3).

---

## 2. Synthetic naming and shape after minification

`synthesis/SyntheticNaming.java` defines the kinds. The pre-minification name is
`<context>$$ExternalSynthetic<Kind><id>` (or `$$InternalSynthetic`). Single-method synthetics
name their method `m` (`INTERNAL_SYNTHETIC_METHOD_NAME`). In R8 release, **all of these are
minified**, and **R8 horizontally merges synthetics across kinds**. For example, the outline method
ended up inside the `EnumUnboxingSharedUtility` class [V-exp `opt/`]. So a residual class can mix
kinds, and identification has to be **per method**.

The mapping-only markers `# {"id":"com.android.tools.r8.synthesized"}` and
`sourceFile "R8$$SyntheticClass"` don't exist in the DEX. The DEX `SourceFile` is just
`"SourceFile"`.

Class access flags from `synthesis/SyntheticClassBuilder.build()`: always
`ACC_PUBLIC|ACC_SYNTHETIC`, and final unless it's abstract or an interface.

| Kind (pre-minify name) | After R8 release | Structural fingerprint (no mapping) | Status |
|---|---|---|---|
| `$$ExternalSyntheticLambda<N>` | Class minified; the method keeps the **functional-interface name** (a library override) | `PUBLIC FINAL SYNTHETIC` (0x1011). Super `Object`, one interface, captured values in final instance fields. The body calls a (possibly inlined) impl. No `INSTANCE` field; a new instance is created per evaluation. | [V-exp] |
| `$r8$lambda$…` / `lambda$main$0` impl methods | Minified, often inlined into the lambda class or the caller | none left once inlined | [V-exp] |
| `…$$ExternalSyntheticBackport<N>` | **Usually disappears: inlined at every call site.** Seen with 12 kept callers of `Math.multiplyExact(JJ)`. | An inlined template fragment. See §3. | [V-exp `bp/`] |
| `…$$ExternalSyntheticApiModelOutline<N>` | Class minified (`PUBLIC ABSTRACT SYNTHETIC`, 0x1401). Methods are static, sometimes `BRIDGE\|SYNTHETIC` (0x1049). | The body is exactly one call to a library member above min-api, a `new-instance`+`<init>` or an `invoke-*`, then return. Call sites are behind `SDK_INT` checks. | [V-exp `syn/`] |
| `…$$ExternalSyntheticOutline<N>` | Merged into another synthetic class; the method is `PUBLIC STATIC` **without** SYNTHETIC | A static, straight-line library-call sequence (for example `new StringBuilder`, `append`×k, `toString`) whose arguments are the "holes". The threshold is **20** call sites, the size 3–99 instructions (`InternalOptions.OutlineOptions`). | [V-exp `opt/`] |
| `$-CC` (interface companion) | Class minified, `PUBLIC ABSTRACT SYNTHETIC` | Static methods whose first parameter has the interface type (the default methods), plus the interface's static and private methods. The interface itself keeps only abstract methods. | [V-exp `itf/`] |
| `-$$Nest$m<name>`, `-$$Nest$fget<name>`, `…$sm`, `$sfget`, `$fput`, `$sfput` (ir/desugar/nest/NestBasedAccessDesugaring.java) | Minified, `PUBLIC STATIC BRIDGE SYNTHETIC` (0x1049) | The first parameter is the owner type. The body is exactly one `iget`/`iput`/`invoke-direct` of a **private** member of the same class, then return. | [V-exp `itf/`]. In R8 the bridges often vanish anyway: they get inlined, or access modification makes them unnecessary. |
| `$r8$classId` (horizontalclassmerging/ClassMerger.java `CLASS_ID_FIELD_PREFIX`) | Field minified, `PUBLIC FINAL SYNTHETIC` int | The field is written first in the synthetic `<init>(…, I)`. Reads feed a `packed-switch`. See §4.1. | [V-exp] |
| `$EnumUnboxingLocalUtility` / `$EnumUnboxingSharedUtility` | Minified, `PUBLIC ABSTRACT SYNTHETIC` | A static `int[]` `$VALUES` filled with `{1..n}` in `<clinit>`. `ordinal(I)I` = `if-eqz → throw null` then `-1`. Also `name(I)`/`toString`/`valueOf` switches. See §4.3. | [V-exp] |
| `$closeResource` / `TwrCloseResource` (`$r8$twr$utility`) | Only for javac-8-style class files, or min-api < 19 | The `AutoCloseable`/`Closeable` dispatch and `addSuppressed` template. javac ≥ 9 inlines TWR and emits no helper; at min-api 21 we saw a direct `Throwable.addSuppressed`. | [V-src] [V-exp] |
| Record desugaring (`RecordTag`, `RECORD_HELPER`) | The record class is minified; the `RecordTag` super can be removed (we saw `Object`) | `toString`: `const-string "<f1>;<f2>"` → `split(";")`, `getSimpleName()`, `"["`, `"="`, `", "`, `"]"`. `hashCode`: `31*h + …`. `equals`: `instance-of`, then per-field compare (`Objects.equals`). The names string holds the **minified** field names (`"a;b"`), and removed fields are dropped (`"a"` for `Point(x, y=const)`). | [V-exp `syn/`, `itf/`] |
| Other kinds (`ThrowIAE`, `ThrowNSME`, `NonNull`, `ToStringIfNotNull`, `CheckNotZero`, `ServiceLoad`, `AutoCloseableDispatcher`, `TypeSwitchHelper*`, `$IA`, `-IA`, …) | Minified | Small fixed-template static methods. List the templates per version from `SyntheticNaming` and the generators. | [V-src] |

With `-dontobfuscate`, and in D8 output, all the names above are **visible and exact**. That is
S *evidence of the kind*, but the names themselves are not "original source names" (C7).

---

## 3. Backport templates

* Location [V-src]: `ir/desugar/backports/BackportedMethods.java`. It's 12,047 lines at 8.10.9 and
  headed `// GENERATED FILE. DO NOT EDIT! See GenerateBackportMethods.java`. It's generated from
  `src/test/java/com/android/tools/r8/ir/desugar/backports/*Methods.java`: `MathMethods`,
  `StringMethods`, `ObjectsMethods`, `CollectionMethods`, … as `CfCode` factories. The wiring is
  in `ir/desugar/BackportedMethodRewriter.java`. At 8.10.9 there are 117 `MethodGenerator`s over
  142 distinct templates, plus 33 `InvokeRewriter`s and some statifying and forwarding generators.
* **Two mechanisms.**
  1. `MethodGenerator`: a synthetic method from a template (`$$ExternalSyntheticBackport`).
  2. `InvokeRewriter` (`ObjectsMethodRewrites`, `LongMethodRewrites`, `NumericMethodRewrites`, …):
     inline instruction substitution with no synthetic. For example `Objects.requireNonNull(x)`
     becomes `x.getClass()`, and `Long.hashCode` becomes inline shifts and xor. These leave only
     an inline sequence, and a debug position that attributes it to the original API **only in the
     mapping** (`…:0:0 -> main` rows with holder `java.lang.Math`).
* **Templates are not injective.** Math and StrictMath share templates (`BackportedMethodRewriter`
  ~L817: `for (DexType mathType : mathTypes)` over `{Math, StrictMath}`, for addExact, floorDiv,
  floorMod, multiplyExact, …). So a matched template determines the API only up to a **finite
  set**. N: {`java.lang.Math.m`, `java.lang.StrictMath.m`}, in canonical order. Other loops over
  types exist (lines ~1403, ~1941, ~1968). Generate the table `template-id → [API…]` per version by
  running `BackportedMethodRewriter`'s initializer in a harness, or by a D8 build of one call per
  API. [V-src]
* **Are bodies exact per version?**
  * In **D8**: yes. Template CfCode goes to IR, then to DEX, deterministically. The D8 body of
    `multiplyExact(JJ)` is 44 instructions, starting with `Long.numberOfLeadingZeros`×4. [V-exp]
    D8 names the class `Main$$ExternalSyntheticBackport0`, with all methods `m`, one class per
    context.
  * In **R8**: **no**. The synthetic is program code, so it is optimized in context and
    **inlined**. We saw this with 5 same-method call sites and with 12 kept callers, and
    `floorMod` was even constant-folded into arithmetic. After inlining, register allocation,
    constant propagation and branch folding apply. [V-exp `syn/`, `bp/`]
* **Rule proposal.** `desugar/backport-method`: an **S** match needs a standalone static method
  whose body equals the version's template *as compiled by that version*. Precompute that by
  running the jar from the marker on a one-call probe in both D8 and R8 modes; if the version is
  unknown, don't match. Then the API is **S** if the template maps to one API, and **N** (the
  exhaustive list) if it maps to several. `desugar/backport-inlined`: matching an optimized
  fragment in a caller is **D** only. It rewrites back to the API call, and semantic equivalence
  must be proven by symbolic execution of the fragment, not assumed. Fragment matching is fuzzy,
  so it is never S. DESIGN §1's "Backports: S without mapping" row should read **"S only for a
  standalone, byte-exact synthetic; N for shared templates; D for inlined fragments"**.

---

## 4. Optimizations: fingerprints and rules

### 4.1 Horizontal class merging [V-src `horizontalclassmerging/`, V-exp]

* The mechanism. A group's target gets `classIdentifiers`: target = 0, then sources = 1, 2, … in
  group order (`ClassMerger.buildClassIdentifierMap`). **A `$r8$classId` field is created only when
  some merged virtual method is non-trivial** (`ClassMerger.Builder.initializeClassIdField`:
  `anyMatch(!isNopOrTrivial)`). Constructors become `<init>(…, I)`, synthetic, and store the id
  **before** calling `super.<init>` (`iput` then `invoke-direct Object.<init>`, which is illegal in
  Java source, so it's a strong fingerprint). Merged virtuals become `packed-switch` on the
  classId field. When those virtuals were inlined into callers, the switch sits **in the caller**
  (`opt/`: `iget a.a:I` + `packed-switch` in `main`, and class `a.a` has no virtual left).
* Policies (`policies/`): no enums, records, interfaces, kept classes, Kotlin-metadata classes,
  runtime type checks (`instanceof`/`checkcast` blocks merging; verified), native methods, or
  inner-class attributes; same package; same superclass; …. The groups found are therefore
  "classes with identical field layout under one super". Static-only classes are merged
  **without** any id (`OnlyClassesWithStaticDefinitionsAndNoClassInitializer`).
* **How many classes were there?** Not exactly the number of distinct constants.
  * The ids passed at `new` sites are a subset of `{0..g-1}`. Instances of some sources can be
    dead (the comment in `ClassMerger.appendClassIdField` explicitly expects `{0,2,3}`).
  * Merges with no classId field (trivial virtuals, or static-only classes) leave **no trace**.
  * After inlining, the id may reach the switch only as a constant, or the switch may be folded.
  * **S facts:** "≥ max(observed id)+1 classes merged here", "id k had behaviour B_k" (from the
    switch arms), and "ids in the default arm of the switch share that arm's behaviour". The last
    arm is usually the fall-through, so an id that is never tested explicitly still maps
    somewhere.
  * **D:** un-merge into one class per *observed* id, named `{Super}_{hash}` (the current §1 row).
    The total count is not enumerable (it is unbounded above), so there is no N form for it.
* Rule sketch `r8/hmerge-classid`: detect the int field written before the super `<init>` call
  from a synthetic `(…,I)V` constructor, with every read feeding a switch or an `if-eq` against
  small constants. Label: **D**, with the S lower bound in the report.

### 4.2 Vertical class merging [V-src `verticalclassmerging/`]

The subclass absorbs its single subtype, or the reverse. Methods that collide get fresh names
`name$Holder$N`, which are **minified** unless `-dontobfuscate`. No residual fingerprint beyond
"a class whose supertypes are fewer than its behaviour suggests". **D·id.** The alternatives, each
possible split point, cannot be enumerated with certainty because the removed class's name is
gone. So it can't be N.

### 4.3 Enum unboxing [V-exp `syn/`, `opt/`, `native/`]

* The enum class is removed (`R8$$REMOVED$$CLASS$$0` in the mapping only). Values are `int` equal
  to ordinal+1, and **0 = null**. The utility is described in §2. Call sites switch on
  `utility.ordinal(I)I`.
* **Enum constant names can be completely absent.** In `opt/`, `Mode {FAST, SLOW, OFF}` was used
  only for `==` and `ordinal()`. The strings `FAST`, `SLOW` and `OFF` appear nowhere in the DEX.
  The names survive only when `name()`, `toString()` or `valueOf()` is reachable: in `syn/` they
  appeared as a caller-side switch returning `"RED"`/`"GREEN"`/`"BLUE"`. So DESIGN §1's
  "Enum constant names: S from `<clinit>` strings" row holds **only when the strings exist**. When
  they do, the association `ordinal ↔ string` is exact (**S**). Otherwise the constant is
  `{Enum}_{hash}_k` (**D**).
* The constant count is S only if the `$VALUES` int array survives (`{1,2,3}` → n = 3). Otherwise
  it's a lower bound: the max literal compared.
* The whole re-boxing is **D** (the enum class name is unknowable). The ordinals are **S**.

### 4.4 Class inlining and scalar replacement

`Holder`/`In` in `syn/` disappeared entirely, and their fields became locals. There is no
fingerprint. **D·id.**

### 4.5 Outlining [V-exp `opt/`, V-src `ir/optimize/outliner/`]

The fingerprint is in §2. Each call site is `invoke-static outline(args…)` followed by
`move-result`, replacing the sequence. The outline body is *pure library calls on its
parameters*, so inlining it back is exact code motion (modulo registers). **D**, matching the
DESIGN row. It can't be S, because a hand-written static helper with the same shape is possible.
Detection tightening: ≥ 2 callers, straight-line code, only library invokes, and all parameters
used in order.

### 4.6 Argument removal, return-value removal, staticizing, devirtualization [V-exp `dontobf/`]

No DEX fingerprint when minified. With `-dontobfuscate`, a collision after rewriting produces
`name$N` (`work(String,int)` lost its unused `int` and became `work$1(String)`). **D·id.**
Candidate originals (a removed argument of unknown type, value and position) cannot be enumerated,
so this can't be N, except for the name: see §6.2.

### 4.7 Member value propagation and constant folding

Fields that are always constant are removed, and uses are folded (`Point.y = 2` became
`hashCode … + 2`, and the field disappeared along with its name in the record string). **D·id.**

### 4.8 Kotlin null checks [V-exp `kt/`, `kt2/`, V-src]

* R8 has no special rewrite that drops the strings. `kotlin.jvm.internal.Intrinsics` is ordinary
  **program** code: stdlib is in the input. It gets minified (`a.a`), its parameter type is
  specialized (`(Ljava/lang/String;Ljava/lang/String;)V`), and it is **inlined**, including into
  `throwParameterIsNullNPE`, with the message string **constant-folded**: `", parameter xname"`.
  R8 only uses `checkNotNullParameter` for nullability facts
  (`ir/optimize/info/MethodOptimizationInfoCollector.java:701`) and read-set modelling
  (`ir/analysis/modeling/LibraryMethodReadSetModeling.java`). When R8 proves the argument non-null
  at every call, the check is **removed**, string and all (`helper(tag, value)` kept `"tag"` but
  lost `"value"`).
* **This breaks DESIGN §1 "Kotlin intrinsics parameter names: S".** `kt2/`: kept Java method
  `javaSide(String javaParam)` calls public Kotlin `Lib.small(xname: String)`. R8 inlines `small`,
  so `javaSide`'s entry now holds a null check on **its own** parameter with the string `xname`. A
  kotlinc-private function, which has no check of its own, has the same problem. So a string next
  to a parameter register proves only that *some inlined callee* had a parameter with that name
  receiving this value.
  * **S** only if the check at a method's entry refers to that method's parameter **and** the
    method can't contain inlined code at that point. Without a mapping, the second condition isn't
    provable. Line tables can't help: they are compacted (§5).
  * So without a mapping this is **D (hint)**: the `{hint}` of `{hint}_{hash}`. A safer, provable
    sub-case: if the method's *own* Kotlin-metadata-derived signature exists (metadata kept), the
    parameter name is in the metadata anyway.
* R8 also *synthesizes* null checks. When it inlines an instance callee whose receiver may be null,
  it inserts `invoke-virtual Object.getClass()` with the result discarded
  (`ir/optimize/Inliner.java:836`, `ir/code/BasicBlockInstructionListIterator.insertNullCheckInstruction`),
  and the `Objects.requireNonNull` backport becomes `getClass()` too (`ObjectsMethodRewrites`).
  A discarded `getClass()` is therefore an **inlining scar**, a D hint that an instance call was
  inlined here. The callee identity is lost (**D·id**).

---

## 5. Line numbers and SourceFile without a mapping [V-src `utils/positions/`, V-exp `syn/`]

* **Line compaction** (`PositionRemapper.OptimizingPositionRemapper`). For DEX, `maxLineDelta = 1`,
  so every new position (a change of original line *or* inline frame) gets the next integer. The
  residual lines in each method are exactly **1, 2, 3, …, n**. Original lines survive only in the
  mapping. The only information left is *the pc boundaries where the (method, line, frame) triple
  changed*. That's a weak D hint for inlining boundaries: a discontinuity inside an otherwise
  straight-line expression.
* **PC encoding** (`InternalOptions.canUseNativeDexPcInsteadOfDebugInfo`): used when
  `min-api ≥ 26` (O) **and** the source file may be discarded
  (`SourceFileProvider.allowDiscardingSourceFile()`, true for `"SourceFile"` or empty). The
  method then has **no debug info at all**, and ART reports the pc as the line (0 `line=` entries
  at min-api 26). With min-api < 26, or a non-discardable source file such as the
  `r8-map-id-…` template, there are line tables with 1..n per method: 50 entries at min-api 21,
  and 69 at min-api 26 with `--source-file-template r8-map-id-%MAP_ID`.
* **SourceFile values** (`naming/SourceFileRewriter.java`, `DexItemFactory.defaultSourceFileAttributeString = "SourceFile"`):

  | Config | Value in DEX |
  |---|---|
  | full mode, `SourceFile` not kept | `"SourceFile"` |
  | full mode, `-keepattributes SourceFile`, minifying or optimizing, no rename | `"SourceFile"` (still) [V-exp] |
  | `-renamesourcefileattribute X` | `X` |
  | `--source-file-template r8-map-id-%MAP_ID` (AGP) | `r8-map-id-<pg-map-id>`, the same id as in the marker [V-exp] |
  | compat mode + `-keepattributes SourceFile`, no rename | **original file name** (`Foo.kt`) |
  | full mode, `-dontobfuscate -dontoptimize` + keep SourceFile | original |

  So an original-looking SourceFile is S only in the last two rows. Detect them with
  `r8-mode: compatibility` from the marker, or by the absence of any minified names. The
  `r8-map-id` value identifies a mapping file (look it up if the user has one) and carries no
  source information.
* Rules: `r8/lines-compacted` is **D·id** (strip or keep; never fabricate). `r8/sourcefile-original`
  is **S** when the value is not `SourceFile`, not `r8-map-id-*`, not empty, ends in `.java` or
  `.kt`, and the marker says compat mode (or nothing was minified). Otherwise **D**.

---

## 6. Ambiguity analysis

### 6.1 Classification

| Transformation | Without mapping | Why |
|---|---|---|
| Kept names (with §1.6 predicate) | **S** | Generator alphabet + length bound + exclusions |
| `it`/`by`/`do`/`if` member or class names | **S** | Reserved, never generated |
| Minified names | **D** | α-invariant `{hint}_{hash}` |
| Package of a non-minified class inside a repackaging target | **N** (§6.4) or D | `allowrepackage`/`-dontobfuscate` moves |
| `name$N` under `-dontobfuscate` | **N** (§6.2) | Finite candidate set |
| Repackaging suffix `Foo<digits>` in a target package | **N** (§6.3) | Finite candidate set |
| Backport, standalone, byte-exact | **S** or **N** (shared template) | Version template table |
| Backport inlined fragment | **D** | Optimized in context |
| API-model outline | **S** that it wraps API X | The body names the library member explicitly and it isn't renamed. Inlining back is exact. |
| `$-CC`, nest bridges, lambda classes (structure) | **S** structure, **D** names | Exact templates; the owner is explicit in the parameter types |
| Record desugaring | **S** "was a record, components in this order"; field names **D** | The names string is minified |
| Horizontal merge | **D** partition; **S** lower bound on count | §4.1 |
| Vertical merge, class inlining, arg removal, staticizing, value propagation | **D·id** | No residue; the preimage is unbounded |
| Enum unboxing | **D** class; **S** ordinals and names *if strings exist* | §4.3 |
| Outlines | **D** | A hand-written look-alike is possible |
| Kotlin `checkNotNullParameter` names | **D (hint)** | Inlined-callee counterexample |
| Lines | **D·id** | Compacted |
| SourceFile | **S** only in compat or unoptimized builds | §5 |

### 6.2 N: fresh member names (`-dontobfuscate` builds only)

`name ∈ .*\$[1-9][0-9]*` is produced by `createFreshMethodNameWithoutHolder(base, …)` = `base$N`,
or by `createFreshMethodNameWithHolder` = `base$<HolderSimple>$N`. Candidates, in canonical order
(the full string first, then shorter prefixes):
1. the name as-is (javac or kotlinc can emit `$1` names themselves: `access$000`, `this$0`);
2. `base`, if the name is `base$N`;
3. `base`, if the name is `base$H$N` and `H` is a residual or known simple name.

Exhaustive, because these are the only fresh-name constructors (graph/DexItemFactory.java:2881–2970).
**[V-src; `$1` V-exp]**

### 6.3 N: repackaging uniqueness suffix

This applies to a class whose simple name `S = T d₁…dₖ` (trailing decimal digits, no leading zero
on the stripped suffix) sits in a repackaging target. `getRepackagedType` appends `i` = 1, 2, …
until the name is unique. The suffix is appended once (`repackagedDexType.addSuffix(i)` on the
un-suffixed base), and `i ≥ 1` has no leading zero. So the candidates are `S` itself, plus `S`
with each nonempty trailing digit run removed whose first digit is `1`–`9`. Canonical order:
longest first. For example `Rep12` → [`Rep12`, `Rep1`, `Rep`], and `Rep10` → [`Rep10`, `Rep`].
[V-src; `Rep1` V-exp]

### 6.4 N: package of a kept-name class in a repackaging target

* `REPACKAGE` target `p`: the original package is unbounded, so there's no N. It's **D**
  (`p_{hash}`), or keep `p` as D·id.
* `-dontobfuscate` + repackage prefix `p` (not minifying): the new package is `p/old`. Candidates
  are every split of the path after `p/` (`p/a/b/c` → [`a/b/c`] if `p` is known). If `p` is
  unknown, the candidates are every suffix of the segment list, longest first, including the path
  itself (the case with no move). This is finite, so it's **N**.
* `MINIFICATION`/`FLATTEN` + `allowrepackage` (the package became `<gen>`): unbounded, so **D**.

### 6.5 Genuinely non-invertible (D·id)

Tree shaking, vertical merging, class inlining, argument and return removal, value propagation,
line compaction, access modification, removed enum names, horizontally merged static-only classes,
and inlining in general (no frames without a mapping).

---

## 7. Fixture ideas (one behaviour each)

Every fixture should record the R8 version, and ideally be built with two versions (8.10.9 and a
≥ 9.0 jar) where §1.2 differs.

1. `naming-digits`: one package with 40 classes and one class with 80 fields. Expect `a0`/`p0`
   names, and the predicate must return "maybe minified" for them (C1).
2. `naming-inner-kept-outer`: kept `Outer` with a static nested class, an inner class and a deep
   nested class, plus `-keepattributes InnerClasses,Signature`. Expect `Outer$a`, `Outer$a$a` (C2).
3. `naming-allowrepackage`: two `-keep,allowrepackage` classes with the same simple name in
   different packages. Expect `a.Rep`/`b.Rep` (8.x) or `Rep`/`Rep1` (9.x) (C3, C4, §6.3).
4. `naming-keepnames`: a `-keepnames` class. Package and name are unchanged, and it's S.
5. `naming-reserved`: a class with more than 60 members, checking that `do`/`if`/`it`/`by` are
   skipped (versions differ on `if`).
6. `naming-mixedcase`: 8.10.9 with and without `-dontusemixedcaseclassnames` (classes `A`–`Z`).
7. `naming-native`: an unkept `native` method gets renamed; the kept variant doesn't.
8. `naming-enum-values-reflect`: `EnumSet.allOf(E.class)` keeps `values` with no rules.
9. `dontobfuscate-fresh-names`: an overload collision after unused-arg removal gives `work$1` (§6.2).
10. `dontobfuscate-synthetics`: lambda, backport, `$-CC` and nest names stay visible (C7).
11. `pkg-default-mode`: identical input under 8.10.9 (packages `a`, `b`) and 9.x (root).
12. `hmerge-classid`: Circle/Square with non-trivial virtuals. Expect a classId field and a switch.
13. `hmerge-dead-id`: a group of 4 where one source is never instantiated. Observed ids are {0,1,2}
    or a set with a gap. Checks the lower-bound logic.
14. `hmerge-static-only`: two static-only util classes merged with no trace (D·id).
15. `enum-unbox-no-names`: an enum used only via `==`/`ordinal`. Assert that no name strings exist.
16. `enum-unbox-names`: the same enum with `name()` used. Assert S names through the switch.
17. `record-desugar`: `record Pt(int x, String y)`. Assert that the names string is `"a;b"`, not
    `"x;y"`.
18. `backport-inlined`: `Math.multiplyExact` with 12 kept callers. No synthetic survives.
19. `backport-d8`: the same code through D8 release. The synthetic body equals the template
    (anchor for §3's per-version table).
20. `backport-strictmath`: `StrictMath.floorMod` vs `Math.floorMod`. Identical bodies, so N.
21. `apimodel-outline`: `new NotificationChannel(…)` at min-api 21, behind `SDK_INT`.
22. `outline-stringbuilder`: 25 kept methods with the same `StringBuilder` chain. The outline
    method lands in a merged synthetic class.
23. `itf-companion`: default, static and private interface methods at min-api 21 (`$-CC`).
24. `nest-bridges`: an inner class touching an outer private field and method, with members
    pinned so the bridges survive.
25. `kotlin-nullcheck-inlined`: Java `javaSide(String)` inlines Kotlin `small(xname)`. The string
    `xname` must **not** become S for `javaSide`'s parameter.
26. `kotlin-nullcheck-removed`: a non-null argument proven at every call site, so the string is
    gone.
27. `lines-api21-vs-26`: the same code at min-api 21 and 26, with and without
    `--source-file-template`. Covers 1..n tables vs no debug info.
28. `sourcefile-compat`: `--pg-compat` + `-keepattributes SourceFile`. The original file name
    survives, and it's S.

---

## 8. Changes this implies for DESIGN.md / existing rules

* `r8/kept-name`: replace the preconditions with §1.6 (alphabet by version and casing, tail after
  the last `$`, a computed length bound, synthetic and fresh exclusions, a separate package test).
  C1 and C2 are the urgent ones.
* §1 rows to amend: Backports (§3), Kotlin intrinsics parameter names (§4.8: D-hint without a
  mapping), Enum constant names (§4.3: "when present"), Record `toString` (§2: field names are
  **minified**, not original), Horizontal merging (§4.1: count is only a lower bound),
  Source file (§5: compat-mode S case).
* §5 item 1: "JNI `native` methods" are not kept by R8 itself, only by AGP's default rules.
