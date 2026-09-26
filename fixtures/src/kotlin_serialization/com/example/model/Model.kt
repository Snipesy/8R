package com.example.model

import kotlinx.serialization.*

@Serializable
data class User(
    val id: Int,
    @SerialName("user_name") val name: String,
    val email: String? = null,
    @Transient val cache: String = "x",
    @OptIn(ExperimentalSerializationApi::class) @EncodeDefault val tags: List<String> = emptyList(),
    @Required val flag: Boolean = false,
    val kind: Color = Color.RED,
) {
    var bodyProp: Long = 5
}

@Serializable @SerialName("acct")
class Account(val owner: User, val balance: Double, val history: Map<String, List<Int>> = mapOf())

@Serializable
sealed class Shape {
    @Serializable @SerialName("circle") data class Circle(val r: Double) : Shape()
    @Serializable data class Rect(val w: Int, val h: Int) : Shape()
}

@Serializable object Singleton

@Serializable enum class Color { RED, @SerialName("verde") GREEN, BLUE }
@Serializable enum class Plain { A, B }

@Serializable
data class Big(
    val f0: Int = 0,
    val f1: Int,
    val f2: Int,
    val f3: Int,
    val f4: Int,
    val f5: Int = 5,
    val f6: Int,
    val f7: Int,
    val f8: Int,
    val f9: Int,
    val f10: Int = 10,
    val f11: Int,
    val f12: Int,
    val f13: Int,
    val f14: Int,
    val f15: Int = 15,
    val f16: Int,
    val f17: Int,
    val f18: Int,
    val f19: Int,
    val f20: Int = 20,
    val f21: Int,
    val f22: Int,
    val f23: Int,
    val f24: Int,
    val f25: Int = 25,
    val f26: Int,
    val f27: Int,
    val f28: Int,
    val f29: Int,
    val f30: Int = 30,
    val f31: Int,
    val f32: Int,
    val f33: Int
)

@Serializable data class Box<T>(val value: T, val label: String)

@Serializable class Named(val q: Int) { companion object Factory { fun make() = Named(1) } }

@Serializable @JvmInline value class Wrapper(val v: String)

@Serializable open class Base(val baseProp: Int)
@Serializable class Derived(val d: String) : Base(1)

@Serializable class Holder(val w: Wrapper, val s: Shape, val b: Box<Int>, val p: Plain, val o: Singleton, val d: Derived, val n: Named, val a: Account, val big: Big)
