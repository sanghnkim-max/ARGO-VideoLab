package dev.argo.videolab

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.lifecycle.lifecycleScope
import dev.argo.videolab.core.Imports
import dev.argo.videolab.ui.AppRoot
import dev.argo.videolab.ui.AppViewModel
import dev.argo.videolab.ui.Route
import dev.argo.videolab.ui.VideoLabTheme
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val app get() = application as VideoLabApp
    private val model: AppViewModel by viewModels()

    /** An import waiting for the answer to the media-read permission request. */
    private var afterPermission: (() -> Unit)? = null
    private val askMedia = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        afterPermission?.invoke()
        afterPermission = null
    }
    private val mediaPermissions = arrayOf(Manifest.permission.READ_MEDIA_VIDEO, Manifest.permission.READ_MEDIA_AUDIO, Manifest.permission.READ_MEDIA_IMAGES)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(SystemBarStyle.dark(android.graphics.Color.TRANSPARENT), SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
        setContent { VideoLabTheme { AppRoot(app, model, onImport = ::importUri) } }
        if (savedInstanceState == null) handleIntent(intent)
        lifecycleScope.launch { app.lab.refresh() }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    override fun onStop() {
        super.onStop()
        app.player.stop()
    }

    /** "Edit with VideoLab" (ACTION_EDIT) and the share sheet (ACTION_SEND). */
    private fun handleIntent(intent: Intent?) {
        val uri: Uri? = when (intent?.action) {
            Intent.ACTION_EDIT -> intent.data
            Intent.ACTION_SEND -> intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            else -> null
        }
        uri?.let { importUri(listOf(it), appendToTimeline = true) }
    }

    /**
     * Import picked media (in place when the media-read permission allows, else as a copy) and
     * optionally add them to the end of the timeline. Asks for the permission first, once.
     */
    fun importUri(uris: List<Uri>, appendToTimeline: Boolean) {
        val missing = mediaPermissions.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty() && afterPermission == null) {
            afterPermission = { importNow(uris, appendToTimeline) }
            askMedia.launch(missing.toTypedArray())
            return
        }
        importNow(uris, appendToTimeline)
    }

    private fun importNow(uris: List<Uri>, appendToTimeline: Boolean) {
        lifecycleScope.launch {
            if (app.lab.state.value.timeline == null) app.lab.fn("doc.new", """{"name":"My video"}""")
            for (uri in uris) {
                val r = Imports.import(this@MainActivity, app.lab, app.workspace, uri)
                val imported = r.getOrNull()?.let(app.lab::parse)
                if (r.isFailure) {
                    app.lab.showMessage("Import failed: ${r.exceptionOrNull()?.message}")
                    continue
                }
                if (appendToTimeline) {
                    val items = ((imported as? kotlinx.serialization.json.JsonObject)?.get("items") as? kotlinx.serialization.json.JsonArray)
                        ?.mapNotNull { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() }.orEmpty()
                    for (item in items) app.lab.fn("timeline.append", """{"item":$item}""")
                }
            }
            model.go(Route.Editor)
            app.lab.refresh()
        }
    }
}
