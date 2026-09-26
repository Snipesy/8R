# SDK anchors survey: Gretio (10804000.apk)

APK facts: 2 dex files, **16,083 classes, 4.32M insn code units**. Only **2,130 classes (13%) keep a
package name**. The other 13,953 were repackaged into the default package with 1-3 character names.
`source_file_idx` is `r8-map-id-f1bea1f2…` on every class, so there are no SourceFile hints. Built
with AGP 9.4.1 (`META-INF/com/android/build/gradle/app-metadata.properties`). Git revision
`e4764f9d…` is in `META-INF/version-control-info.textproto`.

Scratch artifacts are in `research/sdk-anchors/`:
- `classes.jsonl`: per-class strings, refs, fields and methods, parsed from `dexdump -d`
- `m2/`: 125 Maven artifacts
- `m2index.json`: string index
- `seeds.json`, `labels.json`: attribution
- `share.tsv`, `anchors.json`
- scripts: `parse.py`, `idx.py`, `attr.py`, `prop.py`, `anchors.py`, `ver.py`

**Headline findings**
1. **Library identity and version are almost free.** 122 AndroidX/coroutines `.version` files,
   30 GMS/Firebase `.properties` files, Firebase registrar version literals, and User-Agent strings
   cover most of the app. For the rest, **string-set version bracketing** against Maven artifacts
   works: for example Ktor client is about 3.6.0, kotlinx.serialization-json is 1.11.0, and Lottie
   is 6.7.1.
2. **A string-only signature DB already covers most of the code.** 58k library-unique strings from
   125 artifacts directly seed **42% of classes**. One-hop graph propagation raises that to **94%**
   (share estimates below).
3. **S-grade anchors that survive in this app:**
   - protobuf-lite **kept field names** plus `*_FIELD_NUMBER` constants: 70 messages, 213 fields.
     With the **bundled `google/firestore/**.proto` files**, this names message classes.
   - Room entity FQNs: 9.
   - Data-class `toString` templates: 1,050.
   - Enum constants: about 3,150 in 500 enums.
   - `kotlin.Metadata` kept on 900 classes.
   - kept `@Serializable` annotation (`Liw9;` with `with()`) on 466 classes.
   - Firebase registrars (manifest-kept).
   - `.kotlin_module` facade names.
4. **Kotlin null-check strings are absent.** There are 0 `checkNotNullParameter` or
   `"Parameter specified as non-null"` strings and 0 `"getX(...)"` strings. DESIGN §5 rule 3 yields
   nothing on this app, and data-class `toString` becomes the main source of property names.
5. **Compose `sourceInformation` is stripped**: only 2 `C(` strings.

---

## 1. Inventory and version evidence

