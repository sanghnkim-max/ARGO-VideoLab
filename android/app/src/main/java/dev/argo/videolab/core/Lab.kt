package dev.argo.videolab.core

import android.graphics.Bitmap
import android.util.LruCache
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement

/** What the editor shows. */
data class EditorState(
    val timeline: Timeline? = null,
    val items: List<Item> = emptyList(),
    val busy: Boolean = false,
    val message: String? = null,
    val ready: Boolean = false,
    val version: Long = 0,
)

/**
 * The frame in the program monitor. Its own flow, apart from [EditorState]: it changes every
 * frame while playing, and only the monitor should redraw for it.
 */
data class PreviewFrame(
    val bitmap: Bitmap,
    /** Where the frame is from (seconds). */
    val at: Double,
    /** `gpu` or `cpu`: which FilmCraft renderer drew it. */
    val renderer: String,
    /** Bumped per frame: the player reuses bitmaps, so the bitmap alone does not say it changed. */
    val serial: Long,
)

/**
 * The app's handle on the Rust core (one FilmCraft session, `videolab-ffi`).
 *
 * Every core call runs on [Dispatchers.IO]: the core blocks (decodes, renders, exports). The
 * editor follows the core's events: a `docChanged` from any source — the UI, the in-app agent,
 * the CLI console, FilmCraft's MCP bridge, a laptop over adb — re-reads the timeline and
 * re-renders the frame under the playhead (debounced).
 */
class Lab(createApi: () -> VideoLab, private val scope: CoroutineScope) {
    private val apiDeferred = scope.async(Dispatchers.IO) { createApi() }

