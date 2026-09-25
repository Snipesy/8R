# Source: kotlinx.serialization compiler plugin and runtime (with notes on Moshi and Gson)

Status: research notes. Input for `crates/eightr-rules` under the source id `kxs`.
Contract: DESIGN.md §0 (S / D / N, α-invariance) and §1 (rulebook). The input is **no mapping**
throughout.

Undo order: R8 is undone first (DESIGN §1). The `kxs` rules then run on the partly restored
program, before kotlinc-lowering rules. They consume the S/D labels that the R8 phase produced.
Wherever a `kxs` rule needs "this call is `PluginGeneratedSerialDescriptor.addElement`", it
depends on the runtime anchor rule `kxs/anchor-runtime` (§3.1).

## Provenance

| Artifact | Revision |
|---|---|
| JetBrains/kotlin, `plugins/kotlinx-serialization` (sparse, depth 1) | `06003680c56d09dffcf82b3817372c5aaea66b50` (2026-09-25) |
| Kotlin/kotlinx.serialization (depth 1) | `397bb560096fcb7b2a9363741690cca3d28124ba` (2026-09-04, `1.12.0-SNAPSHOT`) |
| square/moshi (depth 1) | `889013ec2edb8d8034902662a1dc8c4f3b3f8111` (2026-06-18) |
| google/gson `gson/src/main/resources/META-INF/proguard/gson.pro` | main @ `854c8255b6` (2026-09-16) |
| Experiment toolchain | kotlinc **2.4.20** (bundled `kotlinx-serialization-compiler-plugin.jar`), runtime `kotlinx-serialization-core-jvm:1.7.3`, **R8 8.10.9-dev** (`build-tools/36.0.0/lib/d8.jar`), `--min-api 24`, both full mode and `--pg-compat` |

Plugin paths below are relative to `plugins/kotlinx-serialization/`, and runtime paths are
relative to the kotlinx.serialization repo root. The experiment sources and outputs live in the
agent scratchpad (`…/scratchpad/agents/kxs/sample/`: `src/Model.kt`, `src/Main.kt`, `build.sh`,
`r8full/`, `r8compat/`). They're throwaway, but `build.sh` reproduces them.

Legend: **[V]** = verified by reading the source at the revision above *and/or* by
compiling, running R8 and dumping (javap / dexdump). **[I]** = inferred (from memory of older
versions, or reasoning not yet tested). Every [I] item should become a fixture before a rule
depends on it.

---

## 1. Transformations catalog

The generators live in `kotlinx-serialization.backend/src/org/jetbrains/kotlinx/serialization/compiler/backend/ir/`,
with names in `kotlinx-serialization.common/src/.../compiler/resolve/NamingConventions.kt`
(`SerialEntityNames`, `CallingConventions`, `findStandardKotlinTypeSerializerName`).

### 1.1 Which properties are "serializable", and in what order [V]

`IrSerializableProperties.kt: serializablePropertiesForIrBackend`
- Candidates are `irClass.properties`, keeping only those that are not fake-override, not
  delegated, not `DELEGATED_MEMBER`, and non-static with a backing field. Then drop `@Transient`.
  A property without a backing field is also treated as transient.
- The list is **partitioned into primary-constructor properties and body properties, preserving
  declaration order**.
- If the superclass also has generated methods, the final order is
  `super.serializableProperties + primaryCtorProps + bodyProps`, recursively.
- `name = @SerialName.value ?: kotlinName` (`IrSerializableProperty.name`).
  `originalDescriptorName = kotlinName`.
- `optional = declaresDefaultValue && !@Required`. `declaresDefaultValue` means either a
  field initializer that isn't just "initialize from the ctor parameter", or a ctor parameter
  with a default.
- `goldenMask` / `goldenMaskList` (`common/.../ISerializableProperties.kt`) set bit `i%32` of
  slot `i/32` iff property `i` is **not** optional.
- Slot count is `bitMaskSlotCount() = ceil(n/32)`.

### 1.2 The `$serializer` class (`SerializerIrGenerator`) [V]

For a non-generic `@Serializable class/data class/value class` `C`, the plugin nests
`C$$serializer`. Its binary simple name `$serializer` is `SerialEntityNames.SERIALIZER_CLASS`.
It is an `object` that implements `kotlinx.serialization.internal.GeneratedSerializer<C>`.

Observed with 2.4.20 (javap of `User$$serializer`):

```
public static final C$$serializer INSTANCE;
private static final SerialDescriptor descriptor;            // property "descriptor"
private <init>()
public final void serialize(Encoder, C)                      // + bridge serialize(Encoder,Object)
public final C deserialize(Decoder)                          // + bridge deserialize(Decoder):Object
public final SerialDescriptor getDescriptor()
public final KSerializer<?>[] childSerializers()
public KSerializer<?>[] typeParametersSerializers()          // bridge to GeneratedSerializer$DefaultImpls when no type params
static <clinit>:
   INSTANCE = new C$$serializer()
   d = new PluginGeneratedSerialDescriptor("<class serialName>", INSTANCE /*GeneratedSerializer*/, <n>)
   d.addElement("<elem0>", <optional0>)   ; then pushAnnotation(<SerialInfo impl>) per SerialInfo annotation on that property
   ...
   d.addElement("<elem n-1>", <optional n-1>)
   d.pushClassAnnotation(...)            ; per class-level SerialInfo annotation
   descriptor = d
class annotations: @kotlin.Deprecated(message="This synthesized declaration should not be used directly", level=HIDDEN)
```

- The descriptor constructor's third argument is `serializableProperties.size`. The second
  argument is `this` only when the serializer is plugin-generated (`isGeneratedSerializer`). An
  **external** serializer (`@Serializer(forClass=…) object Foo`) passes `null`
  (`SerializerIrGenerator.instantiateNewDescriptor`) [V source]. This is how to tell a
  `$serializer` from a user-named external serializer.
- `addElement(name, isOptional)` is emitted in property order, skipping transient properties.
  It's followed by `pushAnnotation` for each `@SerialInfo`-meta-annotated property annotation
  (`copySerialInfoAnnotationsToDescriptor`), which instantiates `<Annotation>$Impl` classes
  generated by `SerialInfoImplJvmIrGenerator` (the nested class is named `Impl`).
  `pushClassAnnotation` handles class-level SerialInfo annotations.
- **Generic classes** (`Box<T>`) [V]: `$serializer` is a class, not an object. It has instance
  fields `descriptor` and `typeSerial0..k` (`typeArgPrefix = "typeSerial"`) and a ctor
  `(KSerializer<T0>, …)`. `typeParametersSerializers()` returns the `typeSerial*` fields.
  `Companion.serializer(KSerializer…)` does `new C$$serializer(ts…)`. The serializable class
  additionally gets a static `$cachedDescriptor` field (`CACHED_DESCRIPTOR_FIELD_NAME`). It holds
  a **second copy** of the same `PluginGeneratedSerialDescriptor(name, null, n)` + `addElement`
  sequence, which the deserialization constructor uses for its mask check
  (`SerializableIrGenerator.createCachedDescriptorProperty`).
