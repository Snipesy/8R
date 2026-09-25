# Sources: compiler plugins, annotation processors, javac lowerings

Scope: what code generators other than kotlinc core and R8 leave in a release DEX, and which of
that is *proof* (S) of an original identifier under the DESIGN §0 contract when there is
**no mapping file**. The coverage and ranking at the end are ordered by prevalence in real apps.

Conventions:
- **VERIFIED** means we ran the generator plus R8 and inspected the dex.
- **INFERRED** means we only read the source or decompiled output.
- Rule ids are `<source>/<name>`.
- **N** is used only with an exhaustive candidate list in canonical order. Per DESIGN §0.1
  the pipeline must still promote N to S with a tie-break anchor, or emit a D name listing the candidates.

Toolchain for every experiment:
- **R8/D8:** `8.10.9-dev` (build-tools 36.0.0), `--release --min-api 24`.
- **javac:** JDK 26.0.2.
- **Kotlin:** 2.3.20 (embeddable compiler, via `K2JVMCompiler`).
- **KSP2:** 2.3.12.
- **AGP default rules:** 9.0.1 (`proguard-android-optimize.txt` / `proguard-common.txt`).
- **Scratch artifacts:** `/tmp/claude-1000/-home-snipesy-8R/4b02079c-3b0c-464c-bf8a-78f254b90fa3/scratchpad/agents/others/{parcelize,dagger,gen,javac}/`.

| Source | Version run | Upstream commit cited |
|---|---|---|
| kotlin-parcelize | 2.3.20 | JetBrains/kotlin tag v2.3.20 `d57eb4a26ed01ab03cf195da686c8047032ff16f` (`plugins/parcelize`) |
| Dagger / Hilt | 2.57.2 | google/dagger tag `dagger-2.57.2` `438334a32db88b32e8420b81bc323a8dbff5144b` |
| Room | compiler 2.6.1 (ran); 2.8.5 (decompiled) | androidx/androidx `536ae8930b6709522d354e32bb4498a2b6663e4c` (now `room3/`, 3.1.0-alpha02) |
| Moshi codegen | 1.15.2 via KSP2 | square/moshi 1.15.2 `7f957b18ea123b7bb2ffc9d3c079e680eecf15e1`; master `889013ec2edb8d8034902662a1dc8c4f3b3f8111` |
| ViewBinding | databinding-compiler-common 9.0.1 (driven directly) | decompiled from AGP 9.0.1 artifacts |
| Safe Args | navigation-safe-args-generator 2.10.2 | androidx `536ae893…` (navigation 2.11.0-alpha01) |
| Anvil | not run | square/anvil `28419c02dc009293f1f6602e0ade0c37a000ac72` |
| javac | 26.0.2 | openjdk/jdk `bf2e6ce918aa525b8a114b82dc056dd3c33703ce` (`Lower.java`) |

## 0. Cross-cutting findings

1. **Keep rules do much of the work.** Several generators need reflection by name, so their
   consumer rules keep names. Those classes arrive with **original names already** (they are S by
   DESIGN §5 item 1). Our job there is to *use them as anchors*, not to recover them.
   - Room keeps `* extends RoomDatabase` and hence `X_Impl`.
   - Moshi keeps each `@JsonClass` model and `XJsonAdapter`.
   - aapt2 keeps the nav-graph destinations.
   - AGP keeps Parcelable `CREATOR`.
2. **The name-derivation functions are exact and one-directional.** Dagger, Hilt, ViewBinding,
   Safe Args, Room and Moshi all name generated classes by a pure function of an *original* name:
   `Foo_Factory`, `Hilt_Foo`, `ActivityMainBinding`, `FooArgs`, `Foo_Impl`, `FooJsonAdapter`.
   So once the input name is S, the generated name is S. The reverse direction (generated → user)
   only works when the generated name survived, as with Moshi or Room.
3. **Overriding annotations are invisible in the DEX.** `@ColumnInfo` has CLASS retention, and
   `@Json`, `@SerialName` and `@Entity(tableName)` are consumed at compile time. So you can't check
   the dex for "was the name overridden?". A string that *defaults* to the identifier but *can* be
   overridden is **always a hint**, unless the generator emits the identifier *separately* from the
   overridable name, as Moshi `Util.*` and Room's validation message do.
4. **R8 horizontal class merging hits generated code hard (VERIFIED).** It merged:
   - Parcelize `$Creator`s;
   - Dagger factories, including one merged with its module;
   - Hilt SwitchingProviders across components;
   - Room `EntityInsertionAdapter`s.

   Every "one generated class per user class" recognizer must first run the class-unmerging split
   (DESIGN §1, `$r8$classId`) and treat merged-class *names* as D.
5. **Bodies get inlined into callers (VERIFIED)**, for example ViewBinding `bind`, DAO queries and
   `MembersInjector` statics. Recognizers must match **inline fragments** anchored on surviving
   strings and library calls, not whole methods.
6. **Strings can be fused by constant propagation (VERIFIED, §6.3).** R8 folded call-site
   constants into a `toString` template: `"User(name=" + name + ", age=" + …` became
   `"User(name=n, age=1, o="`. Every string-template S rule must check that the literal is an
   *unfused* template: its pieces must match the generator's exact pattern, with each hole still
   fed by a field read.

## 1. kotlin-parcelize

### 1.1 Generated shape (VERIFIED, javap; source `plugins/parcelize/parcelize-compiler/parcelize.backend/.../ParcelizeIrTransformerBase.kt`)

**`public static final Parcelable$Creator CREATOR`**
- `<clinit>` runs `new Outer$Creator; <init>()V; checkcast; putstatic CREATOR`. For an `object`, `INSTANCE` is initialized first.

**`Outer$Creator`**
- `public final`, implements `Parcelable$Creator<Outer>`, has no fields and a trivial `<init>`.
- The name is hard-coded: `Name.identifier("Creator")` (`ParcelizeIrTransformerBase.kt:149`).
- It is emitted as a *local* class: an InnerClasses entry `Creator` with no outer class, `EnclosingMethod` = `Outer` with a null method, and Kotlin metadata `k=3`.

**`createFromParcel(Parcel)`**
- Starts with `checkNotNullParameter(p, "parcel")`.
- Then one read per constructor parameter, in order, then `new Outer(...)`.
- If there is an `@IgnoredOnParcel` parameter, it calls the synthetic defaults constructor with a mask.

**`newArray(int)`**
- `anewarray Outer`, or `Parceler.newArray` when a companion Parceler is present.

**`writeToParcel(Parcel,int)`**
- Starts with `checkNotNullParameter(p, "dest")`.
- Then `getfield` + write per property, **in primary-constructor parameter order**. `@IgnoredOnParcel` and non-property parameters are skipped (`:242-260`, `:204-209`).

**`describeContents()`**
- Returns `iconst_0`, or `iconst_1` iff some property type is or contains `java.io.FileDescriptor` (`:44-49`, `:262-267`).

**Per-type serializers** (VERIFIED; `IrParcelSerializerFactory.kt:69-345`, `IrParcelSerializers.kt`):