    private val _state = MutableStateFlow(EditorState())
    val state: StateFlow<EditorState> = _state.asStateFlow()
    private val _frame = MutableStateFlow<PreviewFrame?>(null)
    val frame: StateFlow<PreviewFrame?> = _frame.asStateFlow()
    private val _events = MutableSharedFlow<LabEvent>(extraBufferCapacity = 64, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    val events: SharedFlow<LabEvent> = _events.asSharedFlow()
    val json = Json { ignoreUnknownKeys = true }

    private val refreshRequests = Channel<Unit>(Channel.CONFLATED)
    private val frameRequests = Channel<Double>(Channel.CONFLATED)
    private val renderLock = Mutex()

    /** While playing, the player owns the preview and the playhead; events only re-read the timeline. */
    @Volatile var playing = false

    /** Bumped when a thumbnail or waveform lands, so the timeline redraws. */
    private val _mediaVersion = MutableStateFlow(0L)
    val mediaVersion: StateFlow<Long> = _mediaVersion.asStateFlow()

    init {
        scope.launch(Dispatchers.IO) {
            runCatching {
                apiDeferred.await().setEventListener(object : EventListener {
                    override fun onEvent(eventJson: String) {
                        val event = LabEvent.parse(eventJson) ?: return
                        _events.tryEmit(event)
                        if (event is LabEvent.DocChanged) refreshRequests.trySend(Unit)
                    }
                })
                _state.update { it.copy(ready = true) }
            }.onFailure { t -> _state.update { it.copy(message = "The engine failed to start: ${describe(t)}") } }
        }
        scope.launch {
            for (unit in refreshRequests) {
                delay(REFRESH_DEBOUNCE_MS)
                refresh()
            }
        }
        // A scrub renders each position as playback does (quick: optical-flow slow motion by frame
        // sampling, drafts where decoders can take them), then the position it stops on once at
        // full quality: optical flow on 4K footage is hundreds of milliseconds a frame, which made
        // dragging the playhead crawl.
        scope.launch {
            var unsettled: Double? = null
            while (true) {
                val next = if (unsettled == null) frameRequests.receive() else withTimeoutOrNull(SETTLE_MS) { frameRequests.receive() }
                if (next == null) {
                    renderFrame(unsettled!!, PREVIEW_SIDE)
                    unsettled = null
                } else {
                    renderFrame(next, quickSide(), quick = true)
                    unsettled = next
                }
            }
        }
    }

    /** Run [block] on the core off the main thread. */
    suspend fun <T> io(block: (VideoLab) -> T): Result<T> = withContext(Dispatchers.IO) { runCatching { block(apiDeferred.await()) } }

    /** Run [block] on the core with a busy flag; a failure becomes the editor's message. */
    suspend fun <T> call(block: (VideoLab) -> T): Result<T> {
        _state.update { it.copy(busy = true) }
        val result = io(block)
        _state.update { it.copy(busy = false, message = result.exceptionOrNull()?.let(::describe) ?: it.message) }
        if (result.isSuccess) requestRefresh()
        return result
    }

    /** A registry call from the UI (`timeline.split`, `text.add`, …), attributed to the UI. */
    suspend fun fn(id: String, params: String = "{}"): Result<JsonElement?> = call { parse(it.call(id, params, "ui")) }

    /** A FilmCraft engine command from the UI. */
    suspend fun exec(id: String, params: String = "{}"): Result<JsonElement?> = call { parse(it.execute(id, params)) }

    fun requestRefresh() {
        refreshRequests.trySend(Unit)
    }

    /** Show the frame at [seconds] soon (conflated: a scrub renders only the latest position). */
    fun requestFrame(seconds: Double) {
        frameRequests.trySend(seconds)
    }

    /** Re-read the timeline and items; re-render the frame under the playhead unless playing. */
    suspend fun refresh() {
        val api = runCatching { apiDeferred.await() }.getOrNull() ?: return
        val (items, timeline) = withContext(Dispatchers.IO) {
            val items = parseItems(parse(runCatching { api.items() }.getOrDefault("[]")))
            val map = items.associateBy { it.id }
            items to Timeline.parse(parse(runCatching { api.timeline() }.getOrDefault("null")), map)
        }
        val e = timeline?.epoch ?: epoch
        if (e != epoch) {
            // Another document: its item ids may name other media.
            epoch = e
            thumbs.evictAll()
            peaks.clear()
        }
        _state.update { it.copy(items = items, timeline = timeline, version = it.version + 1) }
        if (!playing) {
            if (timeline == null) _frame.value = null else renderFrame(timeline.playhead, PREVIEW_SIDE)
        }
    }

    /** A scrub frame's longest side: playback's default (half the sequence's), the settled frame sharpens it. */
    private fun quickSide(): Int {
        val t = _state.value.timeline
        return (maxOf(t?.width ?: 1920, t?.height ?: 1920) / 2).coerceIn(320, PREVIEW_SIDE)
    }

    /** Render the program frame at [seconds] into the preview. */
    suspend fun renderFrame(seconds: Double, maxSide: Int, quick: Boolean = false): Boolean = renderLock.withLock {
        withContext(Dispatchers.IO) {
            val api = runCatching { apiDeferred.await() }.getOrNull() ?: return@withContext false
            val frame = runCatching { if (quick) api.renderPlayback(seconds, maxSide.toUInt()) else api.render(seconds, maxSide.toUInt()) }.getOrNull() ?: return@withContext false
            val bitmap = frame.toBitmap() ?: return@withContext false
            showFrame(bitmap, seconds, frame.renderer)
            true
        }
    }

    /** Set the preview directly (the player). */
    fun showFrame(bitmap: Bitmap, seconds: Double, renderer: String) {
        _frame.value = PreviewFrame(bitmap, seconds, renderer, (_frame.value?.serial ?: 0) + 1)
    }

    // ── thumbnails and waveforms ─────────────────────────────────────────────────────────────

    private val thumbs = object : LruCache<String, Bitmap>(48 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Bitmap) = value.byteCount
    }
    private val pendingThumbs = ConcurrentHashMap.newKeySet<String>()
    private data class ThumbJob(val key: String, val item: Long, val t: Double, val height: Int)
    private val thumbQueue = Channel<ThumbJob>(Channel.UNLIMITED)
    private val peaks = ConcurrentHashMap<String, FloatArray>()
    private val pendingPeaks = ConcurrentHashMap.newKeySet<String>()
    /** The document epoch filmstrips and waveforms are cached for (see [Timeline.epoch]). */
    @Volatile private var epoch = -1L

