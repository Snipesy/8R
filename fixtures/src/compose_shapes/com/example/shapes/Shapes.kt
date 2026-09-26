package com.example.shapes

import androidx.compose.runtime.*

val log = StringBuilder()

fun out(s: String) {
    log.append(s).append('\n')
}

// F1: minimal restartable composable.
@Composable
fun Min() {}

// F2: $changed1 boundary (10 vs 11 params) and a member (this param).
@Composable
fun Ten(a1: Int, a2: Int, a3: Int, a4: Int, a5: Int, a6: Int, a7: Int, a8: Int, a9: Int, a10: Int) {
    out("ten ${a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10}")
}

@Composable
fun Eleven(a1: Int, a2: Int, a3: Int, a4: Int, a5: Int, a6: Int, a7: Int, a8: Int, a9: Int, a10: Int, a11: String) {
    out("eleven ${a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10} $a11")
}

class Card(val title: String) {
    @Composable
    fun Show(times: Int, suffix: String = "!") {
        repeat(times) { out("$title$suffix") }
    }
}

// F4: static vs dynamic defaults.
@Composable
fun Defaults(x: Int = 0, label: String = "d", y: Int = remember { 40 } + 2) {
    out("defaults $x $label $y")
}

// F7: non-restartable, read-only, non-Unit return.
@NonRestartableComposable
@Composable
fun NonRestart(x: Int) {
    out("nonrestart $x")
}

@ReadOnlyComposable
@Composable
fun readOnlyValue(): Int = 7

@Composable
fun computed(n: Int): String {
    val s = remember(n) { "c$n" }
    return s + readOnlyValue()
}

// F9: composable lambdas: non-capturing, capturing, sibling calls to the same callee.
@Composable
fun Slot(content: @Composable () -> Unit) {
    out("slot{")
    content()
    out("}")
}

@Composable
fun WithArg(content: @Composable (Int) -> Unit) {
    content(5)
}

@Composable
fun Lambdas(tag: String) {
    Slot { out("static") }
    Slot { out("captured $tag") }
    WithArg { n -> out("arg $n $tag") }
}

// F11: control flow.
@Composable
fun Flow(items: List<String>, mode: Int) {
    if (mode > 1) {
        out("big")
    } else {
        out("small")
    }
    when (mode) {
        0 -> Min()
        1 -> out("one")
        else -> NonRestart(mode)
    }
    for (i in items) {
        key(i) { out("item $i") }
    }
    if (items.isEmpty()) return
    out("after")
}

// F12: remember variants.
@Composable
fun Remembers(a: Int, b: String, c: Long) {
    val r0 = remember { 1 }
    val r1 = remember(a) { a * 2 }
    val r3 = remember(a, b, c) { "$a$b$c" }
    val st = remember { mutableStateOf(3) }
    out("remember $r0 $r1 $r3 ${st.value}")
}

// F14: R8 param removal: called once with constants, and an unused param.
@Composable
fun Constants(a: Int, b: String, unused: Int) {
    out("constants $a $b")
}

// F19: open composable with defaults.
open class Base {
    @Composable
    open fun Render(depth: Int = 1) {
        out("base $depth")
    }
}

class Derived : Base() {
    @Composable
    override fun Render(depth: Int) {
        out("derived $depth")
    }
}

// F21: composable property.
val greeting: String
    @Composable get() = remember { "hi" } + readOnlyValue()

@Composable
fun App(items: List<String>) {
    Min()
    Ten(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)
    Eleven(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, "x")
    Card("card").Show(2)
    Defaults()
    Defaults(1, "e")
    NonRestart(4)
    out(computed(3))
    Lambdas("t")
    Flow(items, items.size)
    Flow(emptyList(), 0)
    Remembers(2, "b", 3L)
    Constants(9, "k", 0)
    Base().Render()
    Derived().Render(2)
    out(greeting)
}