- **Value classes** [V]: `SerializerForInlineClassGenerator` uses
  `InlineClassDescriptor(serialName, this)` + `addElement("<prop>", false)`. `serialize` calls
  `encoder.encodeInline(desc)` then `encodeXxx`. `deserialize` calls
  `decodeInline(desc).decodeXxx()`. With JVM inline-class mangling the method names become
  `serialize-<hash>` / `deserialize-<hash>`, observed as `serialize-5UDShJU` and
  `deserialize-CyM9JdY`.

### 1.3 `childSerializers()` and the `$childSerializers` cache [V]

`childSerializers()` returns `KSerializer[n]` in property order (`generateChildSerializersGetter`).
- Primitives and `String` map to `XSerializer.INSTANCE`.
- Nullable types are wrapped in `BuiltinSerializersKt.getNullable(ser)` (`wrapIntoNullableCallableId`).
- Collections map to `ArrayListSerializer` / `LinkedHashSetSerializer` / `HashSetSerializer` /
  `LinkedHashMapSerializer` / `HashMapSerializer` per `findStandardKotlinTypeSerializerName`.
- Enums map to `<Enum>.Companion.serializer()` (or the legacy enum `$serializer`).
- User classes map to `<C>$$serializer.INSTANCE`, or `Companion.serializer(...)` when generic.

Serializers that are **not objects** and contain no type parameters are cached. They live in a
static field `$childSerializers` on the *serializable class* (`CACHED_CHILD_SERIALIZERS_PROPERTY_NAME`,
`BaseIrGenerator.addCachedChildSerializersProperty`), which is declared first. Cached slots hold
`kotlin.Lazy` (`LazyKt.lazy(PUBLICATION, () -> …)`), with `null` for uncached slots.
- The lambdas are compiled as `_childSerializers$_anonymous_`, `…$0`, … [V javap].
- `serialize`, `deserialize` and `childSerializers` read the cache through the accessor
  `access$get$childSerializers$cp()`.
