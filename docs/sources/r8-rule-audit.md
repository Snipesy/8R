# Adversarial audit of the R8 rulebook (no-mapping case)

Scope: every R8/desugar row of DESIGN.md §1, evaluated **without a mapping**, plus the
preconditions and implementation of `r8/kept-name`
(`crates/eightr-rules/src/lib.rs`, `crates/eightr-core/src/passes/kept_name.rs`).

- **R8 source**: `https://r8.googlesource.com/r8`, branch `8.10`, commit
  `decf0a4af222e0d21cc87a5e9390f334f49ebdfa` (tag `8.10.40`). All `file:line` references below
  are relative to `src/main/java/com/android/tools/r8/` at that commit.
- **R8 binary used for experiments**: 8.10.9-dev (`build-tools/36.0.0/lib/d8.jar`), full mode,
  `--release`. javac 26 with `--release 8/11/17`.
- **Experiments**: scratch directory
  `/tmp/claude-1000/-home-snipesy-8R/4b02079c-3b0c-464c-bf8a-78f254b90fa3/scratchpad/agents/r8-audit/`
  (`eN/`: `src/`, `rules.pro`, `map.txt` = ground truth, `dump.txt` = dexdump). The driver is
  `run.sh <dir> <min-api>`. The essential source and rules for each experiment are reproduced
  inline below, so this document stands on its own.

## 0. Summary of verdicts

| Row / rule | DESIGN claim (no mapping) | Verdict | Key evidence |
|---|---|---|---|
| `r8/kept-name`: class & member names | S | **UNSOUND as implemented. DOWNGRADE until fixed, then SPLIT** | E1 (`a0` names under default settings), E3/E18 (`-dontobfuscate`: `$r8$classId`, `…$$ExternalSyntheticLambda1`, `calc$1`), E4 (dictionary/applymapping) |
| `r8/kept-name`: package | S | **DOWNGRADE → D** (or S only with independent evidence) | E2 (`-keep,allowrepackage` → `vendorinternal.PaymentProcessor`) |
| Class/member renaming | S where proven, else D | CONFIRMED in structure; the S sources in §5 are individually downgraded below | — |
| Repackaging | D | CONFIRMED, but it happens **by default** (not only with `-repackageclasses`) | E1 |
| Aggressive overloading | D | CONFIRMED | — |
| Line numbers | D·id | CONFIRMED | — |
| Source file | D | CONFIRMED. The premise is wrong for 8.10: plain R8 full mode writes `SourceFile`, not `r8-map-id-…`. R8 9.4.24 writes `r8-map-id-…` by default | E17, 9.4.24 fixtures |
| Inlining | D·id | CONFIRMED | E10, E13 |
| Outlining | D | CONFIRMED (α-invariant); outlines are **on by default**. R8 ≥ 9 adds bottom-up throw outlines. Implemented: `r8/outline-inline`, `r8/bu-outline-inline` | E6c, §3.7 |
| Lambda desugaring | D | CONFIRMED | — |
| Backports | S | **DOWNGRADE → SPLIT (S / N)**. Math and StrictMath share templates, producing byte-identical dex | E8 |
| API-model outlines | S | CONFIRMED **for the call target only**, with precondition (synthetic holder) | E13 |
| `$-CC` companions | S (structure) | **DOWNGRADE → SPLIT (S / N / D)**. Default and static methods are indistinguishable; static methods have no owner link | E14 |
| Nest-access bridges | S (structure) | **SPLIT** on `ACC_BRIDGE`. Without it, javac `access$NNN` is original code | E9 |
| Horizontal merging | D | CONFIRMED D, but the partition is often **invisible**: no `$r8$classId`, different interfaces, shared fields | E3, E6g |
| Vertical merging | D·id | CONFIRMED | — |
| Enum unboxing | D; constant names S | Class D CONFIRMED. **Names DOWNGRADE → hint.** Strings are deleted when unused and inlined into if-chains | E3, E7 |
| Staticizing / arg removal / return removal / arg reorder | D·id | CONFIRMED. No residual evidence: the Signature annotation is dropped | E15 |
| Access modification | D | CONFIRMED D. The fixture expectation "no wider than original" is **false** | reasoning, §3.13 |
| Constant propagation, tree shaking, class inlining | D·id | CONFIRMED | — |
| Local names / generics / annotations "S if kept" | S if kept | **DOWNGRADE generics → D**. Kept Signature strings contain minified type names | §3.16 |
| Kotlin intrinsics param names | S | **DOWNGRADE → hint**. Inlining moves `checkNotNullParameter(p,"title")` into a Java caller whose parameter is `headline` | E10 |
| Enum constant names (`<clinit>`) | S | **SPLIT / DOWNGRADE**. Pre-obfuscated SDK input breaks it (the oracle fails) | E12 |
| Data-class / record `toString` | S | **Records: DOWNGRADE → nothing** (R8 rewrites the name string to minified names). **Kotlin: DOWNGRADE → hint** | E11, §3.20 |
| kotlinx.serialization / log tags / SigDB | D hint | CONFIRMED | — |

New sound rules proposed (§4): `r8/library-override-name` (S), `r8/annotation-member-name`
(S, verified E16), `r8/jni-symbol` (S, argued, not run), plus a corrected `r8/kept-name`.

---

## 1. How R8 names things (source facts every rule depends on)

### 1.1 The name generator is `[A-Za-z][0-9A-Za-z]*`, not `[A-Za-z]{1,3}`

`utils/SymbolGenerationUtils.java`:

```java
private static final char[] IDENTIFIER_CHARACTERS =
    "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".toCharArray();
private static final int NON_ALLOWED_FIRST_CHARACTERS = 10;   // digits never first
```

`numberToIdentifier(n, mixedCasing)` emits a letter first, then characters from the full
62-character alphabet (digits included).

| Casing | 1-char names | 2-char names start at | first 2-char name | 3-char names start at | 4-char names start at |
|---|---|---|---|---|---|
| mixed (default for classes **and** members) | 52 | #53 | `a0` | #3 277 (52 + 52·62) | #203 165 |
| `-dontusemixedcaseclassnames` (classes/packages only) | 26 | #27 | `a0` | #963 (26 + 26·36) | #34 659 |

Additional facts:
- `RESERVED_NAMES` = {boolean, byte, char, double, float, int, long, short, void, it, by, do}
  are skipped.
- Class names ending `/R;` are skipped (`Minifier.MinificationClassNamingStrategy.next`).
- Members always use mixed case: `MinifierMemberNamingStrategy` passes `false` for
  `dontUseMixedCasing`.
- Namespaces:
  - Classes: one namespace per package, or per outer class when inner structure is kept
    (`ClassNameMinifier.Namespace`).
  - Packages: one global counter (`MinificationPackageNamingStrategy`).
  - Members: reservation states per hierarchy frontier (`MethodNameMinifier`).
- L8 (desugared library) prefixes class names (`L8MinificationClassNamingStrategy`).

So **`a0`, `b0`, `Z9`, `a0b` are ordinary default minified names**. A package with more than 52
minified classes, or a class with more than 52 renamed members in one naming state, is
enough. 3-character names with digits appear once a namespace passes 3 276 names, which is
realistic after repackaging.

### 1.2 Inner classes

`ClassNameMinifier.computeName`: when `keepInnerClassStructure()` is true, which is when
`-keepattributes Signature` **or** `InnerClasses` is set (`InternalOptions.java:965`), a member
class is named in a namespace prefixed by its outer's *renamed* name plus the original
separator (`$`, `$$`, `$…$`; see `DescriptorUtils.computeInnerClassSeparator`). This produces
`Outer$a`, `a$a`, `Main$a0`, `Outer$$b`.

If an inner class is kept, `registerClassAsUsed` force-keeps its outer. Without inner
structure, inner classes are named in the package namespace like any other class.

### 1.3 Packages move by default

`ProguardConfiguration.java:367`: when obfuscating and no package option is given, the mode
becomes `MINIFICATION`. `Repackaging.DefaultRepackagingConfiguration.getNewPackageDescriptor`
then moves whole packages to fresh minified package names (`a`, `b`, …, `a0`, …). The only
exception is a package that holds a pinned package-private or protected item. **E1 confirms
this with no `-repackageclasses` at all**: `com.example.app.Worker*` → `a.*`.

Other modes:
- `-repackageclasses X` puts everything into the user string `X`.
- `-flattenpackagehierarchy X` gives `X.<minified>`.

`-keep,allowrepackage` (`ProguardConfigurationParser.java:1082`) disallows minification but
**allows repackaging** (`RootSetUtils.java:1805-1815`). The simple name is then kept while
the package changes.

### 1.4 Names R8 invents that are *not* minified-shaped

These survive whenever minification is off (`-dontobfuscate`) or the holder is kept:

- **Classes:** `$$ExternalSynthetic{Lambda,Outline,Backport,ApiModelOutline,…}N`,
  `$-CC`, `$EnumUnboxingSharedUtility` / `$EnumUnboxingLocalUtility`, `$Wrapper`,
  `-IA`, `R8$$SyntheticClass`, `R8$$REMOVED$$CLASS$$N` (mapping only).
  Source: `synthesis/SyntheticNaming.java`. **R8/D8 9.4.24 name synthetics `Outer$N`**
  (`Main$0`, `Account$0`) instead of `$$ExternalSynthetic…N`, which looks like a javac anonymous
  class; only `ACC_SYNTHETIC` tells them apart [V-exp 9.4.24 fixtures].
- **Members:**
  - Merging and desugaring: `$r8$classId` (`ClassMerger.java:57`), `$r8$clinit`,
    `$r8$lambda$…`, `$r8$backportedMethods$utility…`, `$r8$twr$utility`,
    `$r8$java8methods$utility`, `$r8$init$synthetic`.
  - Nest bridges: `-$$Nest$fget…`, `-$$Nest$m…`, `-$$Nest$sm…`.
  - Lambda captures: `f$0`, `f$1`.
  - Synthetic helpers: `m` (a single-method synthetic) and `$VALUES` (on the enum-unboxing
    utility).
  - **Fresh-name collisions `name$N`**: `DexItemFactory.createFreshMethodNameWithoutHolder`,
    used by argument propagation, access modification, class merging, enum unboxing, and
    fix-ups.

---

## 2. `r8/kept-name`

### 2.1 The rule as implemented

`could_be_minified(n) = 1 ≤ len ≤ 3 && all ASCII letters`. A class is labeled ClassName **and
Package** S when neither the simple name nor its last `$` segment could be minified. Members
are labeled S when not minified-shaped.

**The unit test enshrines three of the bugs below:** `"a1"`, `"$r8$classId"` and
`"lambda$main$0"` are asserted to be *not* minified.

### 2.2 Counterexamples

**E1: digits in default minified names (CRITICAL, default settings).**