| Type | Write | Read |
|---|---|---|
| `String`/`String?` | `writeString` | `readString` |
| `Int`, `Long`, `Float`, `Double` | direct `write*` | direct `read*` |
| `Boolean` | `writeInt(z?1:0)` | `readInt()!=0` |
| nullable primitive or Parcelize type | `writeInt(0)` if null, else `writeInt(1)` then the value | `readInt()==0 ? null : …` with `valueOf` boxing |
| `List<String>` | `writeStringList` | `createStringArrayList` |
| `Map`/`Set`/`List<T>` | `writeInt(size)` then a loop | `new LinkedHashMap(n)` / `LinkedHashSet` / `ArrayList(n)` then a loop |
| enum | `writeString(e.name())` | `E.valueOf(readString())` |
| final same-module `@Parcelize` type | `inner.writeToParcel(p, flags)` | `Inner.CREATOR.createFromParcel` |
| other `Parcelable` | `writeParcelable` | `readParcelable(Outer.class.getClassLoader())` |
| `CharSequence` | `TextUtils.writeToParcel` | `TextUtils.CHAR_SEQUENCE_CREATOR` |
| `Serializable` | `writeSerializable` | `readSerializable` |
| `@TypeParceler` / `@WriteWith` | `P.INSTANCE.write(v, parcel, flags)` | `P.INSTANCE.create(parcel)` |

### 1.2 What survives R8 (VERIFIED)

AGP `proguard-common.txt` (the kotlin-parcelize-runtime jar ships **no** consumer rules) has:

```
-keepclassmembers class * implements android.os.Parcelable { public static final ** CREATOR; }
```

It also has `-keepattributes …EnclosingMethod,InnerClasses,…Signature`.

What survives:
- `CREATOR` keeps its name. Its `Signature` `Parcelable$Creator<La/i;>` links it to its class.
- `writeToParcel`, `describeContents`, `createFromParcel` and `newArray` keep their names (they are library overrides). Outer and creator class names are minified.

**Full mode (the R8 default):**
- All 9 creators in the sample were horizontally merged into one class `a.a`, with an int classId field and a `packed-switch` in `createFromParcel`/`newArray`.
- Each `Outer.<clinit>` does `new a.a(k)`.
- The merged class lost InnerClasses and EnclosingMethod.

**Compat mode:**
- The creators were not merged. They became `Outer$a` with `EnclosingClass=Outer`, so the nesting is kept but the name `Creator` is lost.

Other R8 effects:
- Custom Parceler bodies were inlined.
- The boolean ternary was simplified.
- An enum was **unboxed despite AGP's `valueOf` keep**. The write became a switch to `const-string "RED"`, and the read became an `equals` chain.
- A `List` field was widened to `Object`.
- Read order, write order and constructor-argument order were **always preserved**.

Parcelize emits **no property-name strings** (VERIFIED). The only plugin strings are the generic
`"parcel"` and `"dest"`.

### 1.3 Rules

| id | Class | Preconditions → result |
|---|---|---|
| `parcelize/creator-field` | S (no-op) | Already kept. Use the `Signature` generic argument (or the `sput` in `<clinit>`) to link the creator instance to its Outer class. |
| `parcelize/recognize` | S (structure) | Mirror check: the i-th `writeToParcel` write and the i-th `createFromParcel` read use the same serializer from the table above, and `createFromParcel` ends in `new Outer(<reads in order>)`. There is also a `checkNotNullParameter(_, "parcel")` / `(_, "dest")` pair. Exclusions: Java anonymous `CREATOR` (`$1`, no intrinsics) and hand-written Kotlin `companion object CREATOR` (the field type is the companion). |
| `parcelize/creator-class-name` | S | Recognized, the creator class is **unmerged** (a single `new-instance` site in `Outer.<clinit>` feeding `sput CREATOR`, no classId), and Outer's name is S. Result: `<Outer>$Creator`, with the inner name `Creator`, which is hard-coded. If Outer's name is D, the result is `<Outer_hash>$Creator`: the suffix part is still exact, but the attribute is D by composition. |
| `parcelize/merged-creator` | D | Split by classId (the DESIGN class-unmerging rule). Case k ↔ Outer (the `const k` in `Outer.<clinit>`) is S structure. After the split, each part gets `parcelize/creator-class-name`. |
| `parcelize/field-slot-link` | S (links only) | The i-th write reads field Fᵢ. The i-th read feeds constructor argument i. The `<init>` stores argument i into Fᵢ. This gives an exact field↔parameter↔slot bijection, which resolves "two `Int` fields, which is which" **positionally**. It yields no names by itself. |
| `parcelize/names-via-intrinsics` | S | Combine with `Outer.<init>`'s `checkNotNullParameter(argᵢ, "name")`. Parcelize requires every primary-constructor parameter to be a `val`/`var` property, so the parameter name equals the property name. The property name then becomes the backing field name, and the getter is `get<Name>` via metadata or propagation. This covers only non-null reference types; primitives and nullables stay D. It is also S for a Kotlin data class's `toString` via the same slot link. |
| `parcelize/describe-fd` | D hint | `describeContents` returning 1 means some property holds a `FileDescriptor`. |

### 1.4 Ambiguity and fixtures

Ambiguity:
- The name of a merged creator: the merge target is arbitrary, so it's D.
- Primitive and nullable property names are not recoverable.
- A `Parceler` companion's method names are recoverable only from the `kotlinx.parcelize.Parceler` interface (`create`/`write`/`newArray`). When R8 inlined them they're gone.

Fixtures:
- Two same-typed `Int` fields.
- `@IgnoredOnParcel`.
- A `FileDescriptor` property.
- A companion Parceler, and `@TypeParceler`/`@WriteWith`.
- An `object`, an empty class, and a sealed hierarchy.
- An enum both kept and unboxed.
- A 1-Parcelable app (no merge) versus a 9-Parcelable app (merge), each in full and compat mode.
- Negatives: a Java `CREATOR` and a hand-written Kotlin `companion object CREATOR`.

## 2. Dagger 2 / Hilt

### 2.1 Naming functions (INFERRED from source, confirmed by generated output)

These come from `dagger-compiler/main/java/dagger/internal/codegen/binding/SourceFiles.java`, which uses `Joiner.on('_')` (l.63).
- `classFileName(C)` = C's enclosing simple names joined by `_` (e.g. `Outer_Inner`).
- Constructor factory: `classFileName(C) + "_Factory"`.
- `@Provides`/`@Binds` factory: `classFileName(Module) + "_" + UpperCamel(method) + "Factory"`.
- `C_MembersInjector`, with static `inject<UpperCamel(field)>[index+1]` (l.225-233).
- `Dagger<classFileName(Component)>` (`writing/ComponentNames.java:58`), `_LazyMapKey` (`MapKeys.java:229`), and `_Impl` for assisted factories.

Hilt (`hilt-compiler/main/java/dagger/hilt/...`):
- `Hilt_` + enclosed names (`AndroidEntryPointMetadata.java:167`).
- `C_GeneratedInjector.injectC` (l.131, 139).
- `<Root>_HiltComponents` (`ComponentNames.java:59`).
- `hilt_aggregated_deps._<fqn with . → _>` (`Processors.java:88, 272`).
- The Gradle transform rewrites the superclass to `pkg/Hilt_<simple with $ → _>` (`plugin/.../AndroidEntryPointClassVisitor.kt:75,140`).
- The Hilt Gradle plugin always enables `dagger.fastInit` (`HiltCommandLineArgumentProvider.kt:41`).

