package dev.argo.videolab.core

import android.graphics.Bitmap
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTimestamp
import android.media.AudioTrack
import android.os.SystemClock
import java.nio.ByteBuffer
import kotlin.math.floor
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Playback: the program mix through an [AudioTrack], whose presentation timestamp is the clock,
 * and program frames on the sequence's frame grid: each frame is rendered ahead of its time and
 * shown when the clock reaches it, so every frame is shown once, on time, in step with the sound
 * (frames the core cannot keep up with are skipped, never queued). When playback stops, the
 * engine's playhead is set to where it stopped, so agents and the CLI see the same position.
 */
class Player(private val lab: Lab, private val scope: CoroutineScope) {
    private val _playing = MutableStateFlow(false)
    val playing: StateFlow<Boolean> = _playing.asStateFlow()
    private val _position = MutableStateFlow(0.0)
    /** The playback clock (seconds) while playing. */
    val position: StateFlow<Double> = _position.asStateFlow()
    /** Frames shown per second over the last second (shown in the editor's status). */
    private val _fps = MutableStateFlow(0.0)
    val fps: StateFlow<Double> = _fps.asStateFlow()

    private var audioJob: Job? = null
    private var videoJob: Job? = null
    @Volatile private var track: AudioTrack? = null
    @Volatile private var startAt = 0.0
    @Volatile private var startWall = 0L
    @Volatile private var sampleRate = 48_000
    private val stamp = AudioTimestamp()
    @Volatile private var lastClock = 0.0

    fun toggle() = if (_playing.value) stop() else play()

    fun play(from: Double? = null) {
        if (_playing.value) return
        val timeline = lab.state.value.timeline ?: return
        var t0 = from ?: timeline.playhead
        if (t0 >= timeline.duration - 0.05) t0 = 0.0
        startAt = t0
        lastClock = t0
        _position.value = t0
        _playing.value = true
        lab.playing = true
        startWall = SystemClock.elapsedRealtime()
        audioJob = scope.launch(Dispatchers.IO) { audioLoop(t0, timeline.duration) }
        videoJob = scope.launch(Dispatchers.Default) { videoLoop(timeline.duration) }
    }

    fun stop() {
        if (!_playing.value) return
        val at = clock()
        _position.value = at
        _playing.value = false
        audioJob?.cancel()
        videoJob?.cancel()
        runCatching { track?.pause(); track?.flush(); track?.release() }
        track = null
        scope.launch {
            lab.exec("playhead.set", """{"seconds":$at}""")
            lab.playing = false
            lab.requestFrame(at)
        }
    }

    /**
     * Seconds on the timeline now. While playing: the sample being heard now, from the track's
     * presentation timestamp (output latency included, smooth between hardware bursts), else
     * the head position, else the wall clock; never going backwards. Stopped: the position.
     */
    fun clock(): Double {
        if (!_playing.value) return _position.value
        val t = track
        val now = System.nanoTime()
        val heard = t?.let { tr ->
            synchronized(stamp) {
                if (runCatching { tr.getTimestamp(stamp) }.getOrDefault(false) && stamp.framePosition > 0)
                    stamp.framePosition.toDouble() / sampleRate + (now - stamp.nanoTime) / 1e9
                else null
            } ?: runCatching { tr.playbackHeadPosition }.getOrNull()?.takeIf { it > 0 }?.let { it.toDouble() / sampleRate }
        }
        val c = if (heard != null) startAt + heard else startAt + (SystemClock.elapsedRealtime() - startWall) / 1000.0
        // A timestamp can step back a little when it first becomes available: hold still instead.
        return maxOf(c, lastClock).also { lastClock = it }
    }

    private suspend fun audioLoop(t0: Double, duration: Double) {
        val first = lab.io { it.mix(t0, CHUNK_FRAMES.toUInt()) }.getOrNull() ?: return
        sampleRate = first.sampleRate.toInt()
        val min = AudioTrack.getMinBufferSize(sampleRate, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        val t = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(sampleRate).setChannelMask(AudioFormat.CHANNEL_OUT_STEREO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
            .setBufferSizeInBytes(maxOf(min, CHUNK_FRAMES * 4 * 2))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
        if (!_playing.value) {
            t.release()
            return
        }
        track = t
        t.play()
        var cursor = t0
        var chunk = first.pcm16
        while (withContext(Dispatchers.IO) { isActive } && _playing.value) {
            if (t.write(chunk, 0, chunk.size) < 0) break
            cursor += CHUNK_FRAMES.toDouble() / sampleRate
            if (cursor >= duration) break
            chunk = lab.io { it.mix(cursor, CHUNK_FRAMES.toUInt()) }.getOrNull()?.pcm16 ?: break
        }
    }

    /** A rendered frame waiting for its time. */
    private class Ready(val at: Double, val bitmap: Bitmap, val renderer: String)

    /**
     * Two stages, so rendering the next frame overlaps waiting to show this one: the renderer
     * makes frames on the sequence's grid (skipping ahead when it falls behind), the presenter
     * shows each when the clock reaches it.
     */
    private suspend fun videoLoop(duration: Double) = coroutineScope {
        val fps = (lab.state.value.timeline?.fps ?: 30.0).coerceIn(1.0, 120.0)
        val queue = Channel<Ready>(capacity = 1)
        val renderer = launch(Dispatchers.Default) {
            // Bitmaps in turn: on screen, still being drawn, queued, being filled.
            val pool = arrayOfNulls<Bitmap>(4)
            var next = 0
            var cost = 1.0 / fps
            var last = -1L
            while (isActive && _playing.value) {
                val n = maxOf(last + 1, floor((clock() + cost) * fps).toLong() + 1)
                val at = n / fps
                if (at >= duration) break
                val started = System.nanoTime()
                val frame = lab.io { it.renderPlayback(at, PLAY_SIDE.toUInt()) }.getOrNull()
                cost = cost * 0.8 + (System.nanoTime() - started) / 1e9 * 0.2
                last = n
                if (frame == null) continue
                val w = frame.width.toInt()
                val h = frame.height.toInt()
                val bmp = pool[next]?.takeIf { it.width == w && it.height == h }
                    ?: runCatching { Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888) }.getOrNull()?.also { pool[next] = it }
                    ?: continue
                if (runCatching { bmp.copyPixelsFromBuffer(ByteBuffer.wrap(frame.rgba)) }.isFailure) continue
                next = (next + 1) % pool.size
                queue.send(Ready(at, bmp, frame.renderer))
            }
            queue.close()
        }
        var shown = 0
        var windowStart = SystemClock.elapsedRealtime()
        for (f in queue) {
            val wait = f.at - clock()
            if (wait > 0.002) delay((wait * 1000).toLong())
            if (!_playing.value) break
            _position.value = f.at
            lab.showFrame(f.bitmap, f.at, f.renderer)
            shown++
            val wall = SystemClock.elapsedRealtime()
            if (wall - windowStart >= 1000) {
                _fps.value = shown * 1000.0 / (wall - windowStart)
                shown = 0
                windowStart = wall
            }
        }
        renderer.cancel()
        // The end of the sequence: wait for the clock to get there, then stop.
        while (_playing.value && clock() < duration) delay(10)
        if (_playing.value) withContext(Dispatchers.Main) { stop() }
    }

    companion object {
        /** Longest side of a frame while playing: a quarter of the paused preview's pixels. */
        const val PLAY_SIDE = 640
        /** 100 ms of sound per mix call at 48 kHz. */
        private const val CHUNK_FRAMES = 4800
    }
}
