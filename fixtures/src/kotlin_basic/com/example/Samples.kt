@file:JvmName("SamplesFacade")
package com.example

import kotlin.properties.Delegates

data class User(val name: String, val age: Int, val tags: List<String>? = null)

@JvmInline value class UserId(val raw: Long)
@JvmInline value class Email(val s: String)

fun lookup(id: UserId): String = "u" + id.raw
fun lookupBoth(id: UserId, e: Email, n: Int): String = "" + id.raw + e.s + n
fun makeId(x: Long): UserId = UserId(x)

class Repo {
    lateinit var db: String
    val lazyName: String by lazy { "lazy" + db }
    var observed: Int by Delegates.observable(0) { _, o, n -> println("$o->$n") }
    fun find(id: UserId): String = db + id.raw

    fun greet(who: String, times: Int = 2, loud: Boolean = false): String =
        (if (loud) who.uppercase() else who).repeat(times)

    @JvmOverloads fun over(a: String, b: Int = 1, c: Long = 2L): String = "$a$b$c"

    suspend fun load(key: String): String {
        val a = fetch(key)
        val n = key.length
        val b = fetch(a + n)
        return a + b + n
    }
    private suspend fun fetch(k: String): String { kotlinx(); return k }
    private suspend fun kotlinx() {}

    companion object {
        const val TAG = "Repo"
        @JvmStatic fun create(): Repo = Repo()
        @JvmField val shared: Repo = Repo()
        fun plain(): Int = 3
    }
}

enum class Color(val rgb: Int) { RED(1), GREEN(2), BLUE(3) }

sealed class Shape { data class Circle(val r: Double) : Shape(); object Square : Shape() }

fun describe(c: Color): String = when (c) { Color.RED -> "r"; Color.GREEN -> "g"; Color.BLUE -> "b" }
fun parse(s: String): Int = when (s) { "alpha" -> 1; "beta" -> 2; "gamma" -> 3; else -> 0 }
fun area(s: Shape): Double = when (s) { is Shape.Circle -> s.r * s.r; Shape.Square -> 1.0 }

inline fun <T> measure(block: () -> T): T { val t = System.nanoTime(); val r = block(); println(System.nanoTime() - t); return r }
fun useInline(xs: List<Int>): Int = measure { xs.map { it * 2 }.sum() }

fun lambdas(xs: List<String>): List<() -> Int> = xs.map { s -> { s.length } }
fun fnRef(): (String) -> Int = String::length
fun suspendLambda(): suspend (Int) -> String = { x -> "v$x" }
fun template(u: User, n: Int) = "User ${u.name} is $n years"
fun Repo.ext(x: String?): Int = x!!.length
fun notNullParam(a: String, b: List<String>): Int = a.length + b.size

object Singleton { var counter = 0; fun inc() = ++counter }