### 1a. Files outside the dex
| Source | Content |
|---|---|
| `META-INF/*.version` (122) | All AndroidX artifacts plus `kotlinx_coroutines_{core,android,play_services,slf4j}=1.11.0`. Examples: compose.* 1.12.1, material3 1.4.0, material-icons-{core,extended} 1.7.8, activity 1.13.0, core 1.19.1, lifecycle 2.11.0, navigation3 1.2.0, navigationevent 1.1.1, room3-runtime **3.0.3**, sqlite 2.7.1, datastore 1.1.7, credentials 1.6.0-beta01, security-crypto 1.1.0, webkit 1.16.0, window 1.5.0, material (MDC) 1.14.0, constraintlayout 2.2.2 / -compose 1.1.0, privacysandbox ads 1.1.0-beta11, databinding viewbinding 9.4.1. `arch.core_core-runtime.version` = `task` (a known AndroidX bug). Full list in `versions.tsv`. |
| root `*.properties` (30) | `version=` for billing 8.3.0, core-common 2.0.4, firebase-analytics 23.2.0, firebase-auth 24.2.0, firebase-auth-interop 20.0.0, firebase-database-collection 18.0.1, firebase-encoders 17.0.0, firebase-encoders-proto 16.0.0, firebase-measurement-connector 20.0.1, googleid 1.2.0, integrity 1.4.0, play-services-{basement,base} 18.9.0, tasks 18.4.0, auth 22.0.0, auth-base 18.0.10, auth-api-phone 18.0.2, auth-blockstore 16.4.0, fido 21.0.0, identity-credentials 16.0.0-alpha08, location 19.0.0, measurement* 23.2.0, ads-identifier 18.0.0, places-placereport 17.0.0, stats 17.0.2, recaptcha 18.6.1 |
| `META-INF/*.kotlin_module` | `okhttp.kotlin_module` (okhttp3.internal facades HostnamesKt, Util, TaskLoggerKt, DatesKt, HttpHeaders, …) and `com.surrealdev.gretiompp_Shared.kotlin_module` (GmPidConversionsKt, FireModulesKt, Parcelable_androidKt, StandardsKt). Original names; every listed facade is also a kept class here. |
| `META-INF/services/*` (10) | Rewritten to minified names (`uz3 → q14 au3 hj5`, `nb6 → bt9 hn7 q88`). They are still structural interface→impl links. One file names a kept class: `ly7 → com.revenuecat…PaywallAssetWarmerImpl`. |
| `META-INF/native-image/io.ktor/ktor-network/reflect-config.json` | Proves ktor-network is present. It names `InterestSuspensionsMap` and its 4 fields, which R8 kept for reflective atomics. |
| `META-INF/org/jetbrains/kotlinx/kotlinx-serialization-core-jvm/verification.properties` | Identifies the artifact only. |
| Java resources | `google/{protobuf,api,rpc,type,firestore/v1,firebase/firestore/proto,…}/*.proto` (74 files: Firestore + gRPC); `client_analytics.proto` (datatransport cct); `com/google/i18n/phonenumbers/data/*` (libphonenumber); `org/commonmark/internal/util/entities.properties`; `okhttp3/internal/publicsuffix/publicsuffixes.gz`; `DebugProbesKt.bin` (coroutines); `kotlin/**/*.kotlin_builtins` (stdlib; includes `concurrent/atomics`, so ≥2.1.20); `natives/*/jssc*` (jSSC serial, KMP desktop leftovers) |
| `lib/*/` | Only `libandroidx.graphics.path.so` and `libdatastore_shared_counter.so` |
| assets | `dexopt/baseline.prof{,m}` (no names), `vin_tables.sqlite`, `www/*` (Ktor-served pages), `AppstoreAuthenticationKey.pem` |

### 1b. Version strings in the dex
| Library | Evidence (exact) |
|---|---|
| Firebase (registrars, kept by manifest `ComponentDiscoveryService` meta-data) | `FirebaseCommonRegistrar`: `"fire-core"`, `"22.2.1"`. `AnalyticsConnectorRegistrar`: `"fire-analytics"`, `"23.2.0"`. `FirebaseAppCheck{,Debug,PlayIntegrity}Registrar`: `"fire-app-check*"`, `"19.4.1"`. `FirebaseAuthRegistrar`: `"fire-auth"`, `"24.2.0"`. `CrashlyticsRegistrar`: `"fire-cls"`, `"20.1.1"`. `TransportRegistrar`: `"fire-transport"`, `"19.0.0"`. `FirestoreRegistrar`: `"fire-fst"`, `"26.6.0"`. `FirebaseInstallationsRegistrar`: `"fire-installations"`, `"19.1.2"`. `FirebaseSessionsRegistrar`: `"fire-sessions"`, `"3.0.8"`. `StorageRegistrar`: `"fire-gcs"`, `"22.0.2"`. Both strings sit in one `getComponents()` method. |
| Firestore / gRPC | `"Firestore (26.6.0) ran out of memory…"`, `" fire/26.6.0 grpc/"`, `"grpc-java-okhttp/1.62.2"` |
| Crashlytics / Sessions | `"Crashlytics Android SDK/20.1.1"`, `"Initializing Firebase Crashlytics 20.1.1 for "`, `"Initializing Firebase Sessions 3.0.8."`, `", sessionSdkVersion=3.0.8, osVersion="` |
| datatransport | `"datatransport/3.3.0 android/"` (transport-runtime UA) |
| Firebase Storage | `"Android/22.0.2"` |
| FirebaseUI | `"FirebaseUI Compose Registrar initialized: LIBRARY_NAME: firebase-ui-android, VERSION_NAME: 10.0.0-beta05"` (kept `com.firebase.ui.auth.FirebaseUIComposeRegistrar`) |
| OkHttp | `"okhttp/4.11.0"` (all 317 okhttp3 classes kept by name) |
| RevenueCat | `"10.23.0"` next to `"SDK Version - %s"` and in the platform-info ctor (`"android"`, `"10.23.0"`). Headers `X-Client-Version`, `X-RevenueCat-*`. |
| slf4j | `"2.0.99"` (REQUESTED_API_VERSION) and `"2.0"`, so slf4j-api 2.0.x |
| Lottie | `"Lottie only supports bodymovin >= 4.4.0"` (identity). Version from the string bracket below. |
| Kotlin compiler | `kotlin.Metadata.mv` on kept classes: app `{2,4,0}` (Kotlin 2.4); libraries `{1,6,0}`, `{1,8,0}`, `{2,0,0}`, `{2,1,0}`, … |
| App firmware | `"4.5.1"`, `"5.6.23"` (OBDLink firmware, app-specific) |

