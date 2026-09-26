# 8R plan: Compose + library signature DB (+ other SDKs)

Research (all in $CLAUDE_JOB_DIR/tmp/research/): compose-keys.md, compose-app.md, sigdb.md,
sdk-anchors.md, compiler-eras.md. Headline numbers are quoted per milestone. Every milestone
ends with a review agent and a Gretio smoke test (scripts/smoke-apk.sh), like Phases 1–4.

## M0 Groundwork (small, first)
- compose.md corrections: N() already in 2.2.x; K1→K2 key change for composable-lambda params;
  receiver takes the LAST $changed slot; R8 9.4 discards startRestartGroup's result and turns
  updateScope into an iput; updateChangedFlags never inlined; defaults chosen by a phi, not a
  store into the param; `x & 1` ambiguity ($default bit 0 vs force bit); non-restartable
  composables keep no key after R8; restart lambdas are `Outer$N`, merged with lambda bodies.
- Oracle: verify eightr-mapping against two R8 9.4 quirks (fake outermost frame with a
  package-relative class for moved/bridged methods; `residualsignature` once per method but
  applying to all its ranges) — fix `outermost_methods()` if affected.
- Structural Compose detection in sources.rs (today it looks for the `Composer` class name,
  renamed by R8, so Gretio shows no Compose).
- Fixtures: compose_shapes (done, sourceInformation=true); add compose_lib (material3/foundation
  composables, pinned versions) and a sigdb app (coroutines/okhttp/okio/collection/stdlib at
  pinned versions), both with mains for the ART exec harness.

## M1 Compose runtime API by role (S) — highest value per effort
- Identify the Composer class structurally (Gretio: `bl4`, 552 hits vs 15) and name its members by
  the plugin's call shapes: startRestartGroup, endRestartGroup, shouldExecute / getSkipping,
  skipToGroupEnd, startReplaceGroup, startMovableGroup, endReplaceGroup, rememberedValue,
  updateRememberedValue, changed(*), plus ScopeUpdateScope.updateScope (field on 9.4),
  updateChangedFlags, ComposableLambdaImpl / rememberComposableLambda, Composer.Empty.
- Every composable body then reads like the compiler's output.
- Harness: graded against fixture mappings (compose_basic*, compose_shapes, compose_lib).

## M2 App composables: undo the compiler plugin (annotative)
- compose/restart-lambda + synthetic-param-names (S): the restart lambda (all 552 on Gretio)
  proves $composer/$changed; $default by single-bit tests (95 on Gretio); 0 role errors on 201
  key-verified composables. Names go into debug-info parameter names.
- compose/param-slot + default-bit binding + arity bound (D): places 46% of residual params at
  their exact original index, proves removals (109 methods), extension-receiver count.
- `@eightr.Composable(key = K, defaults = …, arity ≥ n)` build annotation (reuses @Inlined plumbing).
- ComposableSingletons `lambda$K` field names (152 on Gretio). No code simplification.
- App composable *names* stay unrecoverable (no strings survive; keys only verify candidates).
- Harness: roles/params vs compose_shapes D8 ground truth (N(...) strings) and mappings.

## M3 Library signature DB infrastructure (`cargo xtask sigdb`)
- Fetch pinned Maven artifacts (list + sha in fixtures/sigdb.conf), run each library version
  through the pinned R8 alone with its consumer rules + "keep public API"; the mapping labels the
  residual shapes. Store per original method: version bitset + one record per distinct body
  (strict body hash, string-set hash, similarity sketch); per class a shape hash.
- Deterministic, versioned DB files (content-addressed; the DB hash goes in the report).
  Size: ~76–430 KB compressed per library version.

## M4 Compose library composables by key
- DB of restartable entry keys (K1 and K2 variants) from the last patch of each minor
  (compose 1.5–1.12, material3 1.1–1.4: ~7k keys, ~100 KB). No collisions among 1,241 keys.
- Match: K is the constant argument of the method's first startRestartGroup-shaped call, |K| ≥ 2^20,
  unique in the DB, order-free shape check (param-count upper bound, ≥1 int, reference types a
  sub-multiset). S when a second key from the same function corroborates; else D.
- Names the method (not its host class: R8 moves them). Version by key-set voting (resolves to the
  minor). Gretio: 167 library composables named (material3 108, foundation 51, …), ~2·10⁻⁴ FP.

## M5 General library matcher
- Seeds: exact, unique on both sides, non-trivial bodies (strict body hash, then string sets).
- Propagation to a fixpoint over call graph and within identified classes; a round accepts only
  pairs without competing proposals (α-invariant; random-rename test identical).
- Runs after the rewrites (outlines, merge split, re-boxing). Tiers (all D): strong (98–100%) /
  medium (~89%); N candidate sets only when ≤ 8. Class names need ≥2 matched members and a 2/3 vote.
- Expected: ~94% precision / ~59% recall on library methods; cross-version −2 recall.
- Harness: per-stage precision/recall graded by fixture mappings; cross-version matrix; ratchet.

## M6 Version evidence (S facts about libraries)
- META-INF/*.version (122 on Gretio), root *.properties (30), Firebase registrar id→version, embedded
  version strings (`okhttp/4.11.0`, …) → report.libraries; used to pick DB versions first.

## M7 SDK extractors (after Compose), in Gretio-value order
1. protobuf-lite: kept field names + FIELD_NUMBER constants matched to the APK's bundled .proto
   files (S message names; 28/70 unique on Gretio).
2. kotlinx.serialization: serial-name descriptors bound structurally to the class's data-class
   toString (S class/property names; 266 descriptors on Gretio).
3. Room: validation-string entity FQNs (S; 9 on Gretio).
4. (Kotlin runtime, Firebase, Ktor, RevenueCat come mostly from the M5 matcher.)

## Order
M0 → M1 → M2 → M3 → M4 → M6 → M5 → M7. M1/M2 need no DB; M4 needs only the key DB (M3 infra
subset); M5 is the largest.