### 2.2 Shapes (VERIFIED, 2.57.2)

**`Foo_Factory implements dagger.internal.Factory`**
- One `Provider` field per constructor parameter.
- `get()` returns `newInstance(p.get(), …)`, and `newInstance` does `new Foo(…)`.
- A no-argument factory gets `InstanceHolder.INSTANCE`.

**`Module_ProvideXFactory`**
- Wraps the module call in `Preconditions.checkNotNullFromProvides`.

**`MembersInjector`**
- Static single-`iput` methods annotated `@InjectedFieldSignature("pkg.C.field")`.

**`DaggerC.Builder.build()`**
- Calls `checkBuilderRequirement(module, Module.class)`. The message `" must be set"` is built from `getCanonicalName()` at **runtime**, so there's no literal (`dagger-runtime/.../Preconditions.java:126`).

**fastInit `SwitchingProvider`**
- Has an `int id`, and `get()` is a `switch(id)` whose default throws `new AssertionError(id)`.
- Ids are allocated in first-request order; cases are split into chunks of 100 per `getN` (`writing/SwitchingProviders.java:75,140,196-260`).
- `initialize` is split every 25 statements (`ComponentImplementation.java:263`).

Annotation retention (VERIFIED, javap):
- CLASS retention, so none reach the dex: `DaggerGenerated`, `InjectedFieldSignature`, `QualifierMetadata`, `ScopeMetadata`, `IdentifierNameString`, `KeepFieldType`, and Hilt's `AggregatedDeps`, `HiltViewModel`, `InstallIn`, `OriginatingElement`, `AndroidEntryPoint` and `GeneratedEntryPoint`.
- dexdump shows zero of them even with `-keepattributes *Annotation*`. **The `@InjectedFieldSignature` field-name string never ships.**

### 2.3 Consumer rules (VERIFIED, exact)

- dagger jar `META-INF/com.android.tools/r8/r8.pro`:
  `-identifiernamestring @dagger.internal.IdentifierNameString class ** { static java.lang.String *; }`
- `…/proguard/proguard.pro` (ProGuard only, not read by R8): `-keepclassmembers,includedescriptorclasses class * { @dagger.internal.KeepFieldType <fields>; }`
- hilt-android aar `proguard.txt`: `-keep,allowobfuscation,allowshrinking` for `@EntryPoint`, `@ComponentEntryPoint`, `@GeneratedEntryPoint` and `@EarlyEntryPoint` classes.
- Generated `META-INF/proguard/<module>_LazyClassKeys.pro`: `-keep,allowobfuscation,allowshrinking class <ViewModel>`. It blocks merging and does not keep names (`LazyClassKeyProcessingStep.java:48,116-122`).
- History (INFERRED, commit `0786d0af5`, Dagger 2.51): before 2.51 Hilt shipped
  `-keepnames @HiltViewModel class * extends ViewModel` and keyed ViewModels by
  `@StringKey("pkg.FooViewModel")`, a **literal original FQN**. From 2.51 on it uses `@LazyClassKey`
  with `-identifiernamestring`, so **R8 rewrites the string to the minified name** (VERIFIED:
  `put("a.h", …)`, `singletonMap("a.l", TRUE)`).

### 2.4 What survives R8 (VERIFIED)

**Full mode:**
- `DaggerAppComponent` and its Builder are removed; `AppComponentImpl` survives.
- All `MembersInjector`s are inlined into callers.
- Factories are class-inlined, or horizontally merged with each other **and with the module class**.
- `hilt_aggregated_deps.*` and `aggregatedroot.*` are gone, so **the FQN-encoded names never ship**.
- `Hilt_MainActivity` is vertically merged into the manifest-kept `MainActivity`. Its template fields survive, renamed.
- `*_GeneratedInjector` interfaces survive as empty renamed markers, implemented by the component impl.
- SwitchingProviders from different components are merged.

**`-dontoptimize`:** everything survives 1:1, renamed.

Surviving strings are library literals only: DoubleCheck's cycle message, `"Cannot return null from a
non-@Nullable @Provides method"`, and Hilt runtime messages. **There is no original user identifier.**

### 2.5 Rules