### 1c. Version bracketing by string sets (method used by `ver.py`)
For each version, count (a) strings unique to that version that appear in the dex and (b) strings
from other versions that are absent from that version but present in the dex.

| Library | Result |
|---|---|
| ktor-client-core | 3.1.3: 44 misses … 3.5.2: 2 … **3.6.0: 2/14 own-only present, 1 miss**. Honest bracket: 3.5.3–3.6.x. |
| ktor-server-core | **3.4.0: 18/29 own-only present, 0 misses**; older versions have 18-21 misses. So ≥3.4.0; newer versions were not tested. |
| kotlinx-serialization-json | **1.11.0: 11/16 own-only present, 0 misses** |
| kotlinx-serialization-core | ≥1.10.0 (consistent with 1.11.0) |
| Lottie | **6.7.1** (3/3 own-only present, 0 misses) |
| zxing core | ≥3.5.3 (3.5.3 and 3.5.4 not separable) |
| coil-base 2.x, commonmark, libphonenumber | Not separable with string sets. These need method fingerprints. |

This works because R8 keeps almost every `const-string`. Coverage is not complete: unused code is
removed, and constants get folded or fused.

### 1d. Bundled SDKs (identified by version file, string or kept name)
**Kotlin:**
- stdlib (≥2.1.20)
- kotlin-reflect (**full**, about 822 classes)
- kotlinx-coroutines 1.11.0
- kotlinx-serialization 1.11.0 (core+json)
- kotlinx-io
- DebugProbes

**AndroidX:** Compose 1.12.1, Material3 1.4.0, material-icons-extended 1.7.8, plus the full AndroidX
list from §1a.

**Google:**
- Firebase:
  - Firestore 26.6.0 (+ gRPC 1.62.2, protobuf-javalite)
  - Auth 24.2.0
  - App Check 19.4.1 (+debug, play-integrity)
  - Crashlytics 20.1.1
  - Sessions 3.0.8
  - Installations 19.1.2
  - Storage 22.0.2
  - datatransport 3.3.0 / 19.0.0
  - Analytics / measurement 23.2.0
- GMS base 18.9.0 / tasks, auth, fido, identity-credentials, location
- reCAPTCHA 18.6.1
- Play Integrity 1.4.0
- Play Billing 8.3.0
- Tink (via security-crypto; `com.google.crypto.tink.shaded.protobuf` strings)
- libphonenumber, zxing ≥3.5.3