Source and rules:
- 70 public classes `com.example.app.Worker0..69`, each with a static `go(int)`.
- `Main` with a nested `Big { public int field0..field69; String toString0..69() }`, plus
  `Inner`, `Nested`, `Helper`, `Helper.HInner`.
- `rules.pro`:
  ```
  -keep class com.example.app.Main { public static void main(java.lang.String[]); }
  -keepattributes InnerClasses,EnclosingMethod,Signature
  -dontoptimize
  ```

Ground truth (`e1/map.txt`):
```
com.example.app.Worker5  -> a.e0
com.example.app.Worker60 -> a.f0
com.example.app.Worker69 -> a.o0
com.example.app.Main$Big -> com.example.app.Main$a
    int field52 -> a0
    int field53 -> b0
com.example.app.Helper$HInner -> com.example.app.a$a
```

What 8R does:
- `La/e0;` has simple name `e0`, and `could_be_minified("e0") == false`. So ClassName is
  labeled **S = `e0`** (truth `Worker5`) and Package is labeled **S = `a`** (truth
  `com.example.app`).
- Field `a0` is labeled **S** (truth `field52`).

This happens with only default options, in any app with more than 52 classes moved into one
package or more than 52 members in one namespace. That is essentially every real app.

**E3 / E6c / E18: `-dontobfuscate` exposes R8-invented names (CRITICAL).**

`-dontobfuscate` is common (open-source apps, F-Droid builds, libraries). Every name is then
"not minified-shaped", and R8-invented names get S:

| dex name (E3 `p.App`, E6c, E18) | 8R label | truth |
|---|---|---|
| class `p/App$$ExternalSyntheticLambda1` | S | synthetic, no original |
| class `p/App$Color$EnumUnboxingSharedUtility` + field `$VALUES:[I` | S | synthetic |
| field `p/App$Sq.$r8$classId` | S | synthetic (Sq and Circ merged; field `s` also holds `Circ.r`) |
| field `f$0` in the lambda class | S | synthetic |
| class `q/Main$Adder$$ExternalSyntheticOutline0` | S | synthetic outline |
| method `calc$1(I)I` (E18) | S | **original name is `calc`** (`calc(int,String)` after unused-arg removal collided with `calc(int)`) |

E18 source, with `-keep class q.Main { public static void main(...); }` and
`-dontobfuscate`:
```java
static int calc(int a) { … }                 // stays calc
static int calc(int a, String unused) { … }  // becomes calc$1(I)I
```

The `name$N` case is the worst of these. It is not a synthetic item: it is a real user
method whose name R8 **changed** to a non-minified-shaped string.

**E4: `-applymapping` and `-obfuscationdictionary` (unsound, undetectable).**

Source `e2/src` (`org.demo.pay.{Main, Ledger, PaymentProcessor}`).
- With `-applymapping apply.txt` containing
  `org.demo.pay.Ledger -> org.demo.pay.AuditTrail:` / `long total -> balance` /
  `void record(long) -> commit`, the output contains `org.demo.pay.AuditTrail.commit` /
  `.balance`. All are labeled S and all are wrong.
- With `-obfuscationdictionary` / `-classobfuscationdictionary` /
  `-packageobfuscationdictionary` over `{alpha, bravo, …, PaymentGateway, userId}`, the
  output is `alpha.alpha`, `alpha.bravo`, with fields and methods `alpha`. All are labeled S.

The precondition "the build used R8's default naming" **cannot be checked from the dex**. That
violates DESIGN §0.2.2 ("checked at runtime"). `-applymapping` is used in real incremental
and hot-patch pipelines (Tinker and similar), and dictionaries are used by hardening-minded
apps.

**E2: kept simple name, changed package.**

```
-keep class org.demo.pay.Main { public static void main(java.lang.String[]); }
-keep,allowrepackage class org.demo.pay.PaymentProcessor
-repackageclasses 'vendorinternal'
-dontoptimize
```

Truth: `org.demo.pay.PaymentProcessor -> vendorinternal.PaymentProcessor`.

8R labels the class S = `PaymentProcessor` (correct) and the **package S = `vendorinternal`
(wrong)**. The segment check in DESIGN's precondition list passes (`vendorinternal` is 14
characters), and the implementation does not even perform that check: it copies the class
verdict to the package.

So "R8 renames a descriptor atomically" is false. The package is subject to repackaging,
which is controlled separately from minification. The same happens with
`-flattenpackagehierarchy 'vendorinternal'` for `-keep,allowrepackage` classes.

**Questions from the brief:**
- *`-keepnames`, `-keep`:* sound (no minification, no repackaging).
- *`-keep,allowobfuscation`:* produces minified names, handled once §1.1 is fixed.
- *`-useuniqueclassmembernames`, `-overloadaggressively`:* still use the generator, so they
  are sound after the regex fix.
- *`-adaptclassstrings`:* does not affect names. It **does** destroy reflection-string
  evidence: `Class.forName("x.Y")` strings are rewritten to the minified name. Such a string
  never proves originality; it just echoes the residual name.
- *Kotlin metadata:* R8 rewrites `d1`/`d2` to residual names, so it is not name evidence
  (DESIGN §5.6 already says so).
- *Inner classes:* the outer can keep its name while the inner becomes `Outer$a`. The
  `last_segment` split handles that, but not `Outer$a0` or `a$b0`.
- *Minified names longer than 3 characters:* only past about 203k names in one mixed-case
  namespace (about 34.7k with `-dontusemixedcaseclassnames`). 3-character names **with
  digits** appear past 3 276.

### 2.3 Verdict and fix

**DOWNGRADE** the whole rule to "not S" until it is fixed. Then **SPLIT**:

1. Minified-shape test: `^[A-Za-z][A-Za-z0-9]{0,2}$`. Widen to 4 when a namespace in the
   input has more than 3 000 items, or more than 900 when all class names are lowercase
   (a `-dontusemixedcaseclassnames` signature). Apply it per `$` segment of the class simple
   name, and to every package segment.
2. Never S for:
   - `ACC_SYNTHETIC` classes and members, and members of synthetic classes (except library
     overrides, §4.1);
   - any name containing `$$`, `$r8$`, `-$$`, `$-CC`, `$-EL`, `$-DC`, or `-IA`;
   - any method name matching `.*\$[0-9]+$` (fresh-name collisions);
   - `f$[0-9]+` fields.
3. **Global refusal.** A dictionary or applymapping build is not provable, so require a
   detector and refuse S app-wide when it fires. It fires when either holds:
   - the same non-generator name is reused across unrelated namespaces (e.g. `alpha` as a
     package, a class and a member);
   - there are fewer than k generator-shaped names in an app with more than N renamed-looking
     items.

   This is heuristic, so the result is "S under assumption A-default-naming". DESIGN must
   either admit assumption-qualified S or downgrade to D.
4. **Package: not S from kept-name alone** (E2). S only with independent evidence (§4.3/§4.4:
   manifest, JNI).
5. Fix the unit test (`a1`, `$r8$classId` and `lambda$main$0` must not be proof).
   `lambda$main$0` is a javac name and is original, but under `-dontobfuscate` R8 can also
   emit `$r8$lambda$…`. Keep javac-pattern members only when not `$r8$`.

---

## 3. Row-by-row

### 3.1 Class/member renaming: CONFIRMED structure

This row delegates to §5 evidence. Each S source is audited in its own row below.

### 3.2 Repackaging (D): CONFIRMED, with a note

Repackaging is the default (§1.3, E1), so every no-mapping run faces it. Classes still in a
long-named package with a minified simple name (E1: `com.example.app.a`) are in their
original package only if no `-repackageclasses` / `-flattenpackagehierarchy` target
coincides with it. That cannot be proven, so D is correct.

The fallback "D·id (flat)" is fine, because the user-chosen target is not a minified
identifier.

### 3.3 Aggressive overloading (D): CONFIRMED

### 3.4 Line numbers (D·id): CONFIRMED

### 3.5 Source file (D): CONFIRMED, premise corrected

`naming/SourceFileRewriter.computeNonCompatProvider`: in full mode, when minifying **or**
optimizing, the attribute is rewritten to the constant `"SourceFile"` even with
`-keepattributes SourceFile` (E17: all four classes, even with `-dontobfuscate`).
In 8.10, `r8-map-id-…` only appears when the build tool passes a source-file template (AGP does).
R8 9.4.24 writes `r8-map-id-<pg-map-id>` with no template (every `*_r94`/`r94_*` fixture, with
or without `-keepattributes SourceFile`) [V-exp 9.4.24].

Upgrade candidate (§4.5): compat mode (`r8-mode:"compat"` in the marker) with
`-keepattributes SourceFile` keeps per-class originals.

### 3.6 Inlining (D·id): CONFIRMED

E13 shows a user helper `helper(t,"c")` inlined into `c` and then API-outlined. Nothing marks
the boundary.

### 3.7 Outlining (D): CONFIRMED

The outliner is **enabled by default** in R8 8.10: E6c produced
`q.Main$Adder$$ExternalSyntheticOutline0.m(String,int,PrintStream)` shared by 23 call sites
across two classes.

The discriminator first proposed here, "the holder is `PUBLIC ABSTRACT SYNTHETIC` (0x1401) with
only static methods", is **wrong** [V-exp R8 9.4.24 fixtures, Gretio]: holder flags don't discriminate. In
9.4.24 a classic-outline holder is `PUBLIC ABSTRACT SYNTHETIC`, a bottom-up (throw) outline holder
is `PUBLIC FINAL SYNTHETIC`, and horizontal merging can put an outline into a merged lambda
group with instance fields, `<init>` and `$r8$classId` (`kotlin_serialization_r94`
`Account$0`). What holds is `ACC_SYNTHETIC` on the holder (javac and kotlinc don't set it on the
classes that hold hand-written helpers). Several R8/D8 synthetics share that holder shape, so
the method body decides (r8-desugar.md §4.5): classic outlines have ≥ 3 operations including a
call and touch only library classes; throw outlines build and throw an exception. Look-alikes
this excludes: D8 backports (`Main$0.m(J)I`, pure arithmetic), `$-CC` companions (call app
code), API-model outlines (2 operations), and hand-written helpers in non-synthetic classes
(Compose `PreconditionsKt.throwIllegalArgumentException`, `ArraysKt.copyInto`). Inlining a
static method also needs no `<clinit>` in the holder's superclass chain, no program subclasses,
and per-site accessibility. Precision and recall are 100% against every fixture mapping.

α-invariance holds if the decision uses only flags and structure, never the name
(`$$ExternalSyntheticOutline` in 8.x, `Outer$N` in 9.4), which only survives under
`-dontobfuscate` or in D8 output.

Caveat: the input may itself be pre-desugared (a library jar produced by D8/R8 CF output).
There the outline is original code. D still holds.

### 3.8 Lambda desugaring (D): CONFIRMED

### 3.9 Backports (S): DOWNGRADE → SPLIT (S / N)

