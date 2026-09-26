package com.example.witness

// Review witnesses for compose/runtime-api (M1): unstable params compared with changedInstance
// (strong skipping), and defaults whose expressions make no composer call (a getDefaultsInvalid
// branch that looks like a skip check).

import androidx.compose.runtime.*
import kotlin.coroutines.EmptyCoroutineContext

val log = StringBuilder()
fun out(s: String) { log.append(s).append('\n') }

// Unstable class: strong skipping compares it with changedInstance.
class U(var v: Int)

@ReadOnlyComposable @Composable fun ro(x: Int): Int = x * 3 + log.length


@Composable fun A1(u: U) { out("a1 ${u.v}") }
@Composable fun A2(u: U, w: U) { out("a2 ${u.v} ${w.v}") }
@Composable fun A3(u: U) { out("a3 ${u.v}") }
@Composable fun A4(u: U, w: U) { out("a4 ${u.v}${w.v}") }
@Composable fun A5(u: U) { out("a5 ${u.v}") }
@Composable fun A6(u: U, w: U, z: U) { out("a6 ${u.v}${w.v}${z.v}") }
@Composable fun A7(u: U) { out("a7 ${u.v}") }
@Composable fun A8(u: U, w: U) { out("a8 ${u.v}${w.v}") }
@Composable fun S1(s: String) { out("s1 $s") }

// Composable-dependent default, always defaulted (R8 may fold $default).
@Composable fun D1(y: Int = remember { 40 } + 2) { out("d1 $y") }
@Composable fun D2(y: String = remember { "q" } + "r") { out("d2 $y") }
@Composable fun D3(y: Int = remember { 7 }) { out("d3 $y") }

@Composable fun E1(a: Int, y: Int = remember { 40 } + 2) { out("e1 $a $y") }
@Composable fun E2(a: Int, y: String = remember { "q" } + "r") { out("e2 $a $y") }
@Composable fun E3(a: Int, y: Int = remember { 7 } * a) { out("e3 $a $y") }

@Composable fun F1(a: Int, y: Int = ro(1) + a) { out("f1 $a $y") }
@Composable fun F2(a: Int, y: Int = ro(2) + a) { out("f2 $a $y") }
@Composable fun F3(a: Int, y: Int = ro(3) + a) { out("f3 $a $y") }
@Composable fun F4(a: Int, y: Int = ro(4) + a) { out("f4 $a $y") }
@Composable fun F5(a: Int, y: Int = ro(5) + a) { out("f5 $a $y") }
@Composable fun F6(a: Int, y: Int = ro(6) + a) { out("f6 $a $y") }
@Composable fun F7(a: Int, y: Int = ro(7) + a) { out("f7 $a $y") }
@Composable fun F8(a: Int, y: Int = ro(8) + a) { out("f8 $a $y") }
@Composable fun F9(a: Int, y: Int = ro(9) + a) { out("f9 $a $y") }
@Composable fun F10(a: Int, y: Int = ro(10) + a) { out("f10 $a $y") }
@Composable fun F11(a: Int, y: Int = ro(11) + a) { out("f11 $a $y") }
@Composable fun F12(a: Int, y: Int = ro(12) + a) { out("f12 $a $y") }

@Composable
fun App(n: Int) {
    F1(n); F2(n); F3(n); F4(n); F5(n); F6(n); F7(n); F8(n); F9(n); F10(n); F11(n); F12(n)
    E1(n); E2(n); E3(n)
    val u = U(n)
    A1(u); A2(u, u); A3(u); A4(u, u); A5(u); A6(u, u, u); A7(u); A8(u, u)
    S1("x$n")
    D1(); D2(); D3()
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
