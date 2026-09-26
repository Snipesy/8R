package com.example

// Inputs flow from args so R8 can't constant-fold the interesting shapes away.
fun main(args: Array<String>) {
    val r = Repo.create(); r.db = "x" + args.size
    println(User("a", args.size)); println(User("a", 1).copy(age = args.size)); println(lookup(UserId(args.size.toLong())))
    println(lookupBoth(UserId(1), Email("e" + args.size), 2))
    println(r.lazyName); r.observed = args.size; println(r.greet("hi")); println(r.over("a")); println(describe(Color.entries[args.size % 3]))
    println(parse(args.firstOrNull() ?: "beta")); println(area(if (args.isEmpty()) Shape.Circle(2.0) else Shape.Square))
    println(useInline(listOf(1, args.size))); println(lambdas(listOf("ab"))[0]()); println(fnRef()("abc"))
    println(template(User("b", 2), args.size)); println(r.ext("q")); println(notNullParam("a", listOf())); println(Singleton.inc())
    println(Repo.shared); println(Repo.plain()); println(r.find(makeId(4)))
    println(Color.entries.size); println(suspendLambda()); println(r::load)
}