Source: `ir/desugar/BackportedMethodRewriter.java:812-910, 1396-1415, 1935, 1963` loops over
`{Math, StrictMath}` and registers **the same template** for both. Examples:
`MathMethods_addExactInt`, `floorDiv`, `floorMod`, `multiplyExact`, `nextDown`,
`subtractExact`, `toIntExact`, `multiplyExact(long,int)`, `multiplyFull`.

A textual comparison of `ir/desugar/backports/BackportedMethods.java` also finds
`ByteMethods_compare == CharacterMethods_compare == ShortMethods_compare`. These are
distinguishable only by proto (B/C/S), which R8 may narrow or widen.

**E8 (verified).** Two programs differ only in `Math.multiplyExact` vs
`StrictMath.multiplyExact`:
```java
long x = a.length * 3_000_000_000L + 7;
for (int i = 0; i < a.length + 5; i++) { x = Math.multiplyExact(x, (long)(i + a.length + 3)); System.out.println(x); }
System.out.println(Math.multiplyExact(x, 11L) + Math.multiplyExact(x ^ 5, 13L));
```
Rules: `-keep class q.Main { public static void main(java.lang.String[]); }`, `--min-api 21`.

Result: **the two `dexdump -d` outputs are identical** apart from header checksum and
signature. The only difference in the file is the `pg-map-id` inside the `~~R8` marker
(`2742260` vs `361e3b6`), which is a hash of the mapping and carries no information. Note too
that the backport was **inlined into `main`**, leaving no synthetic method at all.

