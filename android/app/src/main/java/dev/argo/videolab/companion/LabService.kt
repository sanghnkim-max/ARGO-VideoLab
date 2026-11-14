package dev.argo.videolab.companion

import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Binder
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.os.Process
import android.util.Log
import dev.argo.imagelab.ILabService
import dev.argo.videolab.BuildConfig
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.Lab
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/**
 * The companion wire: the ARGO app's agent drives VideoLab through [ILabService], the same
 * contract ImageLab serves (ARGOv2 `tools/lab/LabLink`; the AIDL keeps ImageLab's descriptor so
 * one client speaks to both). Exported, and answered only for a caller whose package is on
 * [ALLOWED] and signed by one of its pinned certificates, checked on every call: any app can bind
 * an exported service, so the caller's signature is the access control. ARGO holding the binding
 * also keeps VideoLab's process up (not frozen) while its agent works here.
 *
 * Ops: `mcp` carries one JSON-RPC message to VideoLab's MCP server (the core's `companion_mcp`:
 * the program tools, every function as a tool, the resources and the prompts), which ARGO mounts
 * as the server `videolab`; `status` reports the contract and what is open. [ILabService.importPhoto]
 * imports a file ARGO hands over (a video, a photo, a sound) into the project's media.
 */
class LabService : Service() {
    private val app get() = application as VideoLabApp

    /** Long work (exports, programs up to 30 minutes) never holds a Binder thread. */
    private val workers = Executors.newCachedThreadPool { r -> Thread(r, "videolab-companion").apply { isDaemon = true } }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onDestroy() {
        workers.shutdown()
        super.onDestroy()
    }

    private val binder = object : ILabService.Stub() {
        override fun contractVersion(): Int = CONTRACT_VERSION

        override fun call(op: String?, argsJson: String?): ParcelFileDescriptor {
            val refusal = refusal()
            val (read, write) = ParcelFileDescriptor.createPipe()
            workers.execute {
                val reply = when {
                    refusal != null -> error(refusal)
                    // MCP: the JSON-RPC response itself (nothing for a notification).
                    op == "mcp" -> runBlocking { app.lab.io { it.companionMcp(argsJson.orEmpty()) } }.getOrElse { failedMcp(argsJson.orEmpty(), Lab.describe(it)) }
                    op == "status" -> status()
                    else -> error("unknown op `$op`; VideoLab answers status and mcp")
                }
                try {
                    ParcelFileDescriptor.AutoCloseOutputStream(write).use { it.write(reply.toByteArray(Charsets.UTF_8)) }
                } catch (gone: java.io.IOException) {
                    Log.i(TAG, "the caller stopped reading the answer to `$op`", gone)
                }
            }
            return read
        }

        override fun importPhoto(name: String?, photo: ParcelFileDescriptor?): String {
            refusal()?.let { return error(it) }
            if (photo == null) return error("no file was given")
            return runCatching {
                val fileName = name?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "media.mp4"
                val target = runBlocking { app.lab.io { it.mediaTarget(fileName) } }.getOrThrow()
                val file = File(app.workspace, target).apply { parentFile?.mkdirs() }
                var total = 0L
                ParcelFileDescriptor.AutoCloseInputStream(photo).use { input ->
                    file.outputStream().use { out ->
                        val buf = ByteArray(1 shl 20)
                        while (true) {
                            val n = input.read(buf)
                            if (n < 0) break
                            total += n
                            if (total > MAX_IMPORT_BYTES) throw IllegalArgumentException("the file is larger than ${MAX_IMPORT_BYTES shr 30} GB")
                            out.write(buf, 0, n)
                        }
                    }
                }
                val imported = runBlocking { app.lab.io { it.importFile(target) } }.getOrThrow()
                app.lab.requestRefresh()
                JSONObject().put("ok", JSONObject(imported)).toString()
            }.getOrElse { error(Lab.describe(it)) }
        }

        override fun cancel() {
            if (refusal() != null) return
            workers.execute { runBlocking { app.lab.io { it.cancelScripts() } } }
        }
    }

    private fun status(): String {
        val timeline = app.lab.state.value.timeline
        return JSONObject().put("ok", JSONObject()
            .put("contract", CONTRACT_VERSION)
            .put("version", BuildConfig.VERSION_NAME)
            .put("project", timeline?.name ?: JSONObject.NULL)
            .put("duration", timeline?.duration ?: JSONObject.NULL)).toString()
    }

    /** A JSON-RPC error response to the request in [message], when the server could not answer. */
    private fun failedMcp(message: String, why: String): String {
        val id = runCatching { JSONObject(message).opt("id") }.getOrNull() ?: return ""
        return JSONObject().put("jsonrpc", "2.0").put("id", id).put("error", JSONObject().put("code", -32603).put("message", why)).toString()
    }

    /** Why the current Binder caller is refused, or null when it is ARGO (and the person lets it in). */
    private fun refusal(): String? {
        val uid = Binder.getCallingUid()
        if (uid == Process.myUid()) return null
        val packages = packageManager.getPackagesForUid(uid).orEmpty()
        val allowed = packages.firstOrNull { pkg ->
            // A release: ARGO signed with the very key this app is (the companion catalog's rule).
            (pkg == ARGO && packageManager.checkSignatures(packageName, pkg) == PackageManager.SIGNATURE_MATCH) ||
                ALLOWED[pkg]?.any { digest -> packageManager.hasSigningCertificate(pkg, digest, PackageManager.CERT_INPUT_SHA256) } == true
        }
        if (allowed == null) {
            Log.w(TAG, "refused uid $uid (${packages.joinToString()})")
            return "this caller is not allowed to drive VideoLab"
        }
        if (!app.settings.argoCompanion) return "the person turned off \"Let ARGO use VideoLab\" in VideoLab's Settings"
        return null
    }

    companion object {
        private const val TAG = "VideoLabCompanion"
        const val CONTRACT_VERSION = 1
        private const val MAX_IMPORT_BYTES = 8L shl 30

        private const val ARGO = "com.argo.android"

        /**
         * Who else may drive VideoLab: package → SHA-256 digests of its signing certificate. A release
         * VideoLab is signed with ARGO's release key and serves the ARGO signed like itself (above);
         * a debug build is signed by the developer's own key, so ARGO's committed debug key
         * (`apps/argo-android/debug.keystore` in ARGOv2, the same one ARGO Browser carries) is
         * listed here.
         */
        private val ALLOWED: Map<String, List<ByteArray>> = mapOf(
            ARGO to listOf(hex("4eecf98c331b09ebb54559b60ab544856e21c76f1239fdf9e3916bbf60f63c6a")),
        )

        private fun hex(s: String) = ByteArray(s.length / 2) { i -> s.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        private fun error(message: String) = JSONObject().put("error", message).toString()
    }
}