**Third party:**
- RevenueCat purchases + purchases-ui 10.23.0 (Amazon/Galaxy strings present)
- FirebaseUI Auth (Compose) 10.0.0-beta05
- OkHttp 4.11.0 + Okio 3.x
- Ktor client (CIO/OkHttp, 3.5.3–3.6.x) and **Ktor server** (CIO, ≥3.4.0; `"Started Ktor on "`, `KtorServer.*` app classes)
- Coil 2.x (`coil#…` keys)
- Lottie 6.7.1
- androidsvg 1.4
- commonmark
- MPAndroidChart (227 classes kept by name)
- slf4j 2.0.x, jSSC (natives only)
- Conscrypt reflection strings only (not bundled)

Not present:
- Retrofit, Gson, Moshi, Dagger/Hilt, Koin, Glide
- Room's classic `androidx.room` (it is `room3`)

---

## 2. Surviving name-carrying anchors per SDK

### 2a. Global counts (whole app)
| Anchor | Count | Notes |
|---|---|---|
| Classes with a package name | 2,130 | okhttp3 317, recaptcha 563 (`zz*`, pre-obfuscated), app `com/surrealdev` 422, MPAndroidChart 227, MDC 106, play-integrity 95, webkit boundary 65, GMS 66, appcompat 42, revenuecat-ui 81 |
| Data-class `toString` heads `Name(prop=` | **1,050** in 1,046 classes; 3,241 `", x="` pieces | RevenueCat 300 + UI 192, Compose 211, app ≈149, reflect 26, FirebaseUI 25, material3 20, Firestore 14, ktor 27, Sessions 13 |
| Enum classes / constant strings | **500** (471 minified) / about 3,150 | RevenueCat 509, material3 378, reflect 301, measurement 236, gRPC 227, Firestore 135, recaptcha 163 |
| kxs `$serializer` (static `descriptor` field kept + FQN serial name) | **266** (**611 + 567** element names for RevenueCat + app) | RevenueCat 146, app 108 (`com.surrealdev.gretio.mpp…`), FirebaseUI 5, Firebase 5, `androidx.savedstate.serialization` 1, `io.ktor.util.date.GMTDate` 1 |
| `@Serializable` kept as runtime annotation `Liw9;` (only element: `with`) | **466** classes (338 minified) | Marks every serializable class structurally. `La33;` (119, marker annotation) is on `$serializer` classes. |
| `kotlin.Metadata` kept | **900** classes (mv 2.4.0: 413, 1.6.0: 314, 2.1.0: 115) | Mostly on named app/okhttp/RevenueCat views. R8 rewrote them; they carry structure, not names. |
| protobuf-lite messages (kept `xxx_` fields + `XXX_FIELD_NUMBER` + `DEFAULT_INSTANCE`/`PARSER`) | **70** classes (those with `*_FIELD_NUMBER` constants), **213** `_` fields | Firestore 50 (159 fields), Tink 16 (48 fields), other 4 (6 fields). One more class has `_` fields but no FIELD_NUMBER constants and is excluded. |
| SQL strings | Room app DB 9 entities + measurement 95 + Firestore 87 + datatransport 32 | Room: `"adapters(com.surrealdev.max.db.entities.AdapterEntity).\n Expected:\n"` × 9, identity hash `df6760e4…`, 9 `CREATE TABLE` with column lists |
| Dotted-FQN strings | about 1,500 | RevenueCat 258, app 144, reflect 138, credentials 133, Firebase Auth 98, measurement 89 |
| Kotlin null-check strings | **0** | Intrinsics param assertions are stripped. `"(...)"` strings: 0. `lateinit` messages: none. |
| Compose `sourceInformation` | 2 | Stripped in release |
| Material icon names `Filled.X` | 52 | About 50 icon facades survive out of 11k |

