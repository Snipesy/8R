@file:OptIn(ExperimentalSerializationApi::class)
package com.example
import com.example.model.*
import kotlinx.serialization.*
import kotlinx.serialization.descriptors.*
import kotlinx.serialization.encoding.*
import kotlinx.serialization.modules.*
import kotlinx.serialization.builtins.serializer

class ListEncoder : AbstractEncoder() {
    val list = ArrayList<Any?>()
    override val serializersModule: SerializersModule = EmptySerializersModule()
    override fun encodeValue(value: Any) { list.add(value) }
    override fun encodeNull() { list.add(null) }
    override fun encodeNotNullMark() { list.add(true) }
    override fun beginCollection(descriptor: SerialDescriptor, collectionSize: Int): CompositeEncoder { list.add(collectionSize); return this }
}
var SEQ = false
class ListDecoder(val list: ArrayDeque<Any?>, var n: Int = 0) : AbstractDecoder() {
    private var idx = 0
    override val serializersModule: SerializersModule = EmptySerializersModule()
    override fun decodeValue(): Any = list.removeFirst()!!
    override fun decodeElementIndex(descriptor: SerialDescriptor): Int { if (idx == n) return CompositeDecoder.DECODE_DONE; return idx++ }
    override fun beginStructure(descriptor: SerialDescriptor): CompositeDecoder = ListDecoder(list, descriptor.elementsCount)
    override fun decodeSequentially(): Boolean = SEQ
    override fun decodeCollectionSize(descriptor: SerialDescriptor): Int = (list.removeFirst() as Int).also { n = it }
    override fun decodeNotNullMark(): Boolean = list.removeFirst() != null
}
fun <T> roundtrip(s: KSerializer<T>, v: T): T {
    val e = ListEncoder(); e.encodeSerializableValue(s, v); println(e.list)
    return ListDecoder(ArrayDeque(e.list)).decodeSerializableValue(s)
}
fun main(args: Array<String>) {
    SEQ = args.size > 3
    val u = User(args.size, "n", null, "c", listOf("a"), true, Color.GREEN)
    println(roundtrip(User.serializer(), u))
    println(roundtrip(Account.serializer(), Account(u, 1.0)).owner)
    println(roundtrip(Shape.serializer(), Shape.Rect(1, 2)))
    println(roundtrip(Big.serializer(), Big(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34)))
    println(roundtrip(Box.serializer(String.serializer()), Box("v", "l")))
    println(roundtrip(Named.serializer(), Named.make()).q)
    println(roundtrip(Wrapper.serializer(), Wrapper("w")))
    println(roundtrip(Derived.serializer(), Derived("d")).baseProp)
    println(roundtrip(Plain.serializer(), Plain.B))
    println(roundtrip(Singleton.serializer(), Singleton))
    println(Holder.serializer().descriptor)
    val t: java.lang.reflect.Type = if (args.size > 7) Account::class.java else User::class.java
    println(serializer(t).descriptor)
}
