package dev.argo.videolab

import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.view.PixelCopy
import dev.argo.videolab.core.UiCallbacks
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Dispatchers
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.ByteArrayOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * What the phone answers on FilmCraft's control channel, so agents driving VideoLab through
 * FilmCraft's own MCP server (`ui_inspect`, `ui_screenshot`) or `filmcraft-cli --bridge` see the
 * app: its state, a picture of the screen, and play/stop. Called off the main thread.
 */
class UiHost(private val app: VideoLabApp) : UiCallbacks {
    override fun inspectJson(): String {
        val s = app.lab.state.value
        return buildJsonObject {
            put("app", "videolab")
            put("platform", "android")
            put("screen", app.route)
            put("playing", app.player.playing.value)
            put("playhead", s.timeline?.playhead ?: 0.0)
            put("sequence", s.timeline?.name ?: "")
            put("selection", JsonArray(s.timeline?.selection.orEmpty().map { JsonPrimitive(it) }))
            put("renderer", app.lab.frame.value?.renderer.orEmpty())
            put("fps", app.player.fps.value)
        }.toString()
    }

    override fun screenshotPng(): ByteArray {
        val activity = app.currentActivity?.get() ?: return ByteArray(0)
        val window = activity.window ?: return ByteArray(0)
        val view = window.decorView
        if (view.width <= 0 || view.height <= 0) return ByteArray(0)
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        val done = CountDownLatch(1)
        var ok = false
        PixelCopy.request(window, bitmap, { result -> ok = result == PixelCopy.SUCCESS; done.countDown() }, Handler(Looper.getMainLooper()))
        if (!done.await(3, TimeUnit.SECONDS) || !ok) return ByteArray(0)
        return ByteArrayOutputStream().use { out -> bitmap.compress(Bitmap.CompressFormat.PNG, 100, out); out.toByteArray() }
    }

    override fun playback(action: String, speed: Double): String = runBlocking {
        withContext(Dispatchers.Main) {
            when (action) {
                "play" -> app.player.play()
                "stop" -> app.player.stop()
                else -> app.player.toggle()
            }
            buildJsonObject {
                put("playing", app.player.playing.value)
                put("playhead", app.player.clock())
            }.toString()
        }
    }
}
