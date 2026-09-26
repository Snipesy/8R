package com.example.app

import androidx.compose.runtime.*

class Holder(var count: Int)
data class Point(val x: Int, val y: Int)
class Wrapper<T>(val value: T)

@Composable
fun Greeting(name: String, times: Int = 1, enabled: Boolean = true) {
    val s = remember { mutableStateOf(0) }
    if (enabled) {
        Label("Hello $name ${s.value} $times")
    } else {
        Label("bye")
    }
}

@Composable
fun Label(text: String) {
    println(text)
}

@Composable
fun Column(content: @Composable () -> Unit) {
    content()
}

@Composable
fun Screen(items: List<String>, holder: Holder, p: Point) {
    Column { Greeting("a") }
    Column { Label(holder.count.toString() + p.x) }
    for (i in items) {
        key(i) { Label(i) }
    }
    val r = rememberDouble(items.size)
    Label(r.toString() + readOnly())
    NonRestart(3)
    InlineRow { Label("inline") }
}

@Composable
fun rememberDouble(n: Int): Int = remember(n) { n * 2 }

@ReadOnlyComposable
@Composable
fun readOnly(): Int = 3

@NonRestartableComposable
@Composable
fun NonRestart(x: Int) {
    Label(x.toString())
}

@Composable
inline fun InlineRow(content: @Composable () -> Unit) {
    content()
}

@Composable
fun Many(
    a1: Int, a2: Int, a3: Int, a4: Int, a5: Int, a6: Int,
    a7: Int, a8: Int, a9: Int, a10: Int, a11: Int = 11, a12: String = "x",
) {
    Label("$a1$a2$a3$a4$a5$a6$a7$a8$a9$a10$a11$a12")
}

class Screens {
    @Composable
    fun Member(p: Point, w: Wrapper<String>) {
        Label("${p.y}${w.value}")
        Many(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)
    }
}

@Composable
fun App(holder: Holder) {
    Screen(listOf("x", "y"), holder, Point(1, 2))
    Screens().Member(Point(3, 4), Wrapper("w"))
    Many(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, "z")
}