| id | Class | Preconditions → result |
|---|---|---|
| `dagger/runtime-lib` | S (library) | Delegate to SigDB (DESIGN §5.4). Unique string sets (the DoubleCheck triple, Hilt `"Hilt Activity must be attached to an @HiltAndroidApp Application. "`, and so on) seed `dagger.internal.*` and `dagger.hilt.android.internal.*`. |
| `hilt/entrypoint-base` | S | Manifest-kept class C (S) whose own fields or superclass H's fields match the Hilt template: a field of type `ActivityComponentManager` / `ApplicationComponentManager` / `FragmentComponentManager` (identified via the library), a lock `Object`, a `boolean injected`, and a `SavedStateHandleHolder` for activities. If H exists: `H = pkg.Hilt_<C simple names joined by _>`. The members are template names: `componentManager`, `componentManagerLock`, `injected`, `savedStateHandleHolder`, `generatedComponent`, `inject`, `createComponentManager`, `_initHiltInternal`, `initSavedStateHandleHolder`. When H was merged into C the member names are still S (fixed template), and the report notes the merge. |
| `hilt/component-impls` | S | Hilt root Application R is manifest-kept. The impl classes are identified by the library entry-point interfaces they implement (`ViewModelCImpl` implements `HiltViewModelFactory$ViewModelFactoriesEntryPoint`, `ActivityCImpl` implements `DefaultViewModelFactories$ActivityEntryPoint`, `ActivityRetainedCImpl` implements `ActivityRetainedComponentManager$ActivityRetainedLifecycleEntryPoint`, and so on). Result: `DaggerR_HiltComponents_SingletonC$<X>CImpl`. Only for unmerged classes. |
| `hilt/generated-injector` | S if unique, else N | An interface kept by the `@GeneratedEntryPoint` rule and implemented by an identified `<X>CImpl`. With the method present (`(C)V`): `C_GeneratedInjector`, method `injectC`, which is S. When optimized to an empty marker, the candidate set is every Hilt entry point whose component level matches. N lists them sorted by their S names; exactly one candidate means S. |
| `dagger/factory-shape` | S given S target, else D | Implements `Factory`/`Provider` (identified via the library), and `get()` constructs exactly `new X(p₁.get(), …)`, directly or via one static. Result: `classFileName(X)+"_Factory"`, `get`/`create`/`newInstance`, `InstanceHolder.INSTANCE`. The provider *field* names come from constructor parameter names, which are unprovable, so they are D: `<lowerCamel(type)>Provider_<hash>`. |
| `dagger/provides-factory` | D | `get()` wraps a module call with the inlined `"Cannot return null from a non-@Nullable @Provides method"` check. The naming function is exact, but the `@Provides` method name is lost, so the result is `<Module>_Provide<RetType>Factory_<hash>`. The NPE string also marks every inlined `@Provides` call site. |
| `dagger/members-injector` | S class / D methods | Only in the `-dontoptimize`-like case, where the class of static `(X,T)V` single-`iput` methods survives. Class `X_MembersInjector` is S if X is S. Method `inject<Field>` is S only if the field name is S from elsewhere. |
| `dagger/switching-provider` | D | An `int` field and a `switch` whose default throws `AssertionError(int)`. Result: `SwitchingProvider` + `id` + `get`, all D, because merged variants look the same until they are split. |
| `dagger/lazy-map-key` | S (link) / D (name) | A static String whose value equals some class X's *current* DEX name. After 8R renames X, the string must be **rewritten to X's new name**: it's an identifier-name-string, which is a semantics requirement. It's evidence that X is a map-key class (in Hilt, a `@HiltViewModel`). It is **never** a name proof. |
| `hilt/legacy-vm-strings` | S | Hilt older than 2.51 (detected by string keys not equal to any DEX class name, together with `-keepnames`'d ViewModels): the string is the original FQN of the kept ViewModel. That is S for `<FQN>_HiltModules`, `$KeyModule`, `$BindsModule` and `<FQN>_Factory`. INFERRED only; this needs a fixture. |

**Proof vs. hint:**
- **Proof:** manifest-kept names propagated through the naming functions; library strings (for the library classes only); pre-2.51 ViewModel key strings.
- **Hints:** the `@Provides` NPE string (a role marker only), LazyClassKey strings (they are minified), and SwitchingProvider case order.

Fixtures: `dagger/sample` in default and fastInit modes, each optimized and `-dontoptimize`; the Hilt app with and without fastInit; a Hilt ≤2.50 build; a multi-ViewModel app; a component with more than 100 bindings.

## 3. Room

### 3.1 Shape (VERIFIED 2.6.1 javac-AP; INFERRED 2.8.5/room3)

`AppDatabase_Impl`:
- `createAllTables` contains `CREATE TABLE IF NOT EXISTS \`User\` (\`id\` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, \`first_name\` TEXT, …)` and the identity-hash insert.
- `onValidateSchema` builds `TableInfo.Column("first_name", …)`.
- On mismatch it builds the string below. It uses the **canonical** name (dots, not `$`, for nested entities); `@Entity(tableName="books")` on `Book` gives `"books(com.example.data.Book)…"`:

  ```
  "User(com.example.data.User).\n Expected:\n"
  ```
- Views and FTS tables use the same form (`ViewInfoValidationWriter`, `FtsTableInfoValidationWriter`).
- The source template is `"${entity.tableName}(${entity.element.qualifiedName}).\n Expected:\n"` (room3 `TableInfoValidationWriter.kt:155`; same in decompiled 2.6.1 and 2.8.5).
- `InvalidationTracker(…, "User", "books", …)` lists tables in `@Database(entities=)` order.

`UserDao_Impl`:
- `INSERT OR ABORT INTO \`User\` (\`id\`,\`first_name\`,…) VALUES (nullif(?, 0),?,…)`. The i-th `bind` index corresponds to the i-th column, which comes from the i-th field or getter.
- `@Query` SQL is kept verbatim, with `?` in place of each `:param`.
- `CursorUtil.getColumnIndexOrThrow(c, "first_name")`.

### 3.2 R8 (VERIFIED)

Consumer rules:
- 2.6.1 `proguard.txt`: `-keep class * extends androidx.room.RoomDatabase`.
- 2.8.5: `… { void <init>(); }`.
- room3: `META-INF/com.android.tools/r8/room.pro`, with the same rule for `androidx.room3`.

The lookup is reflective. Both 2.6.1 `Room.getGeneratedImplementation` and 2.8.5
`KClassUtil.findAndInstantiateDatabaseImpl` compute the name as follows, unless a
`RoomDatabaseConstructor` is supplied in 2.7+ KMP:

```
pkg + "." + canonical.substring(pkg.length+1).replace('.', '_') + "_Impl"
```

Result:
- `AppDatabase` and `AppDatabase_Impl` keep their names.
- Entities, DAOs and DAO impls are renamed. The DAO interface is removed.
- Insertion adapters for different entities are horizontally merged.
- DAO query methods are inlined into callers.
- **Every SQL and validation string survives**, including the single `const-string` `"User(com.example.data.User).\n Expected:\n"`.

### 3.3 Rules

| id | Class | Preconditions → result |
|---|---|---|
| `room/db-anchor` | S (no-op) | The kept subclass of the (library-identified) `RoomDatabase` and its kept `X_Impl`. Anchor only. |
| `room/entity-fqn` | **S** (or N) | The string matches `^([^()]+)\((.+)\)\.\n Expected:\n$` and flows into the `ValidationResult` constructor (`RoomOpenHelper$ValidationResult`, or `RoomOpenDelegate$ValidationResult` in 2.7+). Group 2 is the entity's canonical name. It is computed from the compile-time element and **no annotation can override it**. **Linking to a DEX class:** the entity E is the `check-cast`/field-owner type in the `bind` branch of the insertion adapter whose SQL is `INTO \`<group 1>\``. Otherwise it's the type instantiated by queries `FROM \`<group 1>\``. If no link exists (the entity was never instantiated and was removed), record the name only. **Canonical → binary name:** use E's nesting depth d from the `InnerClasses`/`EnclosingClass` attributes, which AGP keeps by default. The last d+1 segments are joined with `$`, which is unique and therefore S. If the nesting attributes are absent, it's **N** with candidates `{split at k : k = n-1, n-2, …, 0}`, where `pkg = seg[0..k)` and `binary = pkg + "." + seg[k..n).join('$')`. Order is by descending package length, and the set is exhaustive: n segments give n candidates, and k=0 is the default package. The tie-break is package existence among S-named classes. |
| `room/table-name` | D hint | Group 1 is the table name. It equals the simple name unless `tableName=` was used. If group 1 equals the last segment of group 2 that's consistent, but it adds nothing because group 2 already gives the name. |
| `room/column-bind-link` | S (link) | The i-th `?` column ↔ `bind(i, <read of field/getter F>)`. This gives the exact field↔column pairing. |
| `room/column-name` | D hint | Column name → field name hint. `@ColumnInfo(name=)` (CLASS retention, invisible) and `@Embedded(prefix=)` break it: `first_name` came from `firstName`. **Never S.** |
| `room/kotlin-messages` | S / hint | Kotlin codegen (KSP `generateKotlin`, 2.7+), INFERRED. `"Relationship item '<prop>' was expected to be NON-NULL…"` gives the `@Relation` **property** name, which is S. `"…expected to return a NON-NULL object of type '<FQN>'."` gives the return-type FQN, which is S (the same canonical-name caveat applies). `"Missing column '<col>'…"` gives a column name only, which is a hint. |
| `room/dao-impl` | D | The DAO name is not in any string, and the `userDao()` override on the kept `_Impl` is renamed. `<Iface>_Impl` only when the interface is S from elsewhere. |

Fixtures:
- Present: `@ColumnInfo`, `@Embedded(prefix)`, `tableName`, a nested entity, a projection POJO, a getter/setter entity.
- To add: a Kotlin data-class entity (its `toString` gives S properties, see §1.3), `@Relation`, FTS, `@DatabaseView`, a KSP Kotlin-codegen variant, and a stripped nesting-attribute variant (the N path).

## 4. Moshi codegen (and other JSON)

### 4.1 Shape (VERIFIED, 1.15.2 via KSP2)

Source: `moshi-kotlin-codegen/.../api/AdapterGenerator.kt` (master l.309-322, 702-710, 806-814).
- `JsonReader.Options.of("id", "first_name", …)` lists the JSON names in property order.
- `moshi.adapter(String::class.java, emptySet(), "firstName")`: the third argument is the **raw property name** of the *first* property that uses each distinct delegate type.
- `toString()` returns `"GeneratedJsonAdapter(UserDto)"`. The name is the simple names joined by `.`, so a nested class gives `Outer.Inner`.
- `Util.unexpectedNull("firstName", "first_name", reader)` and `Util.missingProperty("firstName", "first_name", reader)`. The first argument is `property.localName` and the second is `property.jsonName`. This is the **property name emitted separately from the overridable JSON name.**
- Caveat: `localName = NameAllocator.newName(name)`. A name that collides with a reserved local name gets `_` suffixes. Reserved names include `moshi`, `types`, `reader`, `writer`, `options`, `constructorRef`, `mask<N>`, `result`, the property types' simple names, and Kotlin keywords. VERIFIED examples: `options` → `"options_"`, `value` → `"value__"`.
- `toJson` has the shape `writer.name("first_name"); adapter.toJson(writer, value.firstName)`, which links JSON name ↔ field.

### 4.2 R8 (VERIFIED)

Generated per-model rules (`ProguardRules.kt`) are in `META-INF/proguard/moshi-<fqn>.pro`:

```
-keepnames class com.example.net.UserDto
-if class com.example.net.UserDto
-keep class com.example.net.UserDtoJsonAdapter { public <init>(com.squareup.moshi.Moshi); }
```

The runtime lookup is `Types.generatedJsonAdapterName`: `binaryName.replace('$','_') + "JsonAdapter"`.

Result:
- Models and adapters keep their names. Fields are renamed.
- `Util.unexpectedNull` becomes `e.j("firstName","first_name",q)`, and its body still has `"Non-null value '"`. `missingProperty` keeps `"Required value '"`.
- `toString` is folded to a single constant.
- `moshi.pro` keeps `@JsonClass` enum fields.

### 4.3 Rules

| id | Class | Preconditions → result |
|---|---|---|
| `moshi/model-anchor` | S (no-op) | The kept model, and the adapter named `<model binary, $→_>JsonAdapter` (pairing is by name). If `generateProguardRules=false`, fall back to the next rule. |
| `moshi/model-name-from-tostring` | S (simple name) | A `JsonAdapter` subclass whose `toString` returns `"GeneratedJsonAdapter(<N>)"`, with the unfused exact template: `<N>` gives the dotted simple names. The package equals the adapter's package, which is S only if the adapter name is kept. |
| `moshi/property-from-util` | **S** | A static `(String,String,JsonReader)` whose body carries the `"Required value '"` or `"Non-null value '"` fingerprint (library match). Argument 1 is a `const-string` L. Bind L to the constructor slot or setter receiving that local, then to the field. **If L ends in `_`:** the candidates are `{L with j trailing '_' removed : j = 0..m}`, where m is the count of trailing `_`. That set is exhaustive, because NameAllocator only appends `_`. Promote to S when a single candidate is in the reserved set for this adapter (`moshi`, `types`, `reader`, `writer`, `options`, `constructorRef`, `result`, `mask\d+`, Kotlin keywords, the property type simple names), or when a `moshi.adapter(…, "<raw>")` argument pins it. Otherwise it's N. |
| `moshi/property-from-adapter-arg` | S | The third `const-string` argument of `Moshi.adapter(Type, Set, String)` is the raw name of the first property using that delegate. |
| `moshi/json-override-detect` | S (fact) | Argument 1 ≠ argument 2 in a Util call proves `@Json(name=)` was used on that property. |
| `moshi/property-from-json-name` | D hint | Nullable and non-first properties with no Util call or adapter argument. The JSON name (from `Options.of` / `writer.name`) is overridable. |
| `moshi/adapter-members` | D | `options`, `<type>Adapter`, `constructorRef`. These follow a deterministic function of the delegate key, with collisions suffixed. |

Other serialization:
- **Gson 2.11** (INFERRED, `gson.pro`): `-keepclasseswithmembers,allowobfuscation class <1> { @SerializedName <fields>; }` and `-keepattributes RuntimeVisibleAnnotations`. The `@SerializedName` value survives as an annotation. It's a **hint**, since it's the override itself.
- **kotlinx.serialization**: `PluginGeneratedSerialDescriptor(serialName)` and `addElement(name)` are `@SerialName`-overridable, so they're a **hint** (DESIGN §1 agrees). Unlike Moshi, there's no separate property-name string (INFERRED).

Fixtures:
- Present (`gen/moshi/src/.../Models.kt`): a `@Json` override, the collisions `options`/`value`, a nullable property, a default value, a body `var`, a nested class, and an enum.
- To add: generics (`types`), qualifiers, `@Json(ignore)`, and `generateProguardRules=false`.

## 5. ViewBinding / DataBinding, Safe Args

### 5.1 ViewBinding (VERIFIED: generator driven directly; R8 run)

Naming (databinding-compiler-common 9.0.1):
- **Class:** `ResourceBundle.getFullBindingClass()` is `modulePkg + ".databinding." + toClassName(layoutFile) + "Binding"`. `toClassName` splits on `[_-]` and capitalizes each part.
- **Field:** `stripNonJava(idName)` splits on `[^a-zA-Z0-9]`. The first part is kept as-is, later parts are capitalized, and empty parts are dropped (`btn__ok` → `btnOk`). Collisions get `1`, `2`, … via `getUniqueName`.
- **Order:** fields are **sorted by name**, and the constructor takes `(rootView, fields in sorted order)`.

`bind()` shape:
- `findChildViewById(root, R.id.x)`, a null check, then `break missingId`.
- It ends with `NullPointerException("Missing required view with ID: ".concat(getResourceName(id)))`.
- A view present in only some configurations has no null check.
- `<include>` gives `XBinding.bind(v)`.
- The viewbinding AAR ships **no** keep rules.

After R8 (final and non-final R behave the same):
- `R` is removed and ids are inlined as constants.
- `inflate` and `bind` are **inlined into `onCreate`**.
- Surviving pieces: `inflate(0x7f0b0001)`, `a.a.a(view, 0x7f0800xx)` (the renamed `findChildViewById`), the `"Missing required view with ID: "` string, and `new c.a(LinearLayout, ImageView, …)`.
- Unread fields are dropped.

| id | Class | Preconditions → result |
|---|---|---|
| `viewbinding/field-from-id` | **S** | Several things must hold: `findChildViewById` is recognized by its body (a loop over `ViewGroup.getChildAt` + `findViewById`), or is the library-matched `androidx.viewbinding.ViewBindings`; the `"Missing required view with ID: "` anchor is in the same method; binding-class field F is assigned from constructor argument k, whose value traces to `findChildViewById(root, CONST)`; and resources.arsc resolves CONST to entry `id/<name>`. The arsc names must not be collapsed: reject `aapt2 optimize --collapse-resource-names` output and AndResGuard-style names. Result: F = `stripNonJava(name)`. **Consistency check:** the surviving fields' names must be strictly sorted in constructor-parameter order; if not, downgrade to D. **Collision:** if two layout ids strip to the same base, it's N with candidates `{base, base1, base2, …}` (one per colliding id). The assignment is fixed by sorted id-name order, which pins it and promotes it to S. |
| `viewbinding/class-from-layout` | S (simple name) / N (package) | An `inflate(CONST)` whose arsc entry is `layout/<file>` flows into the binding's construction. Simple name: `toClassName(file)+"Binding"`. Package: N over `{<arsc package>.databinding} ∪ {<ns>.databinding : ns ∈ library namespaces present in the APK}`. Canonical order is lexicographic. The resource-table package is the app's, and library namespaces aren't recorded, so promote to S only if the layout is unique to the app module (no library defines it). |
| `viewbinding/root` | D | `rootView` / `getRoot()`. |
| `databinding/*` | hint | `<data class="…">` can override the class name. `DataBinderMapperImpl`'s `"layout/activity_main_0"` keys were **not verified** (they're emitted by the annotation processor, not compiler-common). |

