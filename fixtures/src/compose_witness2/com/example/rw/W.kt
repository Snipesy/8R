package com.example.rw

// M2 review witnesses: real int flags in single-bit tests (not $default), inlined
// non-restartable callees whose masks mention caller slots (not the caller's arity),
// and ComposableSingletons fields whose naming depends on the compiler, not the skip check.

import androidx.compose.runtime.*
import kotlin.coroutines.EmptyCoroutineContext

val log = StringBuilder()
fun out(s: String) { log.append(s).append('\n') }

// W1: a real Int flags param used only in single-bit tests selecting a param's value.
@NonSkippableComposable @Composable
fun Flags(flags: Int, a: String) {
    val s = if (flags and 1 != 0) a else "none"
    out("flags $s")
}
@NonSkippableComposable @Composable
fun Flags2(a: String, flags: Int) {
    var s = "none"
    if (flags and 4 == 0) s = a
    out("flags2 $s")
}

// W2: non-restartable callee inlined (single caller) whose intrinsic remember masks the caller-derived $changed.
@NonRestartableComposable @Composable
fun Inner(x: Int, y: Int, z: Int) {
    val v = remember(z) { z * 2 }
    out("inner $x $y $v")
}
@Composable fun Outer(a: Int) { Inner(a, a + 1, a + 2) }

@NonRestartableComposable @Composable
fun Inner2(x: Int, y: Int, z: Int, w: Int) {
    val v = remember(w) { w * 3 }
    out("inner2 $x $y $z $v")
}
@Composable fun Outer2(a: Int, b: Int) { Inner2(a, b, a + 7, log.length) }

@NonRestartableComposable @Composable
fun Inner3(x: Int, y: Int, z: Int, w: Int) {
    val v = remember(x, w) { x + w * 3 }
    out("inner3 $x $y $z $v")
}
@Composable fun Outer3(a: Int) { Inner3(a, 5, 6, a) }

@Composable
fun App(n: Int) {
    Flags(n, "a$n"); Flags2("b$n", n)
    Item(listOf(U(n)), n); Outer(n); Outer2(n, n + 1); Outer3(n); Holder(n)
}

class UnitApplier : AbstractApplier<Unit>(Unit) {
    override fun insertTopDown(index: Int, instance: Unit) {}
    override fun insertBottomUp(index: Int, instance: Unit) {}
    override fun remove(index: Int, count: Int) {}
    override fun move(from: Int, to: Int, count: Int) {}
    override fun onClear() {}
}

object Main {
    @JvmStatic
    fun main(args: Array<String>) {
        val c = Composition(UnitApplier(), Recomposer(EmptyCoroutineContext))
        c.setContent { App(args.size) }
        print(log)
    }
}

class U(var v: Int)
@Composable fun Item(items: List<U>, style: Int) {
    val shown = if (style and 2 != 0) items else emptyList()
    out("item ${shown.size}")
}

// Composable lambdas held in ComposableSingletons fields (named lambda-N before Kotlin 2.1.20,
// lambda$K after, whatever the skip check).
@Composable fun Box(n: Int, content: @Composable () -> Unit) { out("box $n"); content() }
@Composable fun Holder(n: Int) {
    Box(n) { out("one") }
    Box(n + 1) { out("two") }
    Box(n + 2) { Flags(3, "z") }
}
