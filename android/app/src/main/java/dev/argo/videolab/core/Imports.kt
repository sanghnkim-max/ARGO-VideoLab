package dev.argo.videolab.core

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * Brings picked videos, audio and images into the workspace's `media/` folder by streaming
 * (videos are large: never read whole into memory), then imports them into the project.
 */
object Imports {
    private const val MAX_BYTES = 8L * 1024 * 1024 * 1024

    /** Copy [uri] into the workspace and import it. Returns the core's import result. */
    suspend fun import(context: Context, lab: Lab, workspace: File, uri: Uri): Result<String> = withContext(Dispatchers.IO) {
        runCatching {
            val resolver = context.contentResolver
            val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val size = if (c.isNull(1)) 0L else c.getLong(1)
                    require(size <= MAX_BYTES) { "the picked file is too large" }
                    c.getString(0)
                } else null
            } ?: "media.mp4"
            val target = lab.io { it.mediaTarget(name) }.getOrThrow()
            val file = File(workspace, target)
            file.parentFile?.mkdirs()
            resolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it, 1 shl 20) } } ?: error("cannot read the picked file")
            lab.call { it.importFile(target) }.getOrThrow()
        }
    }
}
