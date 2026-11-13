package com.argo.android.core.client

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.longOrNull

/**
 * A client's reading of one tab's batches (§2.5 "Delivery"): the mirror of
 * what `:core` sends, shared by every client of it.
 *
 * - **Order.** Batches carry a `seq` that rises by one. One already seen is
 *   a retry or a replay and is dropped; one past the next expected is a gap,
 *   answered with a `resume` from the last batch handled, once per gap.
 *   After a reset the next batch is the new base, whatever its `seq` (it is
 *   always above the last).
 * - **Bulk.** A `{"bulk":{"id":N,"len":L}}` element stands in for record N
 *   on the bulk socket; [bulk] supplies it (waiting for it, if it has not
 *   come off the socket yet). Every other element is a frame as it is.
 * - **Acknowledgement.** Every batch handled is acknowledged at once:
 *   `:core` holds back after a few unacknowledged batches per process, and
 *   an acknowledgement is the only thing that frees them.
 *
 * Not thread-safe: run every call on one thread, in arrival order.
 */
class TabReceiver(
    /** The bulk frame with this id. Throws when it cannot be had. */
    private val bulk: (id: Long) -> String,
    private val listener: Listener,
) {
    /** What the tab delivered, in order. */
    interface Listener {
        /** One batch's frames, each a JSON text. */
        fun onFrames(frames: List<String>)

        /** Read state again from the frames that follow; fail what is in flight. */
        fun onReset(coreEpoch: String, reason: String)

        /** The tab is gone. */
        fun onClosed(reason: String)
    }

    /** What to tell `:core` after a batch. */
    sealed interface Answer {
        /** `IArgoTab.ack(seq)`. */
        data class Ack(val seq: Long) : Answer

        /** `IArgoTab.resume(lastSeq)`. */
        data class Resume(val lastSeq: Long) : Answer

        /** Nothing: a duplicate, or a gap already asked about. */
        data object Nothing : Answer
    }

    /** The last batch handled. */
    var last = 0L
        private set

    /** Whether the next batch is a new base: the first one, or the first after a reset. */
    private var base = true

    /** The `last` a resume was already sent for. */
    private var resumedFrom = -1L

    /** One batch. */
    fun onBatch(seq: Long, framesJson: String): Answer {
        if (seq <= last) return Answer.Nothing
        if (!base && seq != last + 1) {
            if (resumedFrom == last) return Answer.Nothing
            resumedFrom = last
            return Answer.Resume(last)
        }
        val frames = expand(framesJson)
        base = false
        last = seq
        listener.onFrames(frames)
        return Answer.Ack(seq)
    }

    fun onReset(coreEpoch: String, reason: String) {
        base = true
        listener.onReset(coreEpoch, reason)
    }

    fun onClosed(reason: String) = listener.onClosed(reason)

    /** The frames of one batch, placeholders resolved. */
    private fun expand(framesJson: String): List<String> {
        val array = Json.parseToJsonElement(framesJson) as? JsonArray
            ?: throw IllegalArgumentException("a batch is not a JSON array")
        return array.map { element -> placeholder(element)?.let(bulk) ?: element.toString() }
    }

    companion object {
        /** The bulk id [element] stands in for, or null for an ordinary frame. */
        fun placeholder(element: JsonElement): Long? {
            val outer = element as? JsonObject ?: return null
            if (outer.size != 1) return null
            val inner = outer["bulk"] as? JsonObject ?: return null
            return (inner["id"] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull
        }
    }
}
