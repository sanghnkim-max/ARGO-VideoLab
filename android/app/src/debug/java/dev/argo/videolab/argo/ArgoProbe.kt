package dev.argo.videolab.argo

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Debug builds only: drive ARGO's agent from `adb shell` through VideoLab's
 * [ArgoAgent] link, to try real tasks end to end without the screen:
 *
 *     adb shell am broadcast -n dev.argo.videolab/.argo.ArgoProbe -a dev.argo.videolab.ARGO_PROBE \
 *         --es do ask --es text "'Describe the timeline'" [--es session S] [--ez auto true]
 *     adb shell am broadcast ... --es do approve --es id ASK --es outcome allowed-once
 *     adb shell am broadcast ... --es do stop --es session S
 *
 * The conversation is logged under `ArgoProbe`. With `auto`, approvals of
 * `videolab__*` calls in the probe's own chats are answered `allowed-once`;
 * every other ask is only logged. Exported only to holders of DUMP (the shell).
 */
class ArgoProbe : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        val agent = agent(context)
        when (intent.getStringExtra("do")) {
            "ask" -> {
                val text = intent.getStringExtra("text").orEmpty()
                val session = intent.getStringExtra("session") ?: agent.newSessionId()
                auto = intent.getBooleanExtra("auto", auto)
                sessions += session
                Log.i(TAG, "ask session=$session auto=$auto: $text")
                agent.whenReady {
                    agent.open(session) { r -> if (r.error != null) Log.w(TAG, "open: ${r.error}") }
                    agent.prompt(session, text) { r -> Log.i(TAG, "prompt accepted=${r.error == null} ${r.error.orEmpty()}") }
                }
            }
            "approve" -> agent.answerApproval(intent.getStringExtra("id").orEmpty(), intent.getStringExtra("outcome") ?: "allowed-once") { r ->
                Log.i(TAG, "approval answered: ${r.error ?: "ok"}")
            }
            "stop" -> agent.interrupt(intent.getStringExtra("session").orEmpty())
            "auto" -> auto = intent.getBooleanExtra("on", true)
            else -> Log.i(TAG, "link ready=${agent.ready} down=${agent.down}")
        }
    }

    companion object {
        private const val TAG = "ArgoProbe"
        const val ACTION = "dev.argo.videolab.ARGO_PROBE"

        @Volatile private var held: ArgoAgent? = null
        private val sessions = java.util.concurrent.CopyOnWriteArraySet<String>()
        @Volatile private var auto = false

        /** Events too chatty or too large to log: streamed tokens, the request bodies sent to the model. */
        private val QUIET = setOf("assistant/chunk", "request/header", "request/composition", "user/message", "tool-trim/decided", "session/nudges-llm-request", "session/title-llm-request")

        private fun agent(context: Context): ArgoAgent = held ?: synchronized(this) {
            held ?: ArgoAgent(context).also { a ->
                a.addListener { frame -> log(a, frame) }
                a.connect()
                held = a
            }
        }

        private fun s(o: JsonObject?, k: String) = (o?.get(k) as? JsonPrimitive)?.content

        private fun log(agent: ArgoAgent, frame: JsonObject) {
            when (val type = s(frame, "type")) {
                "event" -> {
                    val session = s(frame, "sessionId") ?: return
                    if (session !in sessions) return
                    val event = frame["event"] as? JsonObject ?: return
                    if (s(event, "type") in QUIET) return
                    val data = event["data"]?.toString().orEmpty()
                    Log.i(TAG, "event ${s(event, "type")} #${s(event, "seq")}: ${data.take(1500)}")
                }
                "status" -> if (s(frame, "sessionId") in sessions) Log.i(TAG, "status ${s(frame, "status")}")
                "approvalAsk" -> {
                    val id = s(frame, "id") ?: return
                    val tool = s(frame, "toolName").orEmpty()
                    val mine = s(frame, "sessionId") in sessions
                    Log.i(TAG, "approvalAsk id=$id tool=$tool mine=$mine reason=${s(frame, "reason")}")
                    if (auto && mine && tool.startsWith("videolab__")) {
                        agent.answerApproval(id, "allowed-once") { r -> Log.i(TAG, "auto-approved $tool: ${r.error ?: "ok"}") }
                    }
                }
                "questionAsk" -> if (s(frame, "sessionId") in sessions) Log.i(TAG, "questionAsk $frame")
                "core/state" -> Log.i(TAG, "core ${s(frame, "state")} ${s(frame, "reason").orEmpty()}")
                "ready" -> Log.i(TAG, "hub ready")
                else -> if (type != null && type.startsWith("subagent")) Log.i(TAG, "$type ${frame.toString().take(300)}")
            }
        }
    }
}