Ambiguity:
- `tools:viewBindingIgnore` means no class is generated, so a recognizer miss is correct.
- `tools:viewBindingType` changes only the type.
- `<merge>` roots have a different `inflate` signature.
- A binding class with all of its fields dropped by R8 still gets its class name from `inflate`.

### 5.2 Navigation Safe Args (generator VERIFIED; R8 INFERRED, not run)

Naming (navigation-safe-args-generator 2.10.2):
- Classes: `<DestSimple>Args` and `<DestSimple>Directions`, in the destination's package. The destination is `android:name`, with a leading `.` prefixed by the applicationId and `$` → `_` (`Destination.kt`).
- Action method: `toCamelCaseAsVar(actionId)` (`actionHomeToDetail`), with an inner class `ActionHomeToDetail`.
- Arguments: `sanitizedName` splits on `[^a-zA-Z0-9]` and camelCases, with `get`/`set` + capitalized names. Collisions are a compile error (`NavParserErrors.sameSanitizedNameArguments`), so the mapping is injective.

Strings:
- `containsKey("user_id")`.
- `"Required argument \"user_id\" is missing and does not have an android:defaultValue"`.
- `"Argument \"title\" is marked as non-null but was passed a null value."`.
- Java `toString`: `"DetailFragmentArgs{" + "userId=" …` and `"ActionHomeToDetail(actionId=" …`.
- Kotlin: `data class DetailFragmentArgs(…)`, which gives the data-class `toString` names.

