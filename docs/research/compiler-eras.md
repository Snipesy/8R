# Compose compiler eras (main-session experiment, 2026-09-25)

Source: fixtures/src/compose_shapes (committed 10c9d54), compiled with the Compose compiler plugin
bundled in Kotlin 2.1.21, 2.2.21, 2.3.10, 2.4.20 (2.0.21 couldn't run on JDK 26), runtime 1.10.6,
sourceInformation=true. Scripts: $CLAUDE_JOB_DIR/tmp/kc/cmp.py.

- Durable entry keys (startRestartGroup const) are IDENTICAL across all four compiler versions for
  all 13 restartable composables: the key depends only on the source signature/path.
  Formula from compose.md §1.5 reproduced exactly (e.g. fun-Defaults(Int,String,Int)Unit/
  pkg-com.example.shapes/file-Shapes.kt = 0xdf96e8a4).
- All 15 entry keys survive R8 9.4.24 (with runtime consumer rules) as constants.
- Era markers (positive evidence per method):
  - 2.1.x: skipping via Composer.getSkipping(); param info `P(i)`; composable lambdas as anonymous
    classes (ComposableSingletons$FileKt$lambda$K$1); non-restartable composables wrapped in
    startReplaceGroup(fnKey).
  - >= 2.2.x: Composer.shouldExecute(ZI)Z; param info `N(names)` ALREADY in 2.2.21 (compose.md says
    >=2.3.0: correct it); lambdas via invokedynamic with `lambda__K$lambda$0` static bodies;
    non-restartable composables have no group of their own (markers only).
  - ComposableSingletons field `lambda$K` and getter `getLambda$K$<module>` in all four.
- R8 9.4.24 on compose_shapes: removes most composable params (Ten/Eleven/Constants/Remembers/
  Lambdas -> (ILComposer;)V), reorders $changed before the Composer, merges the Composer interface
  into ComposerImpl, merges restart lambdas into lambda groups, inlines non-restartable composables.
- 8R today: Composer and its methods get structural names (m_d1e8 = startRestartGroup, m_2d3f =
  shouldExecute, m_a15a/m_aab2 = rememberedValue/updateRememberedValue, m_7311 = endRestartGroup,
  obj_7d8b = the update-scope field, m_edb0 = updateChangedFlags). These roles are provable from
  the plugin's code shape: candidate S names for the runtime API, the highest-value regen step.
