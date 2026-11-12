package dev.argo.videolab.core

import java.io.File
import java.io.FileInputStream
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull

/**
 * Downloads FilmCraft's Whisper speech models on the phone. The core says which files a model
 * needs and where they go (`speech.modelFiles`: FilmCraft's catalogue, pinned to a Hugging Face
 * revision with sizes and SHA-256); this fetches them with Android's HTTP stack (FilmCraft's own
 * downloader needs JNI set-up for Android's certificate checks) and checks each file the way
 * FilmCraft will. Partial files resume.
 */
object SpeechModels {
    /** The model auto captions use: the smallest (150 MB), quick on a phone. */
    const val DEFAULT = "whisper-tiny"

    data class Progress(val done: Long, val total: Long) {
        val fraction get() = if (total > 0) (done.toDouble() / total).toFloat() else 0f
    }

    /** Whether [model]'s files are all present and valid. */
    suspend fun installed(lab: Lab, model: String = DEFAULT): Boolean =
        ((lab.fn("speech.modelFiles", """{"model":"$model"}""").getOrNull() as? JsonObject)?.get("installed") as? JsonPrimitive)?.booleanOrNull == true

    /** Download what [model] is missing; [onProgress] gets bytes so far over the model's size. */
    suspend fun ensure(lab: Lab, model: String = DEFAULT, onProgress: (Progress) -> Unit): Result<Unit> = runCatching {
        val info = lab.fn("speech.modelFiles", """{"model":"$model"}""").getOrThrow() as? JsonObject ?: error("no model info")
        if ((info["installed"] as? JsonPrimitive)?.booleanOrNull == true) return@runCatching
        val files = (info["files"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { f ->
            FileSpec(f.s("url") ?: error("no url"), File(f.s("path") ?: error("no path")), f.l("size") ?: 0, f.s("sha256").orEmpty())
        }
        val total = files.sumOf { it.size }
        var before = 0L
        for (f in files) {
            if (!valid(f)) download(f) { got -> onProgress(Progress(before + got, total)) }
            before += f.size
            onProgress(Progress(before, total))
        }
    }

    private data class FileSpec(val url: String, val dest: File, val size: Long, val sha256: String)

    private suspend fun valid(f: FileSpec): Boolean = withContext(Dispatchers.IO) {
        f.dest.isFile && f.dest.length() == f.size && sha256(f.dest).equals(f.sha256, ignoreCase = true)
    }

    private suspend fun download(f: FileSpec, onBytes: (Long) -> Unit) = withContext(Dispatchers.IO) {
        f.dest.parentFile?.mkdirs()
        val part = File(f.dest.path + ".part")
        var have = if (part.isFile && part.length() < f.size) part.length() else 0L
        var url = URL(f.url)
        // Hugging Face answers with redirects to its CDN (https → https, which this follows).
        val conn = url.openConnection() as HttpURLConnection
        conn.instanceFollowRedirects = true
        conn.connectTimeout = 20_000
        conn.readTimeout = 30_000
        if (have > 0) conn.setRequestProperty("Range", "bytes=$have-")
        conn.connect()
        val code = conn.responseCode
        if (code != 200 && code != 206) error("download failed: HTTP $code for ${f.dest.name}")
        if (code == 200) have = 0
        RandomAccessFile(part, "rw").use { out ->
            out.setLength(have)
            out.seek(have)
            conn.inputStream.use { input ->
                val buf = ByteArray(256 * 1024)
                var got = have
                var lastReport = 0L
                while (true) {
                    ensureActive()
                    val n = input.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    got += n
                    if (got - lastReport > 1_000_000) {
                        onBytes(got)
                        lastReport = got
                    }
                }
            }
        }
        conn.disconnect()
        if (part.length() != f.size || !sha256(part).equals(f.sha256, ignoreCase = true)) {
            part.delete()
            error("${f.dest.name} did not download correctly; try again")
        }
        if (!part.renameTo(f.dest)) error("could not save ${f.dest.name}")
    }

    private fun sha256(file: File): String {
        val md = MessageDigest.getInstance("SHA-256")
        FileInputStream(file).use { input ->
            val buf = ByteArray(1 shl 20)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                md.update(buf, 0, n)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }
}
