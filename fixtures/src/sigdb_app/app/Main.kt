package app

import androidx.collection.*
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.withPermit
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import okio.Buffer
import okio.ByteString.Companion.encodeUtf8
import okio.GzipSink
import okio.GzipSource
import okio.buffer
import java.util.concurrent.TimeUnit

data class User(val id: Int, val name: String, val tags: List<String>)

class Repo(private val client: OkHttpClient) {
    private val cache = LruCache<String, User>(64)
    private val mutex = Mutex()
    private val state = MutableStateFlow(0)
    val events = MutableSharedFlow<String>(replay = 2, extraBufferCapacity = 8)

    suspend fun load(id: Int): User = mutex.withLock {
        cache.get("u$id") ?: User(id, "user$id", listOf("a", "b")).also { cache.put("u$id", it); state.value = state.value + 1 }
    }

    fun request(path: String): Request {
        val url = "https://example.com/api/".toHttpUrl().newBuilder().addPathSegment(path).addQueryParameter("q", "x y").build()
        val body = "{\"a\":1}".toRequestBody("application/json; charset=utf-8".toMediaType())
        return Request.Builder().url(url).header("X-Trace", path).post(body).cacheControl(CacheControl.FORCE_NETWORK).build()
    }

    fun formAndMultipart(): List<RequestBody> {
        val form = FormBody.Builder().add("k", "v").addEncoded("e", "%20").build()
        val multi = MultipartBody.Builder().setType(MultipartBody.FORM)
            .addFormDataPart("f", "v")
            .addFormDataPart("file", "a.txt", "hello".toRequestBody("text/plain".toMediaType()))
            .build()
        return listOf(form, multi)
    }

    fun execute(req: Request): String? = try {
        client.newCall(req).execute().use { r -> r.body?.string() + r.headers["Date"] + r.code }
    } catch (e: java.io.IOException) {
        null
    }

    fun enqueue(req: Request) {
        client.newCall(req).enqueue(object : Callback {
            override fun onFailure(call: Call, e: java.io.IOException) { println("fail $e") }
            override fun onResponse(call: Call, response: Response) { response.close() }
        })
    }

    fun observe(): Flow<String> = state.map { "n=$it" }.distinctUntilChanged()
}

fun buildClient(): OkHttpClient = OkHttpClient.Builder()
    .connectTimeout(3, TimeUnit.SECONDS)
    .readTimeout(5, TimeUnit.SECONDS)
    .retryOnConnectionFailure(true)
    .addInterceptor(Interceptor { chain -> chain.proceed(chain.request().newBuilder().header("UA", "app").build()) })
    .cookieJar(CookieJar.NO_COOKIES)
    .certificatePinner(CertificatePinner.Builder().add("example.com", "sha256/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").build())
    .connectionPool(ConnectionPool(5, 1, TimeUnit.MINUTES))
    .build()

fun okioStuff(): String {
    val buf = Buffer()
    val gz = GzipSink(buf).buffer()
    gz.writeUtf8("hello world ".repeat(20)); gz.close()
    val src = GzipSource(buf).buffer()
    val s = src.readUtf8()
    val bs = s.encodeUtf8()
    return bs.sha256().hex() + bs.base64() + bs.md5().hex().length + Headers.headersOf("A", "b", "C", "d").toString() +
        Cookie.parse("https://example.com/".toHttpUrl(), "sid=1; Path=/; HttpOnly")
}

fun collections(): String {
    val am = ArrayMap<String, Int>(); am["a"] = 1; am["b"] = 2
    val sam = SimpleArrayMap<Int, String>(); sam.put(1, "x"); sam.put(2, "y")
    val set = ArraySet<String>(); set.add("q"); set.add("r")
    val sp = SparseArrayCompat<String>(); sp.put(7, "seven"); sp.append(9, "nine")
    val lsp = LongSparseArray<String>(); lsp.put(7L, "L7")
    val sm = mutableScatterMapOf<String, Int>(); sm["x"] = 4; sm.getOrPut("y") { 5 }
    val il = mutableIntListOf(3, 1, 2); il.add(5); il.sort()
    val oim = mutableObjectIntMapOf<String>(); oim["z"] = 9
    val lset = mutableLongSetOf(1L, 2L, 3L); lset.remove(2L)
    val ims = MutableIntIntMap(); ims[1] = 2; ims.put(3, 4)
    val cq = CircularArray<String>(); cq.addFirst("a"); cq.addLast("b")
    var acc = 0
    sm.forEach { _, v -> acc += v }
    oim.forEach { _, v -> acc += v }
    return "$am $sam $set ${sp.get(7)} ${lsp[7L]} $sm $il $oim $lset ${ims[1]} ${cq.popFirst()} $acc ${am.containsValue(2)} ${set.indexOf("r")}"
}

