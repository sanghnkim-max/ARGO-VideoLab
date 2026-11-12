package dev.argo.videolab.core

import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * Brings picked videos, audio and images into the project.
 *
 * In place when it can: a file on the phone's shared storage (the camera roll, Downloads) is
 * imported from where it is, read-only (the core allows reading shared storage, never writing
 * it; Android needs the media-read permissions). Otherwise (a cloud file, no permission) it is
 * streamed into the workspace's `media/` folder (never read whole into memory), reusing a copy
 * made before (same name and size) instead of making "name (2)".
 */
object Imports {
    private const val MAX_BYTES = 8L * 1024 * 1024 * 1024

    /** Import [uri]. Returns the core's import result (`items`, `path`). */
    suspend fun import(context: Context, lab: Lab, workspace: File, uri: Uri): Result<String> = withContext(Dispatchers.IO) {
        runCatching {
            localPath(context, uri)?.let { path -> return@runCatching lab.call { it.importFile(path) }.getOrThrow() }
            val resolver = context.contentResolver
            var size = 0L
            val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    size = if (c.isNull(1)) 0L else c.getLong(1)
                    require(size <= MAX_BYTES) { "the picked file is too large" }
                    c.getString(0)
                } else null
            } ?: "media.mp4"
            // A copy made earlier of the same file: reuse it (same name, size and first bytes).
            val earlier = File(workspace, "media/$name")
            if (size > 0 && earlier.isFile && earlier.length() == size && sameStart(resolver.openInputStream(uri), earlier)) {
                return@runCatching lab.call { it.importFile("media/$name") }.getOrThrow()
            }
            val target = lab.io { it.mediaTarget(name) }.getOrThrow()
            val file = File(workspace, target)
            file.parentFile?.mkdirs()
            resolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it, 1 shl 20) } } ?: error("cannot read the picked file")
            lab.call { it.importFile(target) }.getOrThrow()
        }
    }

    /** Whether [picked] starts with the same 1 MiB as [file] (closes the stream). */
    private fun sameStart(picked: java.io.InputStream?, file: File): Boolean = runCatching {
        picked?.use { a ->
            file.inputStream().use { b ->
                val n = 1 shl 20
                a.readNBytes(n).contentEquals(b.readNBytes(n))
            }
        } ?: false
    }.getOrDefault(false)

    /** The picked file's real path on shared storage, when this app can read it there. */
    fun localPath(context: Context, uri: Uri): String? {
        val resolver = context.contentResolver
        fun data(table: Uri, id: String? = null): String? = runCatching {
            resolver.query(table, arrayOf(MediaStore.MediaColumns.DATA), id?.let { "${MediaStore.MediaColumns._ID}=?" }, id?.let { arrayOf(it) }, null)
                ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        }.getOrNull()
        val path = when {
            uri.authority == MediaStore.AUTHORITY -> data(uri)
            DocumentsContract.isDocumentUri(context, uri) -> {
                val doc = DocumentsContract.getDocumentId(uri)
                val (kind, rest) = doc.substringBefore(':') to doc.substringAfter(':', "")
                when (uri.authority) {
                    "com.android.providers.media.documents" -> when (kind) {
                        "video" -> data(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, rest)
                        "audio" -> data(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, rest)
                        "image" -> data(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, rest)
                        else -> null
                    }
                    "com.android.externalstorage.documents" -> if (kind == "primary") "${Environment.getExternalStorageDirectory().absolutePath}/$rest" else null
                    "com.android.providers.downloads.documents" -> when (kind) {
                        "raw" -> rest
                        "msf" -> data(MediaStore.Files.getContentUri("external"), rest)
                        else -> null
                    }
                    else -> null
                }
            }
            else -> null
        } ?: return null
        val f = File(path)
        return path.takeIf { f.isFile && f.canRead() }
    }
}
