package dev.argo.videolab.core

import android.content.ContentValues
import android.content.Context
import android.os.FileObserver
import android.provider.MediaStore
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Finished exports go to the phone's Gallery: videos under `Movies/VideoLab`, GIFs under
 * `Pictures/VideoLab`. An export lives in VideoLab's private workspace (`exports/`), where no
 * other app (and so not the person, from the Gallery) can see it, whoever asked for it: the
 * export sheet, the in-app agent, or ARGO's agent over the companion link.
 *
 * A write in `exports/` wakes a short watch of `export.jobs`; each job that finished without an
 * error is copied once. At start, exports already in the folder that the Gallery lacks are copied
 * too (made while this was not running, or before it existed). The Gallery is asked by name, so
 * nothing is copied twice, also across reinstalls. Inserting its own files needs no permission.
 */
class GalleryExports(private val context: Context, private val workspace: File, private val lab: () -> Lab, private val scope: CoroutineScope) {
    private val exports = File(workspace, "exports").apply { mkdirs() }
    private val watching = AtomicBoolean(false)

    @Suppress("unused") // held: a FileObserver that is collected stops watching
    private val observer = object : FileObserver(exports, CLOSE_WRITE or MOVED_TO) {
        override fun onEvent(event: Int, path: String?) {
            if (path != null && publishable(path)) watchJobs()
        }
    }

    fun start() {
        observer.startWatching()
        scope.launch(Dispatchers.IO) {
            exports.listFiles().orEmpty().filter { it.isFile && publishable(it.name) }.forEach { runCatching { publish(it) } }
        }
    }

    /** Read `export.jobs` until none is running, publishing what finished well. */
    private fun watchJobs() {
        if (!watching.compareAndSet(false, true)) return
        scope.launch(Dispatchers.IO) {
            try {
                repeat(MAX_POLLS) {
                    val jobs = lab().io { it.call("export.jobs", "{}", "ui") }.getOrNull()?.let(lab()::parse) as? JsonArray ?: return@launch
                    var running = false
                    for (j in jobs.mapNotNull { it as? JsonObject }) {
                        if ((j["finished"] as? JsonPrimitive)?.booleanOrNull != true) {
                            running = true
                            continue
                        }
                        val result = j["result"] as? JsonObject ?: continue
                        if (result["error"] != null) continue
                        val path = (result["path"] as? JsonPrimitive)?.content ?: continue
                        val file = File(path).let { if (it.isAbsolute) it else File(workspace, path) }
                        if (file.isFile && publishable(file.name)) runCatching { publish(file) }.onFailure { Log.w(TAG, "could not add ${file.name} to the Gallery", it) }
                    }
                    if (!running) return@launch
                    delay(POLL_MS)
                }
            } finally {
                watching.set(false)
            }
        }
    }

    /** Copy [file] into the Gallery unless an item of its name is already there. */
    @Synchronized
    private fun publish(file: File) {
        val gif = file.extension.equals("gif", true)
        val collection = if (gif) MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY) else MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val folder = if (gif) "Pictures/VideoLab/" else "Movies/VideoLab/"
        val resolver = context.contentResolver
        resolver.query(
            collection,
            arrayOf(MediaStore.MediaColumns._ID),
            "${MediaStore.MediaColumns.DISPLAY_NAME} = ? AND ${MediaStore.MediaColumns.RELATIVE_PATH} = ? AND ${MediaStore.MediaColumns.SIZE} = ?",
            arrayOf(file.name, folder, file.length().toString()),
            null,
        )?.use { if (it.moveToFirst()) return }
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, file.name)
            put(MediaStore.MediaColumns.MIME_TYPE, if (gif) "image/gif" else "video/mp4")
            put(MediaStore.MediaColumns.RELATIVE_PATH, folder)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(collection, values) ?: return
        try {
            resolver.openOutputStream(uri)?.use { out -> file.inputStream().use { it.copyTo(out, 1 shl 20) } } ?: error("no stream")
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            Log.i(TAG, "added ${file.name} to the Gallery ($folder)")
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw e
        }
    }

    private fun publishable(name: String) = name.substringAfterLast('.').lowercase() in setOf("mp4", "mov", "gif") && !name.startsWith(".")

    companion object {
        private const val TAG = "GalleryExports"
        private const val POLL_MS = 1_000L
        /** An hour of watching at most: an export that runs longer is picked up by the next write or start. */
        private const val MAX_POLLS = 3_600

        /** Where the person finds an export, for messages. */
        fun where(name: String) = if (name.endsWith(".gif", true)) "Gallery › Pictures/VideoLab" else "Gallery › Movies/VideoLab"
    }
}