### 2b. Per-SDK anchor catalogue
- **kotlinx.serialization runtime.** The runtime templates from kotlinx-serialization.md §4 Tier C are
  present. 13 runtime classes keep names (`KSerializer`, `SerialDescriptor`, `Encoder`, `Decoder`,
  `Json*$Companion`). The plugin output across app, RevenueCat and FirebaseUI is 266 descriptors.
  - **Cross-check (name-level only, no structural binding yet):** the last segment of a kxs FQN
    also appears as a data-class `toString` head for:
    - RevenueCat: **142/146**
    - FirebaseUI: 5/5
    - Firebase: 5/5
    - app: **29/108**. Many app serializables are kept by name (spojo) or are objects/enums/sealed
      parents without `toString`.
  - Example: `…paywalls.components.properties.CornerRadiuses.Percentage` has a head `Percentage(`.
    So `kxs/class-simple-name` can be S at scale once each head is bound structurally to the
    serializable class.
- **Protobuf-lite (Firestore, Tink, datatransport).**
  - `GeneratedMessageLite` subclasses keep **all field names** (protobuf's consumer rule keeps
    `<fields>`). The `*_FIELD_NUMBER` constants and the `RawMessageInfo` info string (encodes field
    numbers and types) survive too.
  - **Field names are S** (kept, not generated). They are also the protoc name function of the
    `.proto` field (`camel(field)+"_"`).
  - Matching kept field sets against the **74 bundled `.proto` files** gives a crude unique match
    for **28/70** messages. Examples:
    - `bx3 = google.firestore.v1.ExistenceFilter`
    - `ena = StructuredQuery.Order`
    - `iz5 = google.type.LatLng`
    - `lcc = firestore.client.WriteBatch`
  - 9 are ambiguous, for example `MapValue` / `Projection` / `Struct`. The info-string types and
    field numbers would split most of them.
  - The protoc naming function makes the Java class name `<outer>.<Message>`. That is S when the
    match is unique and the `.proto` has no `java_outer_classname`/`java_package` override
    ambiguity (those options are in the file).
  - 16 Tink messages have no bundled `.proto` and need the sigdb.
- **Firebase.**
  - Registrars are kept by name, and each `getComponents()` pairs a `"fire-*"` id with a version
    literal.
  - Component names (`"fire-sessions-component"`) and `LibraryVersionComponent` UA pieces
    (`device-name`, `android-target-sdk`, …) are present.
  - Crashlytics, Sessions and datatransport use firebase-encoders `FieldDescriptor.of("sdkVersion")`
    style field-name strings: about 70 / 53 / 41 camelCase keys. These give AutoValue property
    names as **hints**, overridable by `@Encodable.Field(name=)`.
  - Sessions and Crashlytics kxs/data classes add 13 `toString` heads.
  - Firebase Auth / measurement internals are **pre-obfuscated upstream** (`zza…`), so recovery ends
    at shipped names.
- **GMS measurement.** 95 SQL strings (table/column DDL), `ALTER TABLE` migrations and 236 enum
  constants. Names upstream are `zz*`, so the useful result is identification and collapse, not
  renaming.
- **RevenueCat.**
  - This is the largest non-Compose SDK.
  - It has 146 kxs descriptors with full FQNs and 611 element names, 492 `toString` heads, and 509
    enum constants.
  - Its error enum names (`PurchaseCancelledError`, …), log templates and `"SDK Version - %s"` are
    present.
  - It keeps 83 names (views, `PaywallAssetWarmerImpl`, `ProxyAmazonBillingActivity`) and 314
    kotlin.Metadata with `mv 1.6.0`.
  - Most of its names come out of the **generic** kxs + `toString` extractors without an SDK-specific
    one.
- **Ktor client/server.**
  - Plugin, AttributeKey and phase names are strings: 108 client + 22 server camelCase names, such
    as `"HttpSend"`, `"HttpTimeoutCapability"`, `"ResponseAdapterAttributeKey"`,
    `"IgnoreTrailingSlash"` and `"ApplicationPluginRegistry"`.
  - These usually equal the Kotlin `val`/object name (`createClientPlugin("HttpTimeout")`), so they
    are **D hints**. The strings are also the sigdb seeds.
  - The data-class heads (`URLProtocol(name=`, `Closed(cause=`) give S simple names.
  - `native-image` JSON names 4 fields.
