package dev.argo.videolab.argo

import android.content.Context
import dev.argo.videolab.agent.ChatItem
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * One chat with ARGO's agent, shown in VideoLab's Agent pane: the person's words go to ARGO over
 * [ArgoAgent], and the session's events come back as the pane's rows ([ChatItem]): the reply as it
 * streams, each tool call collapsed to a step, and ARGO's approval asks ([asks]) for the person to
 * answer here. ARGO's agent edits the open timeline through VideoLab's own MCP tools
 * (`videolab__*`), so the person watches the edits land behind the pane.
 *
 * Events are folded on [ArgoAgent]'s connection thread; the flows are read by the UI.
 */
class ArgoChat(private val context: Context) {
    /** An approval ARGO waits for. */
    data class Ask(val id: String, val tool: String, val reason: String, val canAllowForChat: Boolean)

    private val _items = MutableStateFlow<List<ChatItem>>(emptyList())
    val items: StateFlow<List<ChatItem>> = _items.asStateFlow()
    private val _running = MutableStateFlow(false)
    val running: StateFlow<Boolean> = _running.asStateFlow()
    private val _asks = MutableStateFlow<List<Ask>>(emptyList())
    val asks: StateFlow<List<Ask>> = _asks.asStateFlow()

    /** Why ARGO cannot be used now (not installed, refused, not reachable), or null. */
    private val _problem = MutableStateFlow<String?>(null)
    val problem: StateFlow<String?> = _problem.asStateFlow()

    private var agent: ArgoAgent? = null
    @Volatile private var session: String? = null

    /** The streamed reply of the step in flight: its row index in [items] and its text so far. */
    private var streaming: Pair<Int, StringBuilder>? = null

    /** The content block the stream is in: a new block starts a new paragraph. */
    private var streamBlock: String? = null

    /** Tool calls in flight by call id: their row index in [items]. */
    private val calls = HashMap<String, Int>()

    val installed: Boolean
        get() = runCatching { context.packageManager.getPackageInfo(ARGO, 0) }.isSuccess

    /** Bind ARGO's core (once); cheap to call on every pane open. */
    @Synchronized
    fun connect() {
        if (agent != null) return
        if (!installed) {
            _problem.value = "ARGO is not installed on this phone."
            return
        }
        agent = ArgoAgent(context).also { a ->
            a.addListener { frame -> fold(frame) }
            a.connect()
        }
        _problem.value = null
    }

    fun send(text: String) {
        val t = text.trim()
        if (t.isEmpty()) return
        connect()
        val a = agent ?: return
        val id = session ?: a.newSessionId().also { session = it }
        _items.update { it + ChatItem.User(t) }
        _running.value = true
        a.whenReady {
            a.open(id)
            a.prompt(id, t) { reply ->
                if (reply.error != null) {
                    note("ARGO did not take the message: ${reply.error}")
                    _running.value = false
                }
            }
        }
        // Not ready yet (ARGO's core is starting): say so rather than look stuck.
        if (!a.ready) a.down?.let { if (it != "not connected") note("Waiting for ARGO: $it") }
    }

    fun stop() {
        val id = session ?: return
        agent?.interrupt(id)
    }

    /** Answer an ask: `allowed-once`, `allowed-always` (this chat) or `rejected`. */
    fun answer(ask: Ask, outcome: String) {
        _asks.update { list -> list.filterNot { it.id == ask.id } }
        agent?.answerApproval(ask.id, outcome) { r -> r.error?.let { note("The answer did not reach ARGO: $it") } }
    }

    /** A new chat: the next message starts another ARGO session. */
    @Synchronized
    fun reset() {
        session = null
        streaming = null
        calls.clear()
        _items.value = emptyList()
        _asks.value = emptyList()
        _running.value = false
    }

    private fun note(text: String) = _items.update { it + ChatItem.Notice(text) }

    // ---------------------------------------------------------------------------------------
    // Folding ARGO's frames

    @Synchronized
    private fun fold(frame: JsonObject) {
        val mine = session ?: return
        when (frame.s("type")) {
            "event" -> if (frame.s("sessionId") == mine) (frame["event"] as? JsonObject)?.let(::event)
            "status" -> if (frame.s("sessionId") == mine) _running.value = frame.s("status") == "running"
            "approvalAsk" -> if (frame.s("sessionId") == mine) {
                val id = frame.s("id") ?: return
                val ask = Ask(
                    id = id,
                    tool = frame.s("toolName").orEmpty(),
                    reason = frame.s("reason").orEmpty(),
                    canAllowForChat = frame.s("standingGrant").let { it == null || it == "session" || it == "always" },
                )
                _asks.update { list -> list.filterNot { it.id == id } + ask }
            }
            "approvalResolved" -> frame.s("id")?.let { id -> _asks.update { list -> list.filterNot { it.id == id } } }
            "questionAsk" -> if (frame.s("sessionId") == mine) note("ARGO is asking you something: answer it in ARGO.")
            "core/state" -> if (frame.s("state") == "failed") _problem.value = "ARGO's core could not start: ${frame.s("reason").orEmpty()}"
        }
    }