What survives:
- VERIFIED: aapt2 emits `-keep class <dest> { <init>(...); }` for each nav `android:name`, so **destination names are kept**.
- VERIFIED: the compiled nav XML retains argument names, types, defaults and action ids.
- INFERRED: navigation-common keeps `fromBundle(Bundle)` on `NavArgs` implementors.

| id | Class | Preconditions → result |
|---|---|---|
| `safeargs/args-class` | S / N | A NavArgs implementor (library-identified) with kept `fromBundle`. Its set of `containsKey`/`get*` key strings equals the `<argument>` name set of a destination X in nav XML. That gives `<X simple>Args` in X's package. X is kept (aapt2), so this is S when exactly one destination has that argument set. Otherwise it's N over those destinations, sorted by FQN. The Java `toString` prefix `"<Name>{"` (unfused) is direct S. |
| `safeargs/arg-accessors` | S | An accessor that reads key k (from the bundle or HashMap) is named `get<Cap(sanitize(k))>`. The field is `sanitize(k)` (Kotlin property). |
| `safeargs/directions` | S / N | Static methods that return `ActionOnlyNavDirections(CONST)` or an `Action*` instance with `getActionId()==CONST`. arsc gives CONST → `id/<action>`, and the nav XML gives the action's owning destination X. The class is `<X>Directions`, the method is `toCamelCaseAsVar(action)`, and the inner class is its capitalized form. The same action id under several destinations gives N over the owners (sorted). |

### 5.3 Kotlin Android Extensions and Anvil (INFERRED, brief)

- **KAE** (removed in Kotlin 1.8): `_$_findCachedViewById(int)`, `_$_findViewCache` (HashMap) and `_$_clearFindViewByIdCache` are all renamed by R8. `kae/find-cache` is **D**: recognize the HashMap-get/`findViewById`/put shape, name it by the template, then treat call sites like `findViewById`. The synthetic properties were compile-time only.
- **Anvil**: generates hint properties in package `anvil.hint` named `<fqn _-joined>_reference` / `…_scope<N>` (`Utils.kt:66-77`, `ContributesToCodeGen.kt:50-75`). They have no runtime reference, so R8 should strip them (not verified). The rest is Dagger output (§2). There's no rule beyond Dagger's.

## 6. javac lowerings

Setup (VERIFIED): `javac 26.0.2` with `--release 8` and `--release 17`, sample
`scratchpad/agents/others/javac/src/p/Low.java`. D8/R8 are `8.10.9-dev` (build-tools 36.0.0),
`--min-api 24`, and the only keep rule is `-keep class p.Low { main }`. Source citations are
from openjdk/jdk `src/jdk.compiler/share/classes/com/sun/tools/javac/comp/Lower.java` @
`bf2e6ce918aa525b8a114b82dc056dd3c33703ce`.

**Headline (VERIFIED):** the R8 outputs for `--release 8` and `--release 17` are byte-identical
except for the dex header offsets. Every javac-version-dependent lowering (indy concat vs
`StringBuilder`, nestmates vs `access$NNN`) is **erased** by R8. Nothing below can yield
an S *name*. What it can yield is S/D *structure* (re-sugaring).

### 6.1 String `switch`

*Shape (VERIFIED, javac):* `s.hashCode()` → `lookupswitch` over the hashes. In each bucket
there is an `equals` chain that sets `tmp = <position>`, where the position is the label's index in
**source case order** (`caseLabelToPosition`, `LinkedHashMap`, Lower.java `visitStringSwitch`).
A second `tableswitch` on `tmp` follows. Within a hash-collision bucket, javac tested `"BB"` before
`"Aa"` even though `"Aa"` came first in the source. Bucket order is therefore not source order, but
the `tmp` values are.

*After R8 (VERIFIED):* the shape survives: `sparse-switch` on `hashCode`, the `equals` chains, then a
`packed-switch`. The case-label **strings survive verbatim**. R8 re-lowered the switch through
its own string-switch IR, and **re-numbered the positions** (javac: alpha=0, beta=1, Aa=2, BB=3;
R8: alpha=3, beta=2, Aa=1, BB=0). So source case order is *not* recoverable from `tmp` values
after R8.