- Version history (git log via GitHub API):
  - Child-serializer caching was added in `339b152390` (2022-11-23, "Implemented caching of
    child serializers"), so Kotlin ≥ 1.8.20 [I for the release mapping].
  - Lazy wrapping was added in `b35161e241` (2024-12-16, "Wrapped each child serializer into
    lazy delegate"). Before that the array was `KSerializer[]` [I shape].

### 1.4 `serialize` and `write$Self` [V]

`SerializerIrGenerator.generateSave`:
```
desc   = this.descriptor
output = encoder.beginStructure(desc)
C.write$Self[$module](value, output, desc [, typeSerial0…])   // if C has write$Self
output.endStructure(desc)
```
If the class has no `write$Self` (only external serializers), the property loop is inlined
into `serialize` directly.

`write$Self` (`IrPreGenerator.preGenerateWriteSelfMethodIfNeeded` +
`SerializableIrGenerator.generateWriteSelfMethod`) is JVM only and `@JvmStatic`, with parameters
`(self, output, serialDesc, typeSerial0…)`.
- **Visibility is `internal` if the class is `final`, `public` otherwise**
  (commit `a03fd2e29a`, 2023-12-12, "Make visibilities of writeSelf and deserialization
  constructor in FIR").
- JVM internal-name mangling therefore produces **`write$Self$<moduleName>`** for final classes.
  Observed: `write$Self$samplemod` with `-module-name samplemod`. Android Gradle module names look
  like `app_release` [I].
- Open, abstract and sealed classes get plain `write$Self` [V: `Shape.write$Self`, `Base.write$Self`].
- A subclass's `write$Self` first calls `Super.write$Self(self, output, desc, …)`, then emits
  only its own indices [V: `Derived` → `Base.write$Self`].
- K1-era output had plain `write$Self` for all classes [I].

Per property `i` (`BaseIrGenerator.serializeAllProperties`), it emits
`output.encode<Kind>Element(desc, i, self.f)` or
`output.encode[Nullable]SerializableElement(desc, i, ser, self.f)`.
- **Required** properties, or those with `@EncodeDefault(ALWAYS)`: the call is unconditional.
- **Optional** properties (default mode): the call is wrapped in
  `if (output.shouldEncodeElementDefault(desc, i) || self.f != <default expr>)`.
- `@EncodeDefault(NEVER)`: the wrapper is `if (self.f != <default>)`, with no
  `shouldEncodeElementDefault` call.
- Since `269617e2e0` (2026-07-15), array-typed defaults are compared with
  `contentEquals`/`contentDeepEquals` instead of `!=`.
- The default expression is the property initializer, re-targeted to `self` (`initializerAdapter`).
- Special case: properties from another module whose initializer isn't available are always
  encoded (`field == null || initializer == null`).

### 1.5 `deserialize` [V]

`SerializerIrGenerator.generateLoad`. Abstract and sealed classes get an empty body. Otherwise
it declares these locals: `desc`, `flag=true`, `index=0`, `bitMask0..k = 0`,
`local0..n-1 = 0/null`, and `transient*`, the last only used by external serializers.

```
input = decoder.beginStructure(desc)
cached = C.access$get$childSerializers$cp()           // if cache exists
if (input.decodeSequentially()) {
    local_i = input.decode<Kind>Element(desc, i) | decode[Nullable]SerializableElement(desc, i, ser, local_i)
    bitMask[i/32] |= 1 << (i%32)                      // for every i in order
} else while (flag) {
    index = input.decodeElementIndex(desc)
    when (index) { -1 -> flag = false ; i -> { local_i = …; bitMask[i/32] |= 1<<(i%32) } ;
                   else -> throw UnknownFieldException(index) }
}
input.endStructure(desc)
return new C(bitMask0..k, local0..n-1, null /*SerializationConstructorMarker*/)
```
The `when` compiles to a `tableswitch`/`packed-switch` over `-1..n-1`. The masks are OR-ed with
the constants `1<<(i%32)`. External serializers (no synthetic ctor) instead call
`generateGoldenMaskCheck` in place, followed by the primary ctor and setters for body `var`s.

### 1.6 Synthetic deserialization constructor [V]

`IrPreGenerator.preGenerateDeserializationConstructorIfNeeded` +
`SerializableIrGenerator.generateInternalConstructor`
- Signature: `(int seen0, …, int seenK, <props in order, non-primitive types made nullable>,
  SerializationConstructorMarker serializationConstructorMarker)`.
- Visibility is `internal` when final and `public` otherwise. No JVM name mangling applies to
  constructors. Flags are `ACC_SYNTHETIC`.
- Parameter names use the Kotlin name, not the serial name, since `c8b6e57c4d` (2022-12-21).
  This doesn't matter after R8, which drops parameter names.
- Body:
  1. Golden mask check, only if the class isn't abstract or sealed (`generateGoldenMaskCheck`):
     - One slot: `if (G != (G & seen0)) throwMissingFieldException(seen0, G, <desc>)`.
     - Several slots: `if (G0 != (G0&seen0) | G1 != (G1&seen1) …)
       throwArrayMissingFieldException(int[]{seen…}, int[]{G…}, desc)`.
     - `<desc>` is `C$$serializer.INSTANCE.getDescriptor()` for non-generic classes, and
       `Companion.$cachedDescriptor` for generic ones.
  2. Super call:
     - `Object.<init>()`.
     - If the super is serializable: `super(<its seen slots>, <its props>, marker)`. The prefix
       of this ctor's props goes to the super.
     - If the super isn't serializable: its no-arg ctor.
  3. Replay of *non-serializable* field initializers and `init {}` blocks in declaration order,
     interleaved after the serializable property they follow. This is how `@Transient val x = "x"`
     shows up as an unconditional `this.cache = "x"` [V].
  4. For each own property `i`:
     - Required: `this.f = p_i`.
     - Optional: `if ((seen[i/32] & (1<<(i%32))) == 0) this.f = <initializer> else this.f = p_i`.
  5. Interface-delegation fields (`$$delegate_n`) are re-initialized.

Observed for `User` (7 properties, required = {id, name, flag}): `G = 19 = 0b10011` [V].
Observed for `Big` (34 properties): `G0 = 0xBDEF7BDE`, `G1 = 3`, with `throwArrayMissingFieldException` [V].

### 1.7 Companion / object / enum / sealed / polymorphic [V]

`SerializableCompanionIrGenerator`, `Instantiator`
- **Companion `serializer()`**
  - Non-generic class: `Companion.serializer()` returns `C$$serializer.INSTANCE`.
  - Generic class: `serializer(KSerializer<T0>…)` returns `new C$$serializer(…)`.
  - Objects, sealed/abstract classes and enums instead get a lazily cached serializer. There is
    a static `$cachedSerializer$delegate : Lazy` on the class, the companion has a private
    `get$cachedSerializer()`, and the lambda is `_init_$_anonymous_`
    (`shouldHaveSerializerCache`).
  - `@KeepGeneratedSerializer` adds `generatedSerializer()` and `$cachedKeepSerializer`.
  - `Companion.serializer(vararg KSerializer<*>)` (the `SerializerFactory` interface) is emitted
    only on non-JVM targets.
- **Named companion** (`companion object Factory`): the companion gets
  `@kotlinx.serialization.internal.NamedCompanion` (`patchNamedCompanionWithMarkerAnnotation`),
  and the outer class has `static final C$Factory Factory` [V].
- **`@Serializable object O`**: `ObjectSerializer("<serialName>", O.INSTANCE, Annotation[]{class SerialInfo annotations})`.
  `O.serializer()` is on the object itself.
- **Enums**
  - With runtime ≥ the version that has `createSimpleEnumSerializer`/`createAnnotatedEnumSerializer`
    in `kotlinx.serialization.internal` (`DependencySerializationInfoProvider.useGeneratedEnumSerializer`):
    - `createSimpleEnumSerializer("<serialName>", values())` if no enum entry or class has
      `@SerialName`/SerialInfo annotations.
    - Otherwise `createAnnotatedEnumSerializer("<serialName>", values(),
      String?[]{entry @SerialName or null}, Annotation[][]{entry SerialInfo}, Annotation[]? class)`
      [V: `{null,"verde",null}`].
  - Legacy path (older runtimes): a nested `$serializer` object with
    `EnumDescriptor(serialName, n)`, `addElement(entry.serialName ?: entry.name, false)` per
    entry, `encodeEnum(desc, ordinal)`, and `values()[decodeEnum(desc)]`
    (`SerializerForEnumsGenerator`).
  - Older still: `new EnumSerializer(serialName, values())` [I].
- **Sealed / abstract**
  - Sealed: `new SealedClassSerializer("<serialName>", KClass(base), KClass[] subclasses,
    KSerializer[] subclassSerializers, Annotation[] classAnnotations)`
    (`Instantiator.instantiateSealedSerializer`). Subclasses are listed in
    `getSealedSubclasses` order.
  - Abstract classes and interfaces get `new PolymorphicSerializer(KClass(base), Annotation[])`.
- **Contextual**: `new ContextualSerializer(KClass, fallback?, KSerializer[] typeArgs)`.
  `@Serializable(with=X::class)` is `X.INSTANCE` or `new X(...)`.
- **Marker annotation**: `patchSerializableClassWithMarkerAnnotation` adds
  `@SerializableWith(serializer=C$$serializer::class)` to the IR of classes whose serializer is
  an object. **With 2.4.20 it doesn't appear in the class file** [V: javap shows only
  `@Serializable`, plus `@SerialName` if present]. So don't rely on it.

### 1.8 `@SerialName`, `@Transient`, `@Required`, `@EncodeDefault` [V]

| Annotation | Effect in bytecode |
|---|---|
| `@SerialName(s)` on a class | `serialName() = s`, else `fqNameWhenAvailable` with **dots for nesting** (`com.example.model.Shape.Rect`) (`IrPredicates.kt: IrClass.serialName`). Also stays as a RUNTIME class annotation in the `.class` [V]. |
| `@SerialName(s)` on a property | Element name `s`. The annotation lives on the synthetic `getX$annotations()` method (RUNTIME retention) [V]. |
| `@SerialName(s)` on an enum entry | The `names[]` array slot (factory path) or the `addElement` name (legacy path). |
| `@Transient` | Absent from descriptor, `childSerializers`, `write$Self` and `deserialize`. Its initializer is replayed unconditionally in the synthetic ctor. Needs a default (enforced by the checker). |
| `@Required` | `isOptional = false` even with a default. The bit is in the golden mask. The synthetic ctor assigns the parameter unconditionally, so the default **isn't** there. The default only lives in the primary ctor / `$default` ctor. |
| `@EncodeDefault(ALWAYS / NEVER)` | See §1.4. `mode` defaults to ALWAYS. |

### 1.9 Version differences (summary)

| Change | When | Evidence |
|---|---|---|
| Runtime floor is 1.3.0; `throwMissingFieldException`/`throwArrayMissingFieldException` golden-mask path mandatory | `a2821aa738` 2025-11-19 | [V] |
| Pre-golden-mask synthetic ctor threw `MissingFieldException("<serialName>")` per field; very old `$serializer` held `$$serialDesc` and used `SerialClassDescImpl` | ≤ plugin 1.3.x / 0.x | [I] |
| Child-serializer cache `$childSerializers : KSerializer[]` | `339b152390` 2022-11-23 | [V commit], [I shape] |
| `$childSerializers : Lazy[]` | `b35161e241` 2024-12-16 | [V] (2.4.20 output) |
| Ctor param names use Kotlin names | `c8b6e57c4d` 2022-12-21 | [V] |
| SerialInfo annotations on enum class | `5e01669e23` 2022-12-18 | [V commit] |
| `write$Self` internal for final classes, hence `$module` suffix (K2) | `a03fd2e29a` 2023-12-12 | [V] |
| `@KeepGeneratedSerializer` (`generatedSerializer()`, `$cachedKeepSerializer`) | `da0069d909` 2024-04-29 | [V commit] |
| Array defaults compared by content | `269617e2e0` 2026-07-15 | [V commit] |
| Legacy non-IR JVM codegen (`SerializableCodegenImpl` etc.) | removed with the old JVM backend | [I] |
| Bundled R8/ProGuard rules in the runtime jar | runtime 1.5.0, `ea69eb376c` 2022-12-02 | [V commit] |
| R8 rules for `NamedCompanion`, object `INSTANCE`/`serializer()`, `$$serializer.descriptor` | `2e5c66ee76` 2024-06, `1b0accd69f` 2024-11, `4667a1891a` 2025-04, `51fa6ad8dc` 2025-08 | [V] |

---

## 2. What survives R8

### 2.1 Runtime consumer rules [V]

Source: `rules/common.pro` and `rules/r8.pro`. They're packed by `core/build.gradle.kts` into
`META-INF/proguard/`, `META-INF/com.android.tools/proguard/` and
`META-INF/com.android.tools/r8/` (the last one also gets `r8.pro`). HEAD content:

| Rule | Effect without a mapping |
|---|---|
| `-keepclassmembers @Serializable class ** { static ** Companion; }` | The **field name `Companion`** survives. It's uninformative as a name, but it's a strong anchor that the class is `@Serializable` with a companion. |
| `-if @NamedCompanion class * -keepclassmembers class * { static <1> *; }` | The **field that holds a named companion keeps its name**. Kotlin names that field after the companion object, so this is the companion's **original simple name** [V: `Factory -> Factory`]. |
| `-if @Serializable class ** { static **$* *; } -keepclassmembers class <2>$<3> { KSerializer serializer(...); }` | Companion `serializer(...)` keeps its name. |
| `-keepclassmembers @Serializable class ** { public static ** INSTANCE; KSerializer serializer(...); }` (r8.pro) and the `-if … INSTANCE` variant (common.pro) | Serializable objects keep `INSTANCE` and `serializer` [V]. |
| `-keepattributes RuntimeVisibleAnnotations,AnnotationDefault` | Class annotations *can* survive. |
| `-if @Serializable class ** -keep,allowshrinking,allowoptimization,allowobfuscation,allowaccessmodification class <1>` (r8.pro) | Makes R8 full mode keep class annotations on serializable classes. |
| `-keepclassmembers public class **$$serializer { private ** descriptor; }` | The **`descriptor` field name** in every `$serializer` survives [V]. |
| (none) | **No rule keeps any class name, any property/field name, or the runtime's own names.** Everything is obfuscated, including `kotlinx.serialization.*` [V: `PluginGeneratedSerialDescriptor -> q.V`, `addElement -> g`, `Serializable -> n.h`]. |

The 1.7.3 jar ships an older `common.pro` (a `Companion` rule via `-if`, no NamedCompanion rule)
and an older `r8.pro` without the `INSTANCE`/`serializer` rule [V: diff against the jar]. With
**runtime < 1.5.0** there were no bundled rules. Apps then copied the README rules, typically
`-keep,includedescriptorclasses class <pkg>.**$$serializer { *; }`,
`-keepclassmembers class <pkg>.** { *** Companion; }` and
`-keepclasseswithmembers class <pkg>.** { kotlinx.serialization.KSerializer serializer(...); }`
[I from memory]. Those keep **class names** of `$serializer` and companion-bearing classes,
which makes names identity-S (unobfuscated). 8R should detect "name not minified" upstream
(DESIGN §5) and not need `kxs` for it.

### 2.2 Observed R8 output (sample `Model.kt`, R8 8.10.9, full and compat) [V]

| Artifact | Survives? |
|---|---|
| `const-string` serialName in `$serializer.<clinit>` (`"com.example.model.User"`), in `SealedClassSerializer`/`ObjectSerializer`/enum-factory args, and in `$cachedDescriptor` | **Yes**, all. |
| `addElement` sequence (`const-string name; const/4 optional; invoke-virtual Lq/V;.g(String,Z)V`) | **Yes, intact and in order**, with the method renamed. |
| `PluginGeneratedSerialDescriptor.<init>(String, GeneratedSerializer, int)` | Yes (renamed). The second argument is the `INSTANCE` just created. |
| Static field `descriptor` in `$serializer` | Yes, **name kept**. `INSTANCE` renamed. |
| `$serializer` / companion / serializable class names | Renamed (`b.K`, `b.L`, `b.M`). The `Companion` / `Factory` / `INSTANCE` field names are kept. |
| `write$Self$samplemod` | **Inlined into `$serializer.serialize`**. The module name doesn't appear anywhere in the dex [V: 0 hits]. |
| `encode*Element(desc, i, …)` in serialize | In the closed-world sample R8 **devirtualized and inlined** these to `AbstractEncoder.encodeInt` etc., **dropping the index constant**, and folded `shouldEncodeElementDefault` [V]. With `kotlinx-serialization-json` there are several `CompositeEncoder` implementations, so the interface calls should survive [I]. Rules must not require the index constant here. |
| `deserialize` loop | **Yes**: `packed-switch` over `-1..n-1`, `or-int/lit8 vMask, vMask, #1<<i`, `new-instance UnknownFieldException(index)` (renamed), `decodeSequentially` branch. If the decoder's `decodeSequentially()` is constant, R8 drops one branch. In the first sample run (always `true`) it folded the whole mask logic away, so the fixture must use a non-constant decoder. |
| Synthetic ctor | **Yes**: `and-int/lit8 v, seen0, #19; if-ne 19,…` then `throwMissingFieldException(seen, G, $serializer.INSTANCE.getDescriptor())` (renamed), then the per-bit `if-nez … iput`. The **`SerializationConstructorMarker` parameter was removed** (unused-argument removal), and the marker class is gone. |
| `$childSerializers` | Field renamed. The `Lazy` lambdas became `$$ExternalSyntheticLambda0` classes that switch on an int. |
| `@Serializable` class annotation | **Only if the runtime's reflective lookup is live** (for example `serializer(java.lang.reflect.Type)`). When it's live the annotation type is renamed (`Ln/h;`) but its element name `with()` is kept [V]. When nothing reads it reflectively, R8 removes the annotation class and every use in both modes [V]. |
| Class-level `@SerialName("acct")`, property `getX$annotations()`, `@Deprecated` on `$serializer`, `kotlin.Metadata` | **Removed** [V]. `@SerialName`'s class isn't live, the `$annotations` methods are unused, and `kotlin.Metadata` isn't kept by these rules. |
| Data-class `toString` strings (`"User(id="`, `", name="`, …) | **Yes** [V]. |
| Primary-ctor `Intrinsics.checkNotNullParameter(p, "name")` | Removed in this sample because the ctor was specialized with constant args and the checks were folded. It generally survives unless the app strips intrinsics [I, see DESIGN §1 "Kotlin intrinsics parameter names"]. |
| Enum `<clinit>` names `"RED"`, `"GREEN"`, …; enum serial-name array | **Yes**. `createAnnotatedEnumSerializer` was inlined, but `filled-new-array {null,"verde",null}` remains [V]. |
| Runtime message templates (`"An unknown field for index "`, `"' is required for type with serial name '"`, `" are required for type with serial name '"`, `"[UNINITIALIZED]"`, `"Serializer for subclass '"`, `"kotlinx.serialization.Polymorphic"`, …) | **Yes** [V]. These are the anchors for identifying the renamed runtime classes. |
| `SealedClassSerializer("com.example.model.Shape", …)` args | Yes. `const-class` subclass literals are renamed. |

### 2.3 Fingerprints that remain with no mapping

1. **`$serializer` shape**:
   - An `<clinit>` that makes a new-instance of itself into a static field, then
     `new D(const-string, <that instance>, const-int n)` followed by exactly `n` calls
     `D.m(const-string, const-bool)`, storing into a static field **named `descriptor`**.
   - It implements an interface with 2 abstract `()[KSerializer]` methods (`GeneratedSerializer`).
   - It has a `deserialize` whose body ends in `new-instance C; invoke-direct C.<init>(I…, …)`
     with a leading `int` block.
2. **Seen-bitmask arithmetic**:
   - In `deserialize`: `or-int/lit8|lit16 mask, mask, 2^k` after each decode, plus a switch on
     `-1..n-1`.
   - In the synthetic ctor: `and-int mask, G; if-ne G, …; invoke-static T(seen, G, desc)` with
     `T` = throwMissingFieldException. For >32 properties: `new-array int[]`… `T'(int[], int[], desc)`.
   - Optional fields: `and-int/lit seen, 2^k; if-nez → iput param; else iput <default>`.
3. **`write$Self` remnant**: a block between `Encoder.beginStructure(desc)` and
   `CompositeEncoder.endStructure(desc)` (both renamed), reading each serializable field once,
   in index order.
4. **Runtime anchors**: exact message strings (§2.2) plus method shapes identify
   `PluginGeneratedSerialDescriptor`, `PluginExceptionsKt`, `MissingFieldException`,
   `UnknownFieldException`, `SealedClassSerializer`, `ObjectSerializer`,
   `EnumsKt`/`EnumSerializer`, `InlineClassDescriptor` and `GeneratedSerializer` without names.

---

## 3. Undo rules

All rule ids use the prefix `kxs/`.
- Precondition "anchored" means `kxs/anchor-runtime` identified the callee with an exact
  predicate. If that fails, every dependent rule falls back to D·id.
- α-invariance: every rule below keys on string constants, int constants, bit arithmetic and
  call-graph *shape*. None reads a minified name or dex position.
- Ties, such as two `$serializer`s with the same serialName (only possible with a
  `@SerialName` collision in different modules), are broken by structural hash, never by input
  order.

### 3.1 Recognition and structure

| id | Class | Preconditions | Evidence / output |
|---|---|---|---|
| `kxs/anchor-runtime` | S (as anchor) / D (as name) | Runtime class matches an exact version-keyed predicate. Example: PluginGeneratedSerialDescriptor = ctor `(String, I, int)` filling a `String[n]` with `"[UNINITIALIZED]"`, and a method `(String, boolean)V` that pre-increments an index and stores into `String[]` and `boolean[]`. | The anchor is internal knowledge used by the other rules. *Renaming* the runtime classes back to `kotlinx.serialization.*` is SigDB territory. That's S only on an exact whole-class template match for a known runtime version, D otherwise (DESIGN §5.4). |
| `kxs/serializer-class` | S (structure + simple name) | §2.3 fingerprint 1, where the descriptor ctor's second argument is the class's own instance (not `null`). The class instantiates exactly one class `C` in `deserialize`, or none for abstract. | `C$$serializer` is nested in `C`. Simple name **`$serializer`** is S: the plugin always emits that name, and only plugin-generated serializers pass `this` as `generatedSerializer` (`instantiateNewDescriptor`). The outer name follows `C`'s label. Method names `serialize`, `deserialize`, `getDescriptor`, `childSerializers`, `typeParametersSerializers` are S when the implemented interfaces are anchored. Field `INSTANCE` is S (object). `typeSerial<i>` fields are S for generic serializers, with index = ctor-parameter order. |
| `kxs/external-serializer` | D | Same fingerprint, but the second argument is `null` and the class isn't a `GeneratedSerializer`. | A user-named `@Serializer(forClass=C)` object. Its name is D (`{C-hint}Serializer_{hash}`). |
| `kxs/companion` | S | A static field of type `K` on class `C`, where `K.serializer()` returns `C$$serializer.INSTANCE` or a `Lazy`-cached serializer. The field name is `Companion` or a kept NamedCompanion name. | Companion class `C$<fieldName>` is S. The field name was kept by the consumer rules, and Kotlin names the field after the companion object [V]. The method `serializer` is S (kept). `$cachedSerializer$delegate`, `get$cachedSerializer`, `$cachedDescriptor`, `$childSerializers` and `access$get$childSerializers$cp` are S when their shapes match §1.3/§1.7 exactly: the plugin emits these fixed names. |
| `kxs/object-serializer` | S (kind) | `ObjectSerializer(const-string, X.INSTANCE, Annotation[])` built inside `X`. | `X` is a `@Serializable object`. The `INSTANCE` and `serializer` names are S (kept). |
| `kxs/sealed-serializer` | S (relation) / D (completeness) | `SealedClassSerializer(const-string, const-class B, KClass[]{const-class S_i}, KSerializer[]{…}, Annotation[])` in `B`'s lazy. | `B` is `@Serializable` sealed. Each `S_i` is a (transitive) sealed subclass, and `S_i`'s serializer is paired by index: S. The list is the *serializable* sealed subclasses, so "these are all subclasses" is D [I: non-serializable subclasses]. |
| `kxs/polymorphic` | S (kind) | `PolymorphicSerializer(const-class B, Annotation[])` as the companion serializer. | `B` is an abstract `@Serializable` class or interface. |
| `kxs/synthetic-ctor-marker` | S | A ctor on `C` with the §2.3 fingerprint-2 shape, called only from `C$$serializer.deserialize` with the leading int masks. | Restore the trailing `SerializationConstructorMarker` parameter and set `ACC_SYNTHETIC`. The call site passes `null`. **S**, because the plugin *always* passes `irNull()` (`generateLoad`), so the restored argument equals the original. This is the one case where unused-argument restoration is exact. The marker class name is S via the anchor. |
| `kxs/write-self` | D (body extraction) + S/D name | The serialize block of §2.3 fingerprint 3. | Outline it back into `static C.write$Self…(C, CompositeEncoder, SerialDescriptor[, KSerializer…])`. The body extraction is un-inlining, so it's **D** (DESIGN §1 "Inlining"). The name is **S = `write$Self`** iff `C` is provably non-final: `C` is abstract/sealed per `kxs/sealed-serializer`/`kxs/polymorphic`, or another serializable class calls into it as a super. Otherwise the name is `write$Self$<module>` with the module unknown, which is **D** (`write$Self$m_{hash}`). ✗N: guessing `app_release`. |
| `kxs/golden-mask` | S | Synthetic ctor mask check anchored to throwMissing*. | `G` → per-index required flag. Cross-checked against the `addElement` optional flags (`G` bit `i` == `!optional_i`). If they disagree, reject the whole class for S. |

### 3.2 Element table, binding, order, optionality, defaults

| id | Class | Preconditions | Evidence / output |
|---|---|---|---|
| `kxs/element-table` | S | `kxs/serializer-class`. | `(index i, serialName_i, optional_i)` from the `addElement` sequence. These are facts about the *descriptor*, not about Kotlin names. Use it for annotations, the report, and as hints. |
| `kxs/element-field-binding` | S | At least one of these derivations is present unambiguously, and any others agree. (a) Synthetic ctor: bit `2^(i%32)` tested before `iput f`, which covers optional properties even if R8 dropped or reordered params. (b) Synthetic ctor param order: after the leading `ceil(n/32)` ints, param `k` → `iput f`, valid only when the param count is exactly `slots + n` after marker restoration. (c) `deserialize`: `case i:` → decode → the local passed at ctor position `slots+i`. (d) `write$Self` block: `encode*Element(desc, i, iget f)`, when the index constant survived. | Binds element `i` to field `f`, and so to the property `f` backs. Field *types* are S from the dex. |
| `kxs/property-order` | S | Binding for all own elements. | The source order of serializable properties within `C` is the element order, minus the inherited prefix. Primary-ctor properties come before body properties. The split point isn't fully determined, see §5.4. Use it to order fields and ctor params in output and to emit `data class` components. |
| `kxs/optional` | S | Element table. | `optional_i` ⇔ (declares a default ∧ ¬`@Required`). |
| `kxs/required-annotation` | S / D·id | `optional_i = false` **and** a default for that property exists elsewhere: in the primary `$default` ctor (`C(…, int mask, DefaultConstructorMarker)`) or as a body initializer in the primary ctor. | Emit `@Required` (S). Without that evidence you can't separate "no default" from "@Required + default", and the default expression can't be enumerated. So it's D·id (nothing emitted). |
| `kxs/default-value` | D | Optional element `i` with the synthetic ctor's `if ((seen&bit)==0) f = E`. | `E` is the default initializer, semantically. R8 may have rewritten it (for example `emptyList()` → `EmptyList.INSTANCE` [V]), so it's D. It's cross-checked against the `write$Self` default comparison when that survived. |
| `kxs/transient` | D | A field of `C` that isn't bound to any element, is assigned unconditionally in the synthetic ctor from a non-parameter value, and isn't a `$$delegate_n` interface-delegation field. | Mark it `@Transient` with that initializer. It's D because a delegated property (`by …`) field has the same shape, and R8 may have inlined the getter that would tell them apart. |
| `kxs/encode-default` | S / D·id | An unfolded `write$Self` shape for optional element `i`. | Unconditional encode → `@EncodeDefault` (ALWAYS). Guard without `shouldEncodeElementDefault` → `@EncodeDefault(NEVER)`. Guard with it → no annotation. All S. If R8 folded `shouldEncodeElementDefault` (single encoder impl, §2.2) → D·id. Exception: properties inherited from another module are always unconditional, so only apply this to own elements. |
| `kxs/property-type` | S / N | `childSerializers()` / decode calls for element `i`, plus the field JVM type. | Primitive `decodeIntElement` → `Int` (S). `getNullable(X)` → nullable (S). Collection serializers have finite preimages (`findStandardKotlinTypeSerializerName`): `ArrayListSerializer` + field `java.util.List` → **N {List, MutableList}**; + `java.util.Collection` → Collection (S); + `java.util.ArrayList` → ArrayList (S). `LinkedHashSetSerializer` + `Set` → N {Set, MutableSet}; + `LinkedHashSet` → S. Maps are analogous. The Kotlin and Java names on the same JVM type are one preimage after erasure. |

### 3.3 Names (the key question)

**When do serial names prove the original Kotlin names?** The serial name never proves anything
on its own. `@SerialName(s)` can set any string, including one that looks exactly like the
default. The canonical counterexample is common in real code: after a class is moved, people pin
`@SerialName("old.pkg.User")` for wire compatibility. So a serial name becomes S only through
**independent, compiler-emitted evidence bound to the same field or class**.

| id | Class | Preconditions | Evidence / output |
|---|---|---|---|
| `kxs/prop-name-proven` | **S** | Element `i` is bound to field `f` (`kxs/element-field-binding`), **and** at least one independent Kotlin-name source (§4, Tier A) names `f`. | Property name = that source's string. If it equals `serialName_i`, the output has no `@SerialName` and "no override" is **S**. If it differs, emit `@SerialName(serialName_i)`, which is **S** (the plugin's rule is exactly `name = @SerialName ?: kotlinName`). A redundant `@SerialName("x")` on a property named `x` can't be told apart from no annotation, but the two are semantically identical, so the S claim is about semantics, not source text. |
| `kxs/prop-name-override-proven` | S (fact) + D (name) | `serialName_i` isn't a legal Kotlin/JVM identifier even with backticks. On JVM, backticked names may contain spaces, `-` and `@`, but not `.`, `;`, `[`, `]`, `/`, `<`, `>`, `:`, `\`, a backtick, or a newline, and they can't be empty [I: Kotlin JVM identifier rules, needs fixture]. So `""`, `"a.b"` and `"x:y"` prove an override. **`"user-name"` and `"@type"` do not**, because `` val `user-name` `` is legal. | Emitting `@SerialName(s)` is S. The property name falls to D `{sanitized(s)}_{hash}`. |
| `kxs/prop-name-hint` | **D** | Neither of the above. | Name `{sanitized(serialName_i)}_{hash}`, per DESIGN §5.5. **Not N**: the candidate set is "serialName_i, or any identifier at all", which is infinite and can't be enumerated. The hash is structural (WL), so it's α-invariant. The serial name is a string constant, so it's α-invariant too. |
| `kxs/class-name-hint` | **D** | Class descriptor serialName `s`. | The class name is D with hint = last segment of `s` (package hint = prefix). Never S: an override can't be ruled out (see above). |
| `kxs/class-serialname-override-proven` | S (fact) | Either `s` isn't a syntactically valid dotted JVM FQN (an empty segment, or a segment containing a JVM-forbidden character). Note that `"acct"` *is* valid, as a root-package class. Or the last segment of `s` ≠ an **S** simple name of `C` from Tier-A evidence (data-class `toString` prefix, Moshi `GeneratedJsonAdapter(…)`). | Emit `@SerialName(s)` on `C` (S). The FQN stays D. |
| `kxs/class-simple-name` | S | Data-class `toString` prefix `"Name("` bound to `C` (DESIGN §1 row "Kotlin data-class `toString`"). | S simple name. This isn't a `kxs` rule, but `kxs` consumes it. If `s`'s last segment agrees, only the *package* stays D. |
| `kxs/enum-entry-serialname` | S | Enum `values()` array + `<clinit>` constant names (DESIGN §1 "Enum constant names") + the `createAnnotatedEnumSerializer` `names[]` array, or the legacy `EnumDescriptor.addElement` sequence. | Per entry: `names[k] == null` means no override (S). A non-null value is `@SerialName(names[k])` (S). In the legacy path, compare `addElement` name `k` with `<clinit>` name `k` (ordinal-aligned): equal means no override, S, modulo the redundant-annotation caveat. A `createSimpleEnumSerializer` call proves that **no** entry has `@SerialName` or SerialInfo annotations (S). |
| `kxs/companion-simple-name` | S | See `kxs/companion`. | Named-companion simple name = the kept field name (S). |
| `kxs/fqn-split` | N (enumerable) → report only; pipeline uses D | Only meaningful under an assumed non-override. `s = a.b.….z` with `k` dots. | Candidate preimages: package = the first `j` segments and nested-class chain = the rest, for `j = 0..k`. Canonical order is `j` descending (deepest package first). The nesting depth is S if the nested structure is recoverable: InnerClasses/EnclosingClass if kept, or a `$serializer`-nesting chain for sealed children listed in the parent's `SealedClassSerializer`. That fixes `j`. This set is exhaustive *given* no override, but since no-override is itself unproven, the pipeline only uses it for the D hint. |

**Answer to "can you detect whether an override happened?"**
- **Proven override**: when an S Kotlin name (Tier A) differs from the serial name, or when the
  serial name is not a legal identifier/FQN. **Proven non-override**: when an S Kotlin name
  equals it, or for enums via `names[k] == null` / `createSimpleEnumSerializer`.
- Kotlin metadata would decide every case (it records the Kotlin property names), but R8 strips
  or rewrites it under these rules [V: no `kotlin.Metadata` in the dex]. If an app keeps
  `kotlin.Metadata` (for example moshi-kotlin reflection or kotlin-reflect users),
  R8 *rewrites* the names to the residual (minified) ones. So it helps only for members kept by
  name, and those are already S.
- `write$Self` ordering proves **order**, not names.
- `$annotations` synthetic methods carry the `@SerialName` annotation *and* the Kotlin name in
  the method name (`getName$annotations`), but R8 deletes them as unused [V].

---

## 4. Evidence for original names: full list of string sources

**Tier A: independent Kotlin-name evidence** (compiler-emitted, exact; usable for S). Each item
must be bound to a field or class structurally.

| Source | What it names | Survives R8? |
|---|---|---|
| Data-class `toString`: `"C(p0="`, `", p1="`, … followed by `iget` of the field | Simple class name `C`; Kotlin names of **primary-ctor** properties, `@Transient` ones included | Yes [V] |
| `Intrinsics.checkNotNullParameter(param, "p")` in primary ctor / setters | Kotlin param name, which equals the property name for `val`/`var` params; bound by `iput` | Usually. Not if the app strips intrinsics or R8 specializes the call [V: removed in the sample] |
| `Intrinsics.throwUninitializedPropertyAccessException("p")` (lateinit getter / access) | Kotlin name of a `lateinit var` | Yes [I] |
| `Intrinsics.checkNotNull(x, "…")`, `checkNotNullExpressionValue(x, "getFoo(...)")` | Getter/method names of **other** (Java) code | Hint-grade only |
| Moshi codegen `Util.missingProperty("<localName>", "<jsonName>", reader)` / `unexpectedNull(…)` | Kotlin property name (see §5.3 caveat) **and** JSON name | Yes [I] |
| Moshi codegen adapter `toString()` → `"GeneratedJsonAdapter("` + `"Outer.Inner"` + `")"` | Simple-name chain of the target class | Yes [I] |
| Enum `<clinit>` `new E("NAME", ordinal)` | Enum constant names | Yes [V] |
| Kept member names from consumer rules: `Companion`, named-companion field, `INSTANCE`, `serializer`, `descriptor` | Companion object name; plugin-fixed names | Yes [V] |
| `write$Self$<module>` method name (only if not renamed) | Kotlin module name | Only when kept [V: gone in the sample] |

**Tier B: serialization evidence** (exact about the *wire format*; only a hint for Kotlin names).

| Source | Content |
|---|---|
| `PluginGeneratedSerialDescriptor(<serialName>, …, n)` / `InlineClassDescriptor(<serialName>, …)` / `EnumDescriptor` | Class serial name (`@SerialName` or FQN with `.` for nesting) |
| `addElement("<name>", optional)` in `$serializer.<clinit>` and `$cachedDescriptor` | Property serial names + optionality |
| `ObjectSerializer("<serialName>", …)`, `SealedClassSerializer("<serialName>", …)`, `createSimpleEnumSerializer`/`createAnnotatedEnumSerializer("<serialName>", …, String?[] entryNames, …)`, `EnumSerializer(…)` | Class serial names, enum-entry overrides |
| `pushAnnotation(new X$Impl(args))` / `pushClassAnnotation` | SerialInfo annotations, for example `@JsonNames("alt1","alt2")` alternative names and `@ProtoNumber(n)` |
| Moshi `JsonReader.Options.of("a","b",…)` and `writer.name("a")` | JSON names |
| Gson `@SerializedName("x", alternate={…})` annotation values on fields (kept `allowobfuscation`) | JSON names |

**Tier C: runtime templates** (identify *library* code, not app names): `"An unknown field for
index "`, `"Field '…' is required for type with serial name '…', but it was missing"` pieces,
`"[UNINITIALIZED]"`, `"Serializer for subclass '"`, `"kotlinx.serialization.Polymorphic"`,
`"kotlinx.serialization.Sealed<"`, `"This synthesized declaration should not be used directly"`
(annotation value only), and fixed `checkNotNullParameter` names in `$serializer`: `"encoder"`,
`"value"`, `"decoder"`.

---

## 5. Ambiguity analysis and enumeration/resolution

### 5.1 Property names
- Preimage of `serialName_i = s`: `{s} ∪ {every identifier k : k annotated @SerialName(s)}`.
  This is infinite, so it can't be enumerated as N. Resolve it by S (Tier A binding), or demote
  to D (`kxs/prop-name-hint`).
- Binding Tier A to elements has to go through fields. `toString` covers primary-ctor properties
  only. Body properties (`var bodyProp`) have no `toString` or ctor evidence, so they're usually
  D unless they're `lateinit`.

### 5.2 Class names
- The preimage of a class serial name is just as infinite, so it's D. The simple name is often
  S via `toString` (data classes) or Moshi. The package is almost never S.
- Cross-class consistency, such as all serial names sharing the prefix `com.example.model`, is a
  heuristic, not proof. It may feed package clustering as a *hint* (DESIGN §1 "Repackaging").

### 5.3 Moshi `localName` [I from `AdapterGenerator.kt`, `PropertyGenerator.allocateNames`]
- `missingProperty`/`unexpectedNull` receive `localName = NameAllocator.newName(name)`. KotlinPoet's
  NameAllocator appends `_` on collision with already-allocated names. Those are `moshi`, `types`,
  `reader`, `writer`, `options`, `constructorRef`, other properties, `<p>Set`, `mask<k>`,
  `result`, `localConstructor`, and so on, in allocation order. It also sanitizes non-Java
  characters to `_`.
- Enumeration: if `localName` has no trailing `_` and no `_` that could come from sanitization,
  it's S. If `localName = t + "_"*m`, the candidates are `{t + "_"*j : j=0..m}`, filtered by "the
  collision was possible". That means `t + "_"*j` for `j<m` must be a reserved/allocated name.
  This set is finite, exhaustive and canonical (ascending `j`), so it's **N**. It usually
  collapses to S.
- A `_` anywhere else could come from a sanitized backticked name (`` `a b` ``). That set is also
  finite (each `_` ∈ {`_`, any char illegal in Java identifiers}), but the second option is
  infinite over Unicode, so it's D.

### 5.4 Primary-ctor vs body split
- The element order is `super… + ctorProps + bodyProps`. The boundary is determined when the
  primary ctor survives: its non-`$default` signature lists exactly the ctor properties (and
  ctor params that aren't properties). Otherwise the candidates are the `n_own + 1` cut points,
  which is finite, so N.
- Data-class `toString` pins it: `toString` lists exactly the primary-ctor properties. This
  resolves it to S.

### 5.5 `write$Self` name
- Non-final (proven) → `write$Self`, S. Otherwise it's `{write$Self (open class without
  serializable subclasses), write$Self$<m> for any module name m}`, which is infinite, so D.

### 5.6 Collection types
- Finite N sets from `findStandardKotlinTypeSerializerName` × field erasure (see
  `kxs/property-type`), in canonical order by the table order in that function. Every one of
  these sets has size ≤ 2 once the field type is known.

### 5.7 Closed-world folding
- R8 can fold `decodeSequentially`, `shouldEncodeElementDefault` and even the whole mask
  machinery when only one encoder/decoder implementation exists [V: first sample run]. The
  rules must degrade per item: a missing fingerprint means the rule doesn't run (D·id). It must
  never mean "guess".

### 5.8 Other serialization libraries (secondary)
- **Moshi codegen**: the generated `META-INF/proguard/moshi-<Target>.pro` has
  `-keepnames class <Target>` and `-keep class <Target>JsonAdapter { <init>(…) }`
  (`ProguardRules.kt`) [V source]. So target and adapter class names are **not minified**
  (identity S). Property names come from `missingProperty`/`unexpectedNull` (§5.3), and JSON
  names from `Options.of`. `moshi.pro` keeps `@JsonClass` enum fields (names kept). moshi-kotlin
  reflection keeps `kotlin.Metadata` (`r8-from-1.6.0/moshi-metadata-reflect.pro`).
- **Gson** (`gson.pro`): only `-keepclassmembers,allowobfuscation` on `@SerializedName`/`@Expose`
  fields. Field names are minified but the annotations survive, so `@SerializedName` values
  are Tier B (JSON names). Apps using Gson without `@SerializedName` must keep field names
  themselves, which gives identity S. Gson also keeps `TypeToken` subclasses and their
  `Signature` attributes. Generic signatures are S evidence for field types.

---

## 6. Fixture ideas

Each fixture is compiled with the plugin, run through R8 (full and compat), and checked against
`mapping.txt` as the oracle (DESIGN §6.1). Use a **non-constant** encoder/decoder (or the real
`kotlinx-serialization-json`) so R8 can't fold the fingerprints (§5.7). Also run a variant that
uses `serializer(Type)` so that annotation retention is exercised.

1. `kxs-basic-data`: data class with required/optional/nullable/`@Transient`/`@Required`/
   `@EncodeDefault`/body `var`. Check the element table, binding, optionality, golden mask 19,
   `@Required` detection, and property names S via `toString`.
2. `kxs-serialname-prop`: data class with `@SerialName("user_name") val name`. Expect the name
   S (`name`) + `@SerialName` S. Non-data twin: expect the D hint `user_name_{hash}`.
3. `kxs-serialname-illegal`: `@SerialName("a.b")`, `@SerialName("x:y")`, `@SerialName("")`.
   Expect override-proven. Control cases: `@SerialName("user-name")` against a real
   `` val `user-name` `` must **not** be flagged as override-proven.
4. `kxs-serialname-lookalike`: class `a.b.User` with `@SerialName("x.y.User")`. Expect D
   (must **not** emit S `x.y.User`). This is a negative test for over-claiming.
5. `kxs-class-serialname-mismatch`: data class `Account` with `@SerialName("acct")`. Expect
   the override proven via `toString`.
6. `kxs-big-mask`: 34+ and 65+ properties. Check multi-slot masks, `throwArrayMissingFieldException`,
   and `lit16`/`const` (untyped: dexdump prints `0xBDEF7BDE` as a float) mask constants.
7. `kxs-generic`: `Box<T>`, `Pair<A,B>` holders. Check `typeSerial*`, the `$cachedDescriptor`
   duplicate sequence (both must agree), and the `serializer(KSerializer)` companion.
8. `kxs-inheritance`: open `Base` + `Derived` (same module) and `Base` from another module
   (unconditional encode path). Check the `write$Self` S-name for Base and the super-ctor prefix.
9. `kxs-sealed`: sealed with object, data class and nested-sealed children. Check the
   `SealedClassSerializer` relation and FQN-split nesting.
10. `kxs-enum`: plain (`createSimpleEnumSerializer`), per-entry `@SerialName`
    (`createAnnotatedEnumSerializer`), enum with a class-level SerialInfo annotation.
11. `kxs-named-companion`: `companion object Factory`. Expect the S companion name, with and
    without the NamedCompanion rule (1.7.3 vs HEAD rules).
12. `kxs-value-class`: `@JvmInline value class`. Check the `InlineClassDescriptor` shape and the
    mangled serialize names.
13. `kxs-external-serializer`: `@Serializer(forClass=…) object`. Expect D, not `$serializer`.
14. `kxs-collections`: `List`, `MutableList`, `ArrayList`, `Collection`, `Set`, `HashSet`,
    `Map`, `HashMap`, and their nullable forms. Check the N-sets of `kxs/property-type`.
15. `kxs-keep-generated`: `@KeepGeneratedSerializer` with a custom serializer.
16. `kxs-legacy-rules`: pre-1.5 README keep rules. Expect names not minified, and `kxs` should
    agree with the identity.
17. `kxs-alpha`: shuffle class order and rename the input; all `kxs` outputs must be
    byte-identical (DESIGN §0.3).
18. `kxs-plugin-matrix`: the same model compiled with Kotlin 1.8.x, 1.9.x, 2.0.x, 2.1.x and 2.4.x
    (K1/K2). Records the §1.9 deltas as fixtures. The [I] rows need this.
19. `moshi-codegen`: `@JsonClass(generateAdapter=true)` with a property named `reader` (NameAllocator
    collision, §5.3) and `@Json(name=…)`.
20. `gson-serializedname`: `@SerializedName` fields; check Tier-B only.

## Open items
- [I] Release mapping of the commits in §1.9, and legacy (pre-IR / pre-golden-mask) shapes. Needs
  fixture 18.
- [I] Whether real `kotlinx-serialization-json` keeps `encode*Element` interface calls with index
  constants intact in typical apps. Fixture 1 with the json artifact.
- [I] Whether `SealedClassSerializer` subclasses can omit non-serializable subclasses, which is
  what makes completeness D.
- DESIGN.md §1's row "kotlinx.serialization descriptor names | D (hint only)" is refined by
  §3.3: the serial-name facts are S, override detection is sometimes S, and Kotlin names are S
  only through Tier-A binding.