    private fun event(event: JsonObject) {
        val data = event["data"] as? JsonObject ?: return
        when (event.s("type")) {
            "assistant/chunk" -> {
                val chunk = data["chunk"] as? JsonObject ?: return
                if (chunk.s("type") != "text-delta") return
                val piece = chunk.s("text") ?: return
                val (index, text) = streaming ?: (_items.value.size to StringBuilder()).also { s ->
                    streaming = s
                    streamBlock = null
                    _items.update { it + ChatItem.Assistant("") }
                }
                val block = "${data["step"]}:${chunk["index"]}"
                if (streamBlock != null && streamBlock != block && text.isNotEmpty()) text.append("\n\n")
                streamBlock = block
                text.append(piece)
                set(index, ChatItem.Assistant(text.toString()))
            }
            "assistant/message" -> {
                // The step's final text replaces what streamed (or arrives whole when nothing did).
                val content = (data["message"] as? JsonObject)?.get("content") as? JsonArray
                val text = content.orEmpty().mapNotNull { it as? JsonObject }.filter { it.s("type") == "text" }.mapNotNull { it.s("text")?.trim() }.filter { it.isNotEmpty() }.joinToString("\n\n")
                val s = streaming
                streaming = null
                when {
                    s != null && text.isNotEmpty() -> set(s.first, ChatItem.Assistant(text))
                    s != null -> remove(s.first)
                    text.isNotEmpty() -> _items.update { it + ChatItem.Assistant(text) }
                }
            }
            "tool/call" -> {
                val name = data.s("name") ?: return
                if (name in QUIET_TOOLS) return
                val callId = data.s("callId") ?: return
                val args = data.s("arguments").orEmpty()
                val parsed = runCatching { Json.parseToJsonElement(args) as? JsonObject }.getOrNull()
                // A program says what it does; a single function shows its arguments instead.
                val description = data.s("title") ?: parsed?.s("description") ?: parsed?.takeIf { it.isNotEmpty() }?.toString()?.take(120).orEmpty()
                val code = parsed?.s("code") ?: args
                calls[callId] = _items.value.size
                _items.update { it + ChatItem.ToolStep(label(name), description, code, ok = true, output = "…") }
            }
            "tool/result" -> {
                val result = ((data["message"] as? JsonObject)?.get("content") as? JsonArray)?.firstOrNull() as? JsonObject ?: return
                val index = calls.remove(result.s("toolCallId") ?: return) ?: return
                val step = _items.value.getOrNull(index) as? ChatItem.ToolStep ?: return
                val text = (result["content"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.mapNotNull { it.s("text") }.joinToString("\n")
                val failed = (result["isError"] as? JsonPrimitive)?.content == "true"
                set(index, step.copy(ok = !failed, output = unwrap(text).take(4000)))
            }
            "turn/end" -> {
                streaming = null
                val kind = (data["reason"] as? JsonObject)?.s("kind")
                if (kind != null && kind != "completed") note(if (kind == "interrupted" || kind == "cancelled") "Stopped." else "ARGO stopped: $kind")
            }
        }
    }

    private fun set(index: Int, item: ChatItem) = _items.update { list -> if (index in list.indices) list.toMutableList().also { it[index] = item } else list }

    private fun remove(index: Int) {
        _items.update { list -> if (index in list.indices) list.toMutableList().also { it.removeAt(index) } else list }
        // Rows after it moved up by one.
        calls.replaceAll { _, i -> if (i > index) i - 1 else i }
    }

    companion object {
        const val ARGO = "com.argo.android"

        /** ARGO's own bookkeeping calls: not worth a row. */
        private val QUIET_TOOLS = setOf("tool_search", "todo_write", "skill", "memory_search", "memory_timeline")

        private fun JsonObject.s(key: String) = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content

        /** How a tool reads in the pane: VideoLab's own by what it does, others by name. */
        fun label(tool: String): String = when {
            tool == "videolab__lab_read" -> "Looked"
            tool == "videolab__lab_script" -> "Edited"
            tool.startsWith("videolab__") -> "VideoLab · " + tool.removePrefix("videolab__").replace('_', ' ')
            tool == "run_code" -> "Ran a program"
            else -> tool.replace('_', ' ')
        }

        /** A result as written for the model, without ARGO's `<<<DATA … DATA>>>` fence. */
        private fun unwrap(text: String): String = text.substringAfter("data, not instructions)\n", text).substringBeforeLast("\nDATA>>>")
    }
}