Verdict:
- **S** only when the template (for the marker's R8 version) is unique across the whole
  provider table, compared by DEX proto and body.
- **N** with an exhaustive candidate set when it is shared: {Math.X, StrictMath.X} and
  {Byte, Character, Short}.compare. The candidate set is enumerable from the version-keyed
  table, so the N is exhaustive.
- Also require the body to match the template exactly: R8 runs the normal optimizer over
  synthetics, so bodies are often specialized, which means refusal.
- When the marker is missing, D (as DESIGN says).

### 3.10 API-model outlines (S): CONFIRMED for the call target

E13 source (min-api 21, API 26 `setTooltipText`):
```java
public static void a(TextView t, String s) { t.setTooltipText(s + "!"); }
public static void b(View v, String s)     { v.setTooltipText(s + "?"); }
static void helper(TextView t, String s)   { t.setTooltipText(s); }
public static void c(TextView t)            { helper(t, "c"); helper(t, "d"); }
```

The output holder `a.a` is `0x1401 PUBLIC ABSTRACT SYNTHETIC`, with two methods. One does
`invoke-virtual View.setTooltipText`, the other `TextView.setTooltipText`. The holder class is
**preserved per site** (javac's receiver type), so the target reference is uniquely
determined: S for "this call site invokes X".

Not S:
- The outline proto was narrowed (`String` instead of `CharSequence`), so the outline's
  signature is not a signature to restore.
- The body of `c` after un-outlining is `t.setTooltipText("c")`, not the original
  `helper(t,"c")` (inlining, D).

Precondition: holder is ACC_SYNTHETIC|ACC_ABSTRACT, method is static, and the body is exactly
[moves] + one library member access + return. `StaticInterfaceCall` synthetics have the same
shape and the same "inline back gives original call" semantics, so the target claim still
holds.

### 3.11 `$-CC` companions (S structure): DOWNGRADE → SPLIT

E14 source (min-api 21):
```java
public interface Shape { double area();
  default String describe() { … area() … }
  static Shape scaled(Shape s, double k) { return () -> s.area()*k*k + (k>3?1:0); } }
public interface Named { static String fmt(String n, int x) { … } }
```
Rules: keep `main`; `-keep,allowobfuscation interface q.Main$Shape { *; }`;
`-keep,allowobfuscation interface q.Main$Named`.

Output companion `a.c` (`0x1401`):
- `a(La/d;)String` for `describe`;
- `b(La/d;D)D` for the lambda body;
- `c(La/d;D)La/d;` for `scaled`.

`Named.fmt` left no companion (inlined).

- A default method `R m(A)` becomes `static R m(I, A)`, and a static interface method
  `static R m(I, A)` becomes **the same**. `scaled(Shape,double)` and a hypothetical
  `default Shape scaled(double)` are indistinguishable by shape. That is **N** (2
  candidates) unless there is evidence: the interface still declares abstract `m(A)`, **and**
  implementers carry forwarders `m(A){ return C.m(this, A); }` (seen in E14: `Sq.b()` and the
  lambda `a.b.b()`). With that evidence it is a default method: **S**.
- A static interface method whose parameters don't mention the interface has **no owner
  evidence** at all. The companion's link to its interface is only the name `I$-CC`, which is
  minified away. So owner is D (or N over all interfaces), never S.
- Companion classes have the same flags as other global synthetics, and can be horizontally
  merged with other companions (same synthetic kind).

### 3.12 Nest-access bridges (S structure): SPLIT on ACC_BRIDGE

E9 compiles the same source with `--release 8` (javac `access$000/008/100`) and `--release 11`
(R8 nest desugaring `-$$Nest$fgetsecret` etc.):
```java
public class Outer { private int secret; private static String tag(int x){ return "t"+x; }
  class Inner { String peek() { secret++; return tag(secret); } }
  public static void main(String[] a) { Outer o = new Outer(); o.secret = a.length; System.out.println(o.new Inner().peek()); } }
```
Rules: keep `main`, `-dontoptimize`.

Result, after minification both sets become `Outer.a/b/c`:

| | javac `access$` | R8 nest bridge |
|---|---|---|
| access flags | `0x1008 STATIC SYNTHETIC` | `0x1048 STATIC BRIDGE SYNTHETIC` |
| shape | `access$008` = read+increment+write | separate `fget`/`fput` |

Since javac `access$NNN` and Kotlin `access$get…$p` (PUBLIC STATIC FINAL SYNTHETIC, no
BRIDGE) are **original input code**, inlining them back is not "the original":
- **S** only with `ACC_STATIC|ACC_BRIDGE|ACC_SYNTHETIC` and a body that is a single access to
  a member of another class. javac never sets BRIDGE on static methods.
- Otherwise **D**.

Also: in optimized builds R8 usually inlines nest bridges and widens access, leaving nothing
to undo (D·id).

### 3.13 Horizontal class merging (D): CONFIRMED, but the premise is weaker

Source facts:
- `horizontalclassmerging/ClassMerger.java:435-458`: the classId field is created **only** if
  some virtual-method signature has a non-trivial merge. Classes with disjoint method sets,
  and static-only classes (`OnlyClassesWithStaticDefinitionsAndNoClassInitializer`), merge
  **without** `$r8$classId`.
- Class ids are target=0, then sources in group order (`ClassMerger.java:102-103`). The field
  gets an abstract value, so dead ids may be folded.
- `policies/NoIndirectRuntimeTypeChecks`: interfaces may differ **unless** they (or a
  super-interface) are library interfaces or used in `instanceof`/`check-cast`.
- Policies require `SameParentClass`. Instance fields are shared or merged
  (`ClassInstanceFieldsMerger`).

**E6g (verified): different interfaces, no classId.**
```java
public interface Greeter { String greet(String n); }
public interface Counter { int count(int x); }
public class Hello implements Greeter { public String greet(String n) { … } }
public class Adder implements Counter { public int count(int x) { … } }
public class Main { static Greeter g; static Counter c;
  public static void main(String[] a) { g = new Hello(); c = new Adder(); … g.greet(..) … c.count(..) … } }
```
Rules: keep `main`; `-keep,allowobfuscation interface q.Greeter { *; }`; same for `Counter`.

Truth: `q.Adder -> a.a`, and `q.Hello.greet` is mapped into `a.a.b`. Output: `a.a implements
a.b, a.c`, **no fields**, and both `new Hello()` and `new Adder()` became `new a.a; invoke-direct
Object.<init>`.

**E3:** `Sq` and `Circ` (both `implements Shape`) merged into `App$Sq` with `$r8$classId`, and
field `s` now also stores `Circ.r`.

Answers:
- The number of distinct classId values is **not** the class count. It is 0 when no
  dispatch is needed, and it can be fewer than the group size when ids are dead.
- Merged classes **can** have had different interfaces (E6g), and one physical field can
  serve several originals (E3).

Verdict: D as stated, applied only when a classId exists and every read is a dispatch switch.
Otherwise D·id; E6g's merge is simply invisible.

α-note: don't derive split-class names or order from classId numerals. They reflect R8's
group order, the same "arbitrary choice" status as minified names.

### 3.14 Vertical merging (D·id): CONFIRMED

### 3.15 Enum unboxing (class D, constant names S): names DOWNGRADE → hint

**E3 / E7 (verified):**
```java
enum Level { LOW, HIGH }  … System.out.println(l.ordinal() * 10);   // name never used
enum Named { ALPHA, BETA } … System.out.println(n.name());
```
Output:
- `LOW`/`HIGH` **do not exist anywhere in the dex** (unboxed, strings dropped). E3's
  `RED`/`GREEN`/`BLUE` are likewise gone.
- `ALPHA`/`BETA` survive only as an **inlined if-chain in `main`**: `if-eq v1,1 → "ALPHA";
  if-ne v1,2 → throw null; "BETA"`. That is structurally a user `int → String` map.
- `Mode`, which overrides `toString` with a switch returning `"Turbo"/"Eco"/"Disabled"`, was
  not unboxed here. A future R8 that unboxes it would produce exactly the same if-chain with
  display strings. A rule reading "strings in the name utility" would then label `Turbo` S.

So "constant names always survive" (§5.5) is false, and when they survive, provenance is not
provable. They are hints.

### 3.16 Staticizing, arg removal, return removal, arg reordering (D·id): CONFIRMED

E15:
```java
<T extends Comparable<T>> int work(List<T> items, Map<String,T> unused, int k)
int noThis(int x)
```
Rules: `-keepattributes Signature,InnerClasses,EnclosingMethod`.

Both methods became `public static`, `work` became `(List,I)I`, and **no `dalvik.annotation.
Signature` survives**, so there is no stale generic signature revealing the removed
parameter. The instance field `base` was constant-folded away. No residual evidence exists;
D·id is right.

Related: "generics S if kept" is wrong for Signature attributes that do survive. R8 rewrites
their type names to residual (minified) names, so at best the *shape* is original: D.

### 3.17 Access modification (D): CONFIRMED D; fixture expectation false

α-invariance holds: the minimal verifying access is a function of the reference graph.

But §6.3 `access/tighten` says access "must be restored to the original, or to something no
wider". Without un-inlining, R8's cross-class inlining leaves references the original never
had. Example: `A.getX()` is inlined into `B`, so `B` now reads `A.x` directly. The minimal
verifying access for `x` is then **wider** than the original `private`. Drop "no wider".

### 3.18 Constant propagation, tree shaking, class inlining (D·id): CONFIRMED

### 3.19 Kotlin intrinsics param names (S): DOWNGRADE → hint

**E10 (verified).** Stand-ins are used because kotlinc isn't installed:
- `kotlin/jvm/internal/Intrinsics.java` mirrors stdlib: `checkNotNullParameter(Object,String)`
  calls a large private `throwParameterIsNullNPE`.
- `q/K.java` is exactly what kotlinc emits for `fun render(title: String)`:
  ```java
  public static final void render(String title) {
    Intrinsics.checkNotNullParameter(title, "title"); System.out.println("len=" + title.length()); }
  ```
- `q/J.java` is ordinary Java: `public static void show(String headline) { K.render(headline); }`.
- `q/K2.java` has three more Kotlin-style checks, so the helper keeps several callers.
- Rules: `-keep class q.J { public static void main(java.lang.String[]); public static void show(java.lang.String); }`.

Output `q.J.show(Ljava/lang/String;)V`:
```
0000: const-string v0, "title"
0002: invoke-static {v3, v0}, La/a;.a:(Ljava/io/Serializable;Ljava/lang/String;)V   // = Intrinsics.checkNotNullParameter
```
Parameter 0 of `show` gets the canonical `checkNotNullParameter(p0, "title")` shape. **8R would
label it S = `title`; the truth is `headline`.**

The same happens in pure Kotlin: kotlinc emits no checks for `private`/`internal` functions,
so a private caller of a public function inherits the callee's name. It also happens for
every inlined public-to-public call where the caller's own check was removed.

Also in E10: with a small Intrinsics, R8 fully inlined and constant-folded it to
`", parameter title"`. That is a benign refusal.

Verdict: hint. The only S-safe variant would require proof that the method was not the target
of inlining, which isn't available without a mapping.

### 3.20 Enum constant names from `<clinit>` (S), and data-class/record `toString` (S)

**Records: DOWNGRADE to "no evidence" (E11, verified).**
```java
record Point(int xCoord, int yCoord) {}
```
Compiled with `--release 17`, min-api 21 and 34. The desugared `toString` uses the string
**`"a;b"`**, the *minified* field names. There is no `"Point"` string. R8 rewrites the
record-components string after minification. A rule reading it would label fields `a`/`b`.
Delete records from §5.4.

**Kotlin data classes: DOWNGRADE → hint.**
- R8 strips `kotlin.Metadata` for non-kept classes, so the "verified compiler-generated shape"
  cannot be verified. A hand-written `override fun toString() = "User(name=" + name + ")"` in
  `class Account`, or the equivalent Java
  `new StringBuilder("User(name=").append(name).append(')').toString()`, compiles to the
  same instruction sequence kotlinc emits for `data class User`. After R8 they are
  indistinguishable, so this is a counterexample by construction.
- The printed name is the **inner simple name** (`User`), not the binary simple name
  (`Outer$User`).
- `NoKotlinMetadata` only blocks horizontal merging while metadata is kept.

**Enum `<clinit>` names: SPLIT (S only under an explicit input-provenance assumption).**

E12 (verified): an SDK is pre-obfuscated first, as Play Services, Firebase and many ad SDKs
ship.

Stage 1: `R8 --classfile` with `-keep public class sdk.Api { public *; }` and `-dontoptimize`
over:
```java
public class Api { public enum Region { EUROPE, AMERICA, ASIA }
  public static Object region(int i) { return Region.values()[i % 3]; }
  public static String describe(Object r) { return "region#" + ((Region) r).ordinal() + ":" + r; } }
```
This produces `sdk.Api$Region -> a.a`, with fields `EUROPE -> a` etc.

Stage 2 (the app build): `app.Main` calls `sdk.Api`; rules keep `main`, `-dontoptimize`.

The stage-2 dex has `const-string "EUROPE" … sput-object La/a;.a`. The stage-2 mapping, which
is the oracle, says the original field name is **`a`**, not `EUROPE`. 8R's S label fails the
S-exactness gate on essentially every real app (T6), because Enum names are stored as data and
ProGuard/R8 rename enum fields freely.

Verdict: S is justified only if DESIGN adopts an explicit axiom, "R8's input bytecode came from
javac/kotlinc, not from a prior obfuscator", and the T6 oracle excludes pre-obfuscated
libraries. Otherwise use D-hint. This same axiom underlies every string-evidence S
(`toString`, intrinsics).

### 3.21 kotlinx.serialization, log tags, SigDB (D hint): CONFIRMED

### 3.22 A design-level issue: identity is not α-invariant

§0.3 defines D as "output invariant under any consistent renaming of minified identifiers".
§0.1 and §1 say "D·id / identity is trivially D". They contradict each other: identity is
α-*equivariant*. Renaming the input renames the output, so `8R(O') ≠ 8R(O)` byte-wise, and the
§6.4 property test fails.

`core/identity` is the fallback for `r8/kept-name`, and it is currently the only other
registered rule. It will fail the α-invariance harness on every minified name unless naming
(§5.5 structural names) always replaces minified identifiers. Either make the §5.5 rename
mandatory for every non-S name, or restate identity's contract as equivariance and exempt it
from the harness.

---

## 4. Additional rules that are sound without a mapping

### 4.1 `r8/library-override-name`: S (source-verified; E3 observed)

A non-private, non-static virtual method that overrides or implements a **library** method
with the same name and proto keeps its name. This includes short names like `run`, `get`,
`add` and `of`, which `r8/kept-name` must refuse.

Source:
- `naming/MethodNameMinifier` class comment: reservations from library classes are pulled to
  the "frontier" and program methods below it cannot take them.
- `MinifierMemberNamingStrategy.next` returns `method.getName()` for non-program methods.

Observed in E3 (obfuscated): `apply` on the synthetic `Function` lambda was kept.

Soundness: R8 cannot rename an override (that would break dispatch), and it cannot give a
non-override a library signature (the reservation forbids it). Preconditions:
- Use a library at or below the app's **targetSdk** (from the manifest), which is at or below
  the compileSdk that R8 saw. An 8R android.jar newer than R8's library could otherwise make
  a coincidental minified name look like an override.
- The class is not a member of `R8$$…`/`j$` rewritten types. For desugared-library types,
  map `j$` back to `java` first.
- The *owner* is not claimed, only the name.

### 4.2 `r8/annotation-member-name`: S (E16, verified)

`Minifier.getReservedName` returns the original name for every method whose holder
`isAnnotation()`.

E16:
```java
@Retention(RUNTIME) @interface Route { String path(); int id() default 0; }
@Route(path="/home", id=7) static class Home { … }
```
Rules: keep `main` plus `-keepattributes RuntimeVisibleAnnotations`.

Output: class `q.Main$Route -> a.b` (renamed), elements `id` and `path` unchanged, including
the 2-letter `id` that kept-name would refuse. Soundness: annotation elements are
reflectively bound by name.

Precondition: holder has `ACC_ANNOTATION`. The build-level `-applymapping` exclusion still
applies.

### 4.3 `r8/jni-symbol`: S (argued, not run: no NDK in this environment)

For each `native` method in the dex, if a bundled `lib/*/*.so` **exports**
`Java_<mangled pkg>_<mangled class>_<mangled method>` (and `__<mangled sig>` for overloads)
that decodes to exactly this dex class and method, then package, class and method name are
S.

Soundness: the native library was compiled against the original names and R8 never rewrites
`.so` files. For the dex name to equal the symbol name without being original, R8 would have
to rename `X` to exactly the name the native code independently exports. A minified name
colliding with a real exported symbol for the same class is not something R8 can produce: it
would have to be a *different* native method of the same class that R8 renamed onto an
exported name, which breaks the app.

Methods registered through `RegisterNatives` give no evidence.

Note that DESIGN §5.1's "JNI native methods are kept names" is **not** sound on its own.
Without the standard `-keepclasseswithmembernames class * { native <methods>; }` rule, R8
renames native methods. Only the symbol cross-check proves it.

### 4.4 `r8/manifest-component`: S for package + class (argued)

Components named in `AndroidManifest.xml` (activity, service, receiver, provider,
application, instrumentation, and `android:name` of backup agents) get AGP/aapt2
`-keep class X { <init>(); }` rules, with no `allowrepackage` or `allowobfuscation`. R8 does
not rewrite the manifest in the standard pipeline.

If the dex contains a class whose descriptor equals the manifest-resolved name, package and
class are S. Guard: require the name to also pass the fixed minified-shape test, in case some
future pipeline lets R8 rewrite the manifest consistently.

This is the only package-level S source found in this audit, apart from JNI.

### 4.5 `r8/sourcefile-compat`: SPLIT (candidate; verify further before registering)

Source: `SourceFileRewriter.computeCompatProvider`. When the marker says `r8-mode:"compat"`,
with `-keepattributes SourceFile` and no `-renamesourcefileattribute`, per-class `SourceFile`
values are the originals.

Detection: at least 2 distinct values across non-synthetic classes, none equal to
`SourceFile` or `r8-map-id-*`. A single app-wide constant means a rename.

Exclusions:
- synthetic classes (they inherit the context's value);
- classes that are known merge targets (the value belongs to the target only).

---

## 5. Recommended changes to DESIGN §1 and the registry (no-mapping column)

| Row | Change to |
|---|---|
| Class/member renaming | S only via §4.1, §4.2, §4.3, §4.4 and the fixed kept-name. Everything else D. |
| Backports | S if the template is unique for the marker version and the body matches exactly. N {Math, StrictMath} / {Byte, Char, Short} otherwise. D without marker. |
| API-model outlines | S for the call target only, with the synthetic-holder precondition. Body and signature D. |
| Outlining | D, done: `r8/outline-inline` (classic) and `r8/bu-outline-inline` (throw), detection by holder `ACC_SYNTHETIC` + body shape, not holder abstractness (§3.7). |
| `$-CC` | S (default) only with abstract declaration + forwarders. Otherwise N {default, static} for receiver-typed methods, and D owner for static methods without receiver. |
| Nest bridges | S only with STATIC\|BRIDGE\|SYNTHETIC. Otherwise D. |
| Horizontal merging | D when a dispatching classId exists. D·id otherwise. Document that the partition is often invisible and interfaces/fields are unioned. |
| Enum unboxing | Class D. Constant names: hint. |
| Enum `<clinit>` names | S only under an explicit "input not pre-obfuscated" axiom. Otherwise hint. |
| Kotlin intrinsics | hint |
| Records `toString` | remove (strings are post-minification) |
| Kotlin data-class `toString` | hint |
| Generics "S if kept" | D (types inside are residual names) |
| Access modification fixture | drop "no wider than original" |
| Source file | note full-mode default is `SourceFile`; add §4.5 candidate |
| `r8/kept-name` | fix the regex (digits), exclude synthetic/`$r8$`/`$$`/`name$N`, stop deriving Package S, add a dictionary/applymapping detector or downgrade to D; fix the unit test |
| `core/identity` | resolve the α-invariance contradiction (§3.22) |