suspend fun flows(repo: Repo): List<String> = coroutineScope {
    val ch = Channel<Int>(Channel.BUFFERED)
    val prod = launch { for (i in 1..5) ch.send(i); ch.close() }
    val out = mutableListOf<String>()
    for (x in ch) out.add("c$x")
    prod.join()
    val merged = merge(flowOf(1, 2, 3), (4..6).asFlow()).filter { it % 2 == 0 }.map { it * 10 }.onEach { out.add("e$it") }
        .catch { out.add("err") }.toList()
    val combined = combine(flowOf("a", "b"), flowOf(1, 2)) { a, b -> "$a$b" }.flowOn(Dispatchers.Default).toList()
    val zipped = flowOf(1, 2, 3).zip(flowOf("x", "y", "z")) { a, b -> "$a$b" }.take(2).toList()
    val flat = flowOf(1, 2).flatMapLatest { v -> flow { emit(v); delay(1); emit(v + 100) } }.toList()
    val debounced = flow { repeat(5) { emit(it); delay(2) } }.debounce(1).buffer(4).conflate().toList()
    val deferreds = (1..4).map { i -> async(Dispatchers.IO) { repo.load(i).name } }
    val names = deferreds.awaitAll()
    val sem = Semaphore(2)
    val permits = (1..3).map { launch { sem.withPermit { delay(1) } } }
    permits.joinAll()
    val t = withTimeoutOrNull(50) { delay(10); "ok" }
    val sel = kotlinx.coroutines.selects.select<String> {
        async { delay(5); "slow" }.onAwait { it }
        async { "fast" }.onAwait { it }
    }
    val st = repo.observe().stateIn(this, SharingStarted.Eagerly, "init")
    val sh = flowOf(1, 2, 3).shareIn(this, SharingStarted.WhileSubscribed(100), replay = 1)
    val first = sh.first()
    repo.events.tryEmit("ev")
    val sup = supervisorScope { val j = launch { throw IllegalStateException("boom") }; j.join(); "sup" }
    val ctx = withContext(Dispatchers.Default + CoroutineName("w")) { coroutineContext[CoroutineName]?.name }
    coroutineContext.cancelChildren()
    out + merged.map { "$it" } + combined + zipped + flat.map { "$it" } + debounced.map { "$it" } + names +
        listOfNotNull(t, sel, st.value, "$first", sup, ctx)
}

fun main() {
    val client = buildClient()
    val repo = Repo(client)
    val handler = CoroutineExceptionHandler { _, e -> println("handled $e") }
    val res = runBlocking(handler) { flows(repo) }
    println(res.joinToString())
    println(collections())
    println(okioStuff())
    val req = repo.request("users")
    println(req.url.toString() + req.method + repo.formAndMultipart().size)
    if (System.getProperty("net") != null) { println(repo.execute(req)); repo.enqueue(req) }
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    scope.launch(start = CoroutineStart.LAZY) { println(repo.load(1)) }.start()
    scope.cancel()
    println(res.groupBy { it.length }.mapValues { it.value.size }.toSortedMap().entries.joinToString(prefix = "[", postfix = "]"))
    println(listOf(3, 1, 2).sortedDescending().windowed(2).chunked(1).flatten().associateWith { it.sum() })
    println("a,b,,c".split(',').filter { it.isNotBlank() }.zipWithNext().reversed())
    println(Regex("[a-z]+(\\d+)").findAll("ab12 cd34").map { it.groupValues[1] }.toList())
    println(buildString { appendLine("x"); append(3.14.toString().padStart(8, '0')) }.trimIndent().lines())
    println(sequence { yield(1); yieldAll(listOf(2, 3)) }.map { it * it }.sum())
    println(lazy { "lazy" }.value + runCatching { error("x") }.exceptionOrNull()?.message)
}
