package dev.argo.videolab.argo

import android.content.Context
import android.util.Log
import com.argo.android.core.client.CoreConnection
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicLong
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * ARGO's agent, reached from VideoLab over ARGO's `:core` service: one hub tab
 * ([CoreConnection], ARGO's own client half), on which VideoLab opens chats,
 * sends the person's words, and reads the conversation back as frames.
 *
 * ARGO admits VideoLab as a `client` tab only when it is signed by a certificate
 * on ARGO's allow-list (`core_clients.xml`); such a tab can chat but changes no
 * ARGO settings. The agent reaches VideoLab back through its MCP tools
 * (`videolab__*`), so a request made here can edit the timeline open in the app.
 *
 * Frames are delivered on the connection's own thread, in order, to every
 * [Listener]. Replies to requests are matched by id.
 */
class ArgoAgent(context: Context) : CoreConnection.Listener {
    fun interface Listener {
        fun onFrame(frame: JsonObject)
    }

    /** A reply: its result, or the hub's refusal. */
    data class Reply(val result: JsonElement?, val error: String?)

    private val connection = CoreConnection(context.applicationContext, this)
    private val nextId = AtomicLong(1)
    private val waiting = ConcurrentHashMap<String, (Reply) -> Unit>()
    private val listeners = CopyOnWriteArrayList<Listener>()

    /** Whether the hub has answered `web/hello` on the current tab. */
    @Volatile var ready = false
        private set

    /** Why the link is down, or null. */
    @Volatile var down: String? = "not connected"
        private set

    private val held = ArrayList<() -> Unit>()

    fun addListener(l: Listener) = listeners.add(l)

    fun removeListener(l: Listener) = listeners.remove(l)

    fun connect() = connection.connect()

    fun close() = connection.close()

    /** Run [action] once the hub is ready (now, if it is). */
    fun whenReady(action: () -> Unit) {
        val now = synchronized(held) { if (ready) true else { held += action; false } }
        if (now) action()
    }

    /** Send one request; [onReply] runs on the connection thread. False when no tab is open. */
    fun request(method: String, params: JsonElement?, onReply: (Reply) -> Unit = {}): Boolean {
        val id = nextId.getAndIncrement().toString()
        waiting[id] = onReply
        val frame = buildJsonObject {
            put("id", id.toLong())
            put("method", method)
            if (params != null) put("params", params)
        }.toString()
        val sent = connection.send(frame)
        if (!sent) waiting.remove(id)?.invoke(Reply(null, "ARGO is not connected"))
        return sent
    }

    /** A new chat's id, as ARGO's own app mints them. */
    fun newSessionId(): String = "session-" + UUID.randomUUID().toString().replace("-", "")

    /** `session/prompt`: [text] to the chat [sessionId] (which it creates when new). */
    fun prompt(sessionId: String, text: String, onReply: (Reply) -> Unit = {}) = request(
        "session/prompt",
        buildJsonObject {
            put("sessionId", sessionId)
            put("contentBlocks", buildJsonArray {
                add(buildJsonObject {
                    put("type", "text")
                    put("text", text)
                })
            })
            put("messageId", "c-" + UUID.randomUUID())
        },
        onReply,
    )

    /** `session/open`: subscribe this tab to [sessionId]'s events (creating it when new). */
    fun open(sessionId: String, onReply: (Reply) -> Unit = {}) =
        request("session/open", buildJsonObject { put("sessionId", sessionId) }, onReply)

    /** `session/interrupt`: stop the turn in flight. */
    fun interrupt(sessionId: String) = request("session/interrupt", buildJsonObject { put("sessionId", sessionId) })

    /** `web/approval/answer`: `allowed-once`, `allowed-always` (this chat), or `rejected`. */
    fun answerApproval(id: String, outcome: String, onReply: (Reply) -> Unit = {}) = request(
        "web/approval/answer",
        buildJsonObject {
            put("id", id)
            put("outcome", outcome)
        },
        onReply,
    )

    private fun hello() {
        request("web/hello", buildJsonObject { put("pages", "none") }) { reply ->
            if (reply.error != null) Log.w(TAG, "ARGO's hub refused hello: ${reply.error}")
            val run = synchronized(held) {
                ready = true
                down = null
                held.toList().also { held.clear() }
            }
            run.forEach { runCatching(it).onFailure { e -> Log.w(TAG, "a held request failed", e) } }
        }
    }

    override fun onOpened(coreEpoch: String?) {
        Log.i(TAG, "tab open on ARGO's core (epoch ${coreEpoch ?: "booting"})")
        // While the core boots the tab gets `core/state` frames, then onReset when it is up.
        if (coreEpoch != null) hello()
    }

    override fun onDisconnected(reason: String) {
        Log.w(TAG, "ARGO link down: $reason")
        ready = false
        down = reason
        failAll(reason)
    }

    override fun onFrames(frames: List<String>) {
        for (text in frames) {
            val frame = runCatching { Json.parseToJsonElement(text) as? JsonObject }.getOrNull() ?: continue
            val type = (frame["type"] as? JsonPrimitive)?.content
            if (type == "reply") {
                val id = (frame["id"] as? JsonPrimitive)?.content ?: continue
                val error = (frame["error"] as? JsonObject)?.let { e ->
                    listOfNotNull((e["code"] as? JsonPrimitive)?.content, (e["message"] as? JsonPrimitive)?.content).joinToString(": ")
                }
                waiting.remove(id)?.let { done -> runCatching { done(Reply(frame["result"], error)) } }
                continue
            }
            listeners.forEach { l -> runCatching { l.onFrame(frame) }.onFailure { Log.w(TAG, "a frame listener failed", it) } }
        }
    }

    override fun onReset(coreEpoch: String, reason: String) {
        Log.i(TAG, "ARGO's core reset ($reason), epoch $coreEpoch")
        ready = false
        failAll("ARGO restarted ($reason)")
        hello()
    }

    override fun onClosed(reason: String) {
        ready = false
        down = reason
        failAll(reason)
    }

    private fun failAll(reason: String) {
        val all = waiting.keys.toList()
        all.forEach { id -> waiting.remove(id)?.let { runCatching { it(Reply(null, reason)) } } }
    }

    companion object {
        private const val TAG = "ArgoAgent"
    }
}
