package com.example.app

import androidx.compose.runtime.*
import kotlin.coroutines.EmptyCoroutineContext

class UnitApplier : AbstractApplier<Unit>(Unit) {
    override fun insertTopDown(index: Int, instance: Unit) {}
    override fun insertBottomUp(index: Int, instance: Unit) {}
    override fun remove(index: Int, count: Int) {}
    override fun move(from: Int, to: Int, count: Int) {}
    override fun onClear() {}
}

object Entry {
    @JvmStatic
    fun start() {
        val r = Recomposer(EmptyCoroutineContext)
        val c = Composition(UnitApplier(), r)
        c.setContent { AppScreen("hello") }
    }
}