- **Compose.** 19 kept names, 211 `toString` heads, 474 camelCase strings (VectorDrawable attrs,
  `"InfiniteTransition"` labels), 59 FQNs. No `sourceInformation`. docs/sources/compose.md covers
  the key/`$changed` structure.
- **kotlin-reflect (full copy).** 138 FQN strings (`kotlin.collections.MutableList`, JVM descriptors
  like `getBinaryClasses$descriptors_jvm()Ljava/util/Map;`), 301 enum constants, 26 `toString`
  heads. It is probably pulled in by the kept `kotlin.Metadata` + reflection use. Every Kotlin app
  that uses reflection carries these ~800 classes, so it is a good sigdb target.
- **kotlinx-coroutines.** Named debug strings (`"DefaultDispatcher"`, `"CancellableContinuation"`,
  `"onCancellationImplDoNotCall"`), symbol names (`"CLOSED"`, `"NO_DECISION"`, …) and
  `DebugProbesKt.bin`. No classes keep names.
- **Room3 (app DB).** Validation strings give **9 S entity FQNs** (`com.surrealdev.max.db.entities.*`)
  and column lists (D hints). `MultiInstanceInvalidationService` is kept (manifest).
- **Credentials / GMS identity.** 133 FQN strings (`androidx.credentials.*` bundle keys, provider
  class names used reflectively), for example
  `"androidx.credentials.playservices.CredentialProviderPlayServicesImpl"`. These are reflection
  targets, so S where the class is kept.
- **Relative Java-resource loads.** okhttp `PublicSuffixDatabase` (`publicsuffixes.gz`), commonmark
  `entities.properties`, libphonenumber `/com/google/i18n/phonenumbers/data/…`. Absolute paths
  identify the library. A relative `getResourceAsStream` would prove the loader's original package,
  but R8 keeps those names here anyway.
- **Log tags / exception messages.** Every SDK has them. They are the main mass of the 58k-string
  index. Examples:
  - `"Okio Watchdog"`
  - `"Room Invalidation Tracker Refresh"`
  - Other library-specific log/exception strings.
- **R8-synthesized `Enum.valueOf` (new rule candidate `r8/unboxed-enum-valueof-fqn`).** There are 4
  strings of the form `"No enum constant <FQN>."`:
  - `com.caverock.androidsvg.SVG.Unit`
  - `…SVG.GradientSpread`
  - `com.google.zxing.qrcode.decoder.ErrorCorrectionLevel`
  - `com.google.firebase.appcheck.internal.StorageHelper.TokenType`

  None of these enums keeps its name (they are minified or unboxed). The strings come from R8, not
  javac: R8 rewrote `valueOf(String)` into an inline string switch (`"pad".equals`,
  `"reflect".equals`, …) that ends in `"No enum constant <FQN>.".concat(name)`, followed by a throw
  helper and `"Name is null"`.
  - The FQN is the **original canonical name**, so it is α-clean. It is bound structurally to the
    enum through the switch arms, which equal the enum `<clinit>` constant names.
  - The simple name and package are S up to the canonical-name `.` vs `$` nesting split. That split
    is an enumerable N set, like `kxs/fqn-split`, and is S when the InnerClasses or outer structure
    is known.

---

## 3. Share of code and anchor strength

### 3a. Attribution method
1. **Index.** Take string constants from 125 Maven artifacts at the proven versions (`m2/`,
   including indy-concat recipe pieces). Keep strings of length ≥6 that belong to exactly one
   library group, where the kotlin-stdlib and kotlin-reflect labels yield to any specific library.
   This gives **58,180 unique strings**.
2. **Seed.** Seed each dex class with its majority unique-string label, or with its package for
   named classes. That labels 6,716 classes. A further 328 classes get "unindexed-string" seeds.
3. **Propagate.** Run up to 8 rounds over super/interface/outer edges (weight 5) and ref edges
   (weight 1, damped by √degree), requiring a 2× margin. This labels 15,159/16,083 classes.
4. **Measure.** Share is by insn units.