| Rule | Class | Preconditions | Evidence |
|---|---|---|---|
| `javac/string-switch-resugar` | **D** | `hashCode()` switch whose every key equals `lbl.hashCode()` for the `const-string` labels in its bucket. `equals` chain → int temp → dense switch. Each temp value is reached from exactly one label. The default path is the `-1`/fallthrough. | Semantics-preserving fold into a `switch(String)`. Case order is canonical: **sorted by label string (UTF-16 code units)**. It is not original. Also matches kotlinc `when(String)` and R8's own output, so the source language is not claimed. |
| `javac/string-switch-order` | ✗N → identity | — | Source case order: after R8, the positions are permuted, so there is no evidence. Don't enumerate it (n! candidates, and it's meaningless). |

Case labels are *values*, not identifiers, so they give no name evidence.

### 6.2 Enum `switch`

*Shape (VERIFIED, javac 26):* for an enum **outside the current top-level class**, javac
creates a synthetic holder class `Outer$N`, reusing an existing synthetic empty class if there is one
(`outerCacheClass()`). The holder has `static final int[] $SwitchMap$<enum flat name with . and / → $>`
(Lower.java `RuntimeEnumMapping`). Its `<clinit>` does `values().length`, then for each case, in
first-use order, `try { map[E.X.ordinal()] = k; } catch (NoSuchFieldError) {}`. For an enum
**declared in the same top-level class**, javac 26 switches directly on `ordinal()`
(`CompileTimeEnumMapping`, verified with `n/src/N.java`). Older javac always used `$SwitchMap$`
(INFERRED: the compile-time path is recent). kotlinc's analogue is `…$WhenMappings` with
`$EnumSwitchMapping$<i>` (INFERRED, not tested here).

*After R8 (VERIFIED):*
- If the enum is unboxable, it is unboxed. The switch map collapses to an int array in
  `…$EnumUnboxingSharedUtility` (`a.a`), and the enum's constant names are lost with it. That's
  the enum-unboxing row in DESIGN §1.
- With `-keep enum p.Color`, the holder survives as `a.a` with one field `a:[I`. The
  `<clinit>` still has `ordinal()` calls and a `catches` table. The field name `$SwitchMap$p$Color`
  and the class name `Low$1` are minified.

| Rule | Class | Preconditions | Evidence |
|---|---|---|---|
| `javac/switchmap-recognize` | **S** (structure) | The class has only static `int[]` fields and `<clinit>`. Each field is initialized with `new int[E.values().length]` and filled only by `map[E.X.ordinal()] = const` stores, each guarded by `NoSuchFieldError`. The fields are read only as `map[x.ordinal()]` feeding a switch. | This is exactly the javac or kotlinc template. No hand-written code has this shape in practice, but it's still a structural claim, so gate it behind the exact template. |
| `javac/switchmap-resugar` | **D** | recognized, and the enum class is in the APK (closed world) | Rewrite `switch(map[e.ordinal()])` into a switch on the enum constant. The holder can then be deleted. Semantically equivalent once R8 has fixed the ordinals. |
| `javac/switchmap-field-name` | **S** if the enum FQN is S; else **D** | recognized, and `javac` is proven as the producer (the enum isn't a Kotlin class and the outer class isn't Kotlin; see ambiguity) | javac's name is a pure function of the enum's flat name: `$SwitchMap$` + flatname.replace('.', '$'). Kotlin's `$EnumSwitchMapping$i` depends on `i`, the first-use index in the file, so it's **D**. |
| `javac/switchmap-holder-name` | **D** | — | `Outer$N`. `N` is javac's anonymous-class counter and "Outer" is whichever top-level class first switched. R8 may have merged or moved it. Emit `SwitchMap_{hash}`. N enumeration is possible in principle (`$1..$(k+1)`, k = surviving anonymous classes of Outer), but Outer itself isn't provable, so don't. |

*Ambiguity:* the javac holder and the kotlinc `WhenMappings` holder are structurally
near-identical. Distinguish them only when kotlin metadata or an intrinsics use on the *user* of
the map proves the language.

### 6.3 String concatenation

*Shape (VERIFIED):* `--release 8` gives `new StringBuilder().append(..)…toString()`, with
`append(Object)` for objects. `--release ≥9` gives `invokedynamic makeConcatWithConstants`
with recipe `"User(name=\u0001, age=\u0001, o=\u0001)"` (in the BootstrapMethods).

*D8 (VERIFIED):* D8 desugars indy concat for dex into `String.valueOf(obj)`, hoisted **before**
`new StringBuilder(firstConst)`, followed by `append(String)` for the converted args. From
javac-8 `StringBuilder` code, D8 produces `new StringBuilder("User(name=")` + `append(Object)`.
The two are distinguishable **in D8 output**. That's only a version-keyed hint of the javac target,
and it's useless for names anyway.

*R8 (VERIFIED):* the outputs from both javac targets are identical. R8 also **constant-folds
across inlining**: the call `concat("n", 1, l)` became `new StringBuilder("User(name=n, age=1, o=")`.
So a `toString`-style template may be *fused with call-site constants*. **This is a precondition
for the data-class/record `toString` S rule in DESIGN §1 and §5 (item 4):** each literal piece must match the
generator template exactly (`"Cls(" f1 "="`, `", " fN "="`, `")"`), and every hole must be fed by a
field read. A fused literal like `"User(name=n, age=1, o="` must be rejected, or split only at holes that
are proven. Otherwise call-site values (`n`, `1`) get parsed as identifiers.

| Rule | Class | Notes |
|---|---|---|
| `javac/concat-resugar` | **D** | Fold a `StringBuilder` chain with only `append` + a final `toString`, where the builder doesn't escape, into canonical `a + b + …`. Whether the original was indy, `StringBuilder`, or hand-written is ✗N, so it's never claimed. |

### 6.4 Synthetic accessors (`access$NNN`) and nest bridges

*Shape (VERIFIED):* `--release 8` gives `static access$000(Low)` (read `secret`),
`access$002(Low,int)` (write `secret`), and `access$100()` (call `hidden()`). The name is
`"access$" + anum + acode/10 + acode%10` (Lower.java `accessName`). `anum` is the per-class
index of the accessed symbol in lowering traversal order. `acode` is the access kind:
DEREF=0, ASSIGN=2, PREINC=4, PREDEC=6, POSTINC=8, POSTDEC=10, then compound-assign codes. With
`--release ≥11` (nestmates) there are no accessors. D8/R8 then synthesize
`-$$Nest$fget<field>`, `-$$Nest$fput<field>`, `-$$Nest$sm<method>`, and `-$$Nest$m<method>`
(VERIFIED in D8 `--debug` output: `-$$Nest$fgetsecret`, `-$$Nest$fputsecret`, `-$$Nest$smhidden`).

*After R8 (VERIFIED):* all accessors were inlined, and the two javac targets produced identical dex.
If an accessor survives, it is renamed.

| Rule | Class | Notes |
|---|---|---|
| `javac/accessor-inline` | **D** | Static, synthetic (or minified), single-use-kind trampoline to a private member of a class in the same nest: inline it at call sites and delete it. That's pure code motion. It's the same rule as DESIGN's "Nest-access bridges" row. |
| `javac/accessor-name` | ✗N → D | Recovering `access$NNN`: the `acode` digits are determined by the body shape (S-able), but `anum` depends on traversal order. Also, a javac-8-vs-nestmate origin can't be told apart after R8. Don't emit it. |
| `d8/nest-bridge-name` | **S (target name)** | Only in **non-R8 (D8-only)** dex: `-$$Nest$fget<name>` contains the original field name. When the bridge is present but the target field *was* renamed (D8 then a separate obfuscator, which is rare), the embedded name is S evidence for the field. In R8 output the bridge name is minified, so this doesn't apply. |