    init {
        // Two decoders at most: thumbnails are background work behind the preview.
        repeat(2) {
            scope.launch(Dispatchers.IO) {
                for ((key, item, t, h) in thumbQueue) {
                    if (thumbs.get(key) == null) {
                        val api = runCatching { apiDeferred.await() }.getOrNull()
                        val frame = api?.let { a -> runCatching { a.thumbnails(item.toULong(), listOf(t), h.toUInt()).firstOrNull() }.getOrNull() }
                        frame?.toBitmap()?.let { thumbs.put(key, it) }
                        _mediaVersion.update { it + 1 }
                    }
                    pendingThumbs.remove(key)
                }
            }
        }
    }

    private fun thumbKey(item: Long, t: Double, h: Int) = "$epoch:$item@${"%.2f".format(t)}@$h"

    /** A filmstrip frame of media [item] at media time [t] (quantized by the caller), or null while it loads. */
    fun thumbnail(item: Long, t: Double, height: Int): Bitmap? {
        val key = thumbKey(item, t, height)
        thumbs.get(key)?.let { return it }
        if (pendingThumbs.add(key)) thumbQueue.trySend(ThumbJob(key, item, t, height))
        return null
    }

    /** Waveform of media [item]: (min, max) pairs, [PEAKS_PER_SECOND] per media second; null while it loads. */
    fun peaks(item: Long, duration: Double): FloatArray? {
        val key = "$epoch:$item"
        peaks[key]?.let { return it }
        if (duration > 0 && pendingPeaks.add(key)) {
            scope.launch(Dispatchers.IO) {
                val buckets = (duration * PEAKS_PER_SECOND).toInt().coerceIn(1, 16_384)
                val api = runCatching { apiDeferred.await() }.getOrNull()
                val v = api?.let { a -> runCatching { a.peaks(item.toULong(), 0.0, duration, buckets.toUInt()) }.getOrNull() }
                if (v != null) {
                    peaks[key] = v.toFloatArray()
                    _mediaVersion.update { it + 1 }
                } else {
                    pendingPeaks.remove(key)
                }
            }
        }
        return null
    }

    /** Actual buckets per media second of a cached waveform (long media are capped). */
    fun peaksPerSecond(item: Long, duration: Double): Double {
        val n = (peaks["$epoch:$item"]?.size ?: 0) / 2
        return if (duration > 0 && n > 0) n / duration else PEAKS_PER_SECOND.toDouble()
    }

    fun clearMessage() = _state.update { it.copy(message = null) }
    fun showMessage(text: String) = _state.update { it.copy(message = text) }

    fun parse(text: String): JsonElement? = runCatching { json.parseToJsonElement(text) }.getOrNull()

    companion object {
        /** Longest side of the paused preview; the player renders smaller. */
        const val PREVIEW_SIDE = 1280

        /** A scrub has stopped once no new position came for this long: render it at full quality. */
        private const val SETTLE_MS = 200L
        const val PEAKS_PER_SECOND = 20
        private const val REFRESH_DEBOUNCE_MS = 60L

        fun describe(t: Throwable): String = when (t) {
            is VideoLabException.BadRequest -> t.reason
            is VideoLabException.Failed -> t.reason
            else -> t.message ?: t.toString()
        }
    }
}

fun RgbaFrame.toBitmap(): Bitmap? = runCatching {
    Bitmap.createBitmap(width.toInt(), height.toInt(), Bitmap.Config.ARGB_8888).apply {
        copyPixelsFromBuffer(ByteBuffer.wrap(rgba))
    }
}.getOrNull()