This is α-clean in spirit (no minified names used), but it is rough. **Attribution is by strings only.** Framework-API call-pattern fingerprints (the other half of §5.4) were not built; that work is deferred. The app is under-counted,
because app classes that only reference libraries get pulled toward their neighbours. The
**"APP?"** bucket mixes app code with unindexed AndroidX (activity, navigationevent,
ui-tooling-data) and unindexed GMS/protobuf pieces (for example `com.google.protobuf.UnsafeUtil` and
`ISmsRetrieverApiService` strings).

### 3b. Share table (top 30 of 62)
| SDK | seeded classes | classes after propagation | insns | share |
|---|---|---|---|---|
| Compose (runtime/ui/foundation/animation/material) | 634 | 2,155 | 563k | **13.0%** |
| RevenueCat core | 776 | 1,486 | 391k | **9.1%** |
| APP? (unindexed strings: app + unindexed androidx) | 328 | 882 | 248k | 5.7% |
| GMS measurement (Analytics) | 228 | 627 | 240k | 5.5% |
| kotlin-reflect | 328 | 822 | 227k | 5.3% |
| RevenueCat UI | 364 | 680 | 183k | 4.2% |
| APP (surrealdev strings or names) | 464 | 638 | 159k | 3.7% |
| FirebaseUI Auth | 115 | 309 | 152k | 3.5% |
| Material Components (views) | 146 | 348 | 150k | 3.5% |
| ConstraintLayout (+core) | 84 | 195 | 141k | 3.3% |
| Firebase Auth | 177 | 557 | 135k | 3.1% |
| Firestore | 205 | 419 | 124k | 2.9% |
| reCAPTCHA | 569 | 657 | 98k | 2.3% |
| Compose Material3 | 125 | 326 | 89k | 2.1% |
| Ktor client | 186 | 451 | 89k | 2.0% |
| gRPC | 159 | 325 | 83k | 1.9% |
| kotlinx.serialization | 77 | 242 | 65k | 1.5% |
| AppCompat | 82 | 196 | 65k | 1.5% |
| MPAndroidChart | 227 | 233 | 62k | 1.4% |
| GMS auth/fido/identity | 60 | 146 | 52k | 1.2% |
| Lottie | 56 | 165 | 49k | 1.1% |
| kotlinx-coroutines | 95 | 219 | 49k | 1.1% |
| Crashlytics | 108 | 201 | 49k | 1.1% |
| Tink | 40 | 136 | 46k | 1.1% |
| GMS base/basement/tasks | 129 | 232 | 44k | 1.0% |
| OkHttp | 321 | 347 | 41k | 0.9% |
| Billing | 55 | 167 | 37k | 0.9% |
| androidsvg | 33 | 90 | 37k | 0.9% |
| Ktor server | 71 | 171 | 35k | 0.8% |
| Credentials | 38 | 95 | 33k | 0.8% |
| … unlabelled | | 924 | 163k | 3.8% |

**Rollups:**
- Compose family: about **15.6%**. Adding FirebaseUI and RevenueCat-UI, which are themselves
  Compose, brings Compose-based code to about 23%.
- RevenueCat: **13.3%**.
- Firebase + GMS + reCAPTCHA + Integrity + Billing: about **24%**. About 13 points of that are
  upstream-obfuscated `zz*` (measurement, Auth internals, GMS, reCAPTCHA, billing).
- Kotlin runtime family (reflect, serialization, coroutines, stdlib, io): about **8.3%**. Stdlib is
  under-counted because it has few strings.
- Ktor: 2.8%.
- View-system AndroidX + MDC + ConstraintLayout: about 9%.
- App: about 9-13%.