### 6.5 `assert`

*Shape (VERIFIED):* the `static final boolean $assertionsDisabled` field is initialized in
`<clinit>` from `Low.class.desiredAssertionStatus()`. For interfaces it goes in a synthetic
holder class. The check is `getstatic $assertionsDisabled; ifne; <cond>; new AssertionError(msg)`.

*D8/R8 (VERIFIED):* **D8 removes the assertion code by default, in both release *and* `--debug`
mode.** `check(I)V` became `return-void`, `<clinit>` became `return-void`, and the field is kept.
R8 removes the field too. The `"x must be positive"` message string disappears. So `assert` is
**not recoverable**: it's D·id. The only exception is builds made with `--force-enable-assertions`,
where the shape survives and can be re-sugared (**D**, `javac/assert-resugar`). Kotlin's
`assert()` is a stdlib call (`_Assertions.ENABLED`), not this shape.

### 6.6 try-with-resources

*Shape (VERIFIED, javac 26 at both targets):* the `close()` is inlined on the normal path, and the
exceptional path has a `catch Throwable t` that calls `close()` in a nested try whose catch calls
`t.addSuppressed(t2)`, then rethrows. javac 9–10 emitted a synthetic
`$closeResource(Throwable, AutoCloseable)` helper instead (INFERRED, JDK-8194978; javac 26 at
`--release 8` does not). **The shape survives R8 intact (VERIFIED).** On API < 19 D8/R8
backport `addSuppressed` (out of scope here; see desugaring).

| Rule | Class | Notes |
|---|---|---|
| `javac/twr-resugar` | **D** | The exact normal-close plus exceptional-close-with-`addSuppressed` shape folds to `try (R r = …) {}`. It's semantically identical, but a hand-written equivalent is indistinguishable, so it's not S. Kotlin `use {}` has a different shape (`kotlin.io.CloseableKt.closeFinally`) and is a separate rule. |

### 6.7 Fixtures (javac)

- `javac-lowerings/Low.java` above, compiled at `--release 8` and `--release 17`. Assert that
  the R8 outputs are equal. That's a regression test documenting that these origins are
  indistinguishable, so no rule may claim S about them.
- A string switch with a hash collision (`"Aa"`/`"BB"`): resugar gives the canonical sorted order.
  α-test: rename every minified identifier and check that the output is unchanged.
- A cross-file enum switch with `-keep enum`, the same with an unboxable enum, and a nested enum
  (compile-time mapping). The expected outcomes are: holder recognized; unboxing path; no holder.
- Concat with constant-propagated arguments (the `concat("n",1,l)` case). It's the negative fixture for the
  data-class `toString` S rule: the fused literal `"User(name=n, age=1, o="` must **not**
  yield field names from the folded values (`n`, `1`).

## 7. Ambiguity summary: proof vs hint

| String / structure | Proof of | Grade | Broken by |
|---|---|---|---|
| Room `"<table>(<canonical FQN>).\n Expected:\n"` | entity FQN | **S** (N only on the `$`-split without nesting attributes) | nothing (compile-time element name) |
| Moshi `Util.unexpectedNull/missingProperty(arg1, …)` | Kotlin property name | **S** (N on trailing `_`) | NameAllocator `_` suffixing only |
| Moshi `moshi.adapter(…, "prop")` | property name | **S** | — |
| Moshi `"GeneratedJsonAdapter(<N>)"` | model simple name | **S** | fused constants (check the template) |
| ViewBinding `findChildViewById(root, CONST)` + arsc | field name | **S** | arsc name collapsing / AndResGuard |
| ViewBinding `inflate(CONST)` + arsc | binding simple name | **S** (package N) | DataBinding `<data class=>` |
| Safe Args bundle keys + nav XML | Args/Directions/accessor names | **S** (N if the argument sets tie) | — |
| Parcelize write/read order | field↔ctor-param↔slot bijection | **S** (links) | — |
| Kotlin `checkNotNullParameter` + Parcelize slot link | property name | **S** | nullable and primitive types have no string |
| Hilt template members / `Hilt_X` | names, given a manifest-kept X | **S** | — |
| Dagger `X_Factory`, `X_MembersInjector` | names, given an S X | **S** | horizontal merging (split first) |
| Room column names, `INSERT … (\`col\`)` | field names | hint | `@ColumnInfo(name=)`, `@Embedded(prefix=)` |
| Moshi `Options.of`, `writer.name` | property names | hint | `@Json(name=)` |
| Gson `@SerializedName`, kotlinx `addElement` | field names | hint | the annotation *is* the override |
| Dagger `@LazyClassKey` strings | nothing (minified by `-identifiernamestring`) | link only | — |
| Room `@Entity(tableName)` string | class simple name | hint | `tableName=` |
| javac lowerings | no names | structure (D) | R8 erases the javac version |

## 8. Ranking: S-grade name recovery per unit of effort

The ranking weighs prevalence in real apps, the number of S names per app, and implementation cost.

1. **Moshi codegen.** Adapters and models are already kept (anchors for free). The `Util.*` first
   argument gives S property names even under `@Json(name=)`, and the recognizer is a single library-call
   match plus a `const-string`. The effort is low and there are many names per model. The caveat
   (trailing `_`) has a closed, exhaustive candidate set.
2. **Room entity FQN.** One regex over `const-string`s gives the **full original FQN of every
   entity class**, which are otherwise renamed. The link to the DEX class is via insert-adapter SQL.
   Effort is very low, the payoff per app is modest (a handful of classes), and entities are commonly
   Kotlin data classes whose `toString` then yields S field names too.
3. **ViewBinding (+ Safe Args).** This is extremely prevalent, with dozens of S field names and binding
   class names per app. It needs resources.arsc parsing and nav-XML parsing (which 8R wants anyway for
   manifest and layout keeps), plus inline-fragment matching, because R8 inlines `bind()`. The effort is
   medium. Safe Args reuses the same arsc/XML plumbing.
4. **Parcelize.** Very prevalent, but it yields **links, not names**. Combined with Kotlin intrinsics
   it names non-null reference properties, and it gives `$Creator` class names once the outer class is S.
   The effort is medium, because the recognizer must survive merged creators and unboxed enums.
5. **Hilt/Dagger.** Very prevalent, but there's **no string proof at all** post-2.51. The value is in
   propagation: manifest-kept Activities and Application → `Hilt_*`, template member names, and
   component-impl names. Most factories are inlined or merged, so there's little to name. The effort is
   high (library identification, merge splitting) for a moderate gain. The legacy Hilt <2.51 FQN keys are a
   cheap win where present.
6. **Gson / kotlinx.serialization / Room columns.** Hints only. They feed `{hint}` of D names.
7. **javac lowerings, KAE, Anvil.** No S names. Re-sugaring (string/enum switch, TWR, accessor
   inlining) improves readability as **D** only. R8 erases the javac-version signal, and it strips
   `assert` in both D8 and R8 by default.