### 3c. Anchor strength per SDK
| SDK | Anchor strength | Why |
|---|---|---|
| Firestore (+ protobuf) | **Very high** | Kept proto field names (S), bundled `.proto` (message names S on unique match), version, registrar, 87 SQL, 135 enum constants, exception templates |
| RevenueCat | **Very high** | 146 kxs FQNs + 492 `toString` + 509 enum constants. Names cross-validate, so S simple names at scale. |
| App (kxs, Room, `toString`) | **High** | 108 kxs FQNs + app `toString`; 9 Room FQNs (S); kept spojo classes and `.kotlin_module` |
| kotlinx.serialization / coroutines / stdlib / reflect | High (identification) | Unique runtime messages and FQN strings; version via strings. Names come only from the sigdb (D). |
| Ktor | Medium-high | Plugin/key names (hints), `toString` heads (S), version bracket |
| Compose | Medium | `toString`, VectorDrawable attrs, keys; no sourceInformation (compose.md) |
| Crashlytics / Sessions / datatransport | Medium | Encoder field-name keys (hints), registrars, versions |
| GMS measurement / Auth / base, reCAPTCHA, Billing, Integrity | Identification high, recovery low | Upstream `zz*` names. At best "library X, shipped name zzab". |
| OkHttp, MPAndroidChart, Integrity, webkit | n/a | Already kept by name, so S for free. An extractor adds nothing here. |

---

## 4. Recommendations (after Compose)

1. **Generic string-set sigdb core** (DESIGN §5.4, first slice).
   - Index `const-string` sets per class/method from Maven artifacts. Seed by unique strings and
     propagate over hierarchy and ref edges.
   - Version-bracket from the same index.
   - On this app, strings alone seed 42% and reach 94% after propagation, with no fingerprints yet.
     That gives library **identification (D hints / "this is library X")** for everything,
     including zz-obfuscated SDKs, which should be labelled and collapsed rather than renamed.
   - Output grade: D hint. A candidate list is N only when it is exhaustive over the DB, and it
     never is, because unindexed code exists.
2. **Protobuf-lite extractor.**
   - Cheap, and S-grade for field names (already kept) and message class names (unique match of
     field set + numbers + types against `.proto` files shipped in the APK, or a DB of well-known
     protos).
   - It applies to every Firebase-Firestore app, every Tink/security-crypto app, DataStore-proto
     apps and gRPC apps.
3. **kotlinx.serialization + data-class `toString` cross-binding** (already specified in
   kotlinx-serialization.md). On this app it is the single biggest source of S names:
   - 266 descriptors × `toString` heads (name-level agreement: 142/146 for RevenueCat, 29/108 for the app)
   - kept `@Serializable` `Liw9;` marks 466 classes
   - Intrinsics strings are absent, so `toString` is *the* Tier-A source here.
4. **Kotlin runtime family** (stdlib, coroutines, serialization runtime, reflect): universal across
   Kotlin apps, rich unique messages, 8%+ of code. Build fingerprints here first.
5. **Firebase open-source SDKs** (Firestore, Crashlytics, Sessions, Installations, common/components,
   datatransport):
   - registrar/version facts (S facts about identity and version)
   - encoder field-name keys (hints)
   - SQL schemas (hints)
6. **Room (room3 too)**: the §8 #2 rule from other-plugins works here as-is (9 S FQNs).
7. **Later**: Ktor (plugin-name hints), RevenueCat-specific (mostly covered by 1 and 3), Lottie/Coil.
   Deprioritize GMS/measurement/reCAPTCHA/Billing name recovery; identify them only.

**S-grade anchors** (exact original names, compiler/library/R8-emitted, structurally bound):
- kept names (manifest / consumer rules / app rules)
- protobuf-lite kept fields
- proto message names (on unique `.proto` match)
- Room validation FQNs
- data-class `toString` (unfused-template check)
- enum `<clinit>` names
- R8-synthesized `"No enum constant <FQN>."` in rewritten `valueOf` (4 here; bound via the switch arms = enum constants; `.`/`$` split is N)
- `.kotlin_module` facade names (when bound to a class)
- Firebase registrar id→version (fact about the library, not a name)

**D hints:**
- kxs serial names (unless cross-bound)
- Room columns
- Firebase encoder keys
- Ktor plugin/AttributeKey names
- log tags
- sigdb matches
