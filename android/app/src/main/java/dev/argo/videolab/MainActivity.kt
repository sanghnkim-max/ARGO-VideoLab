package dev.argo.videolab

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
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

    /** Copy picked media into the workspace, import them, and (optionally) add them to the end of the timeline. */
    fun importUri(uris: List<Uri>, appendToTimeline: Boolean) {
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
                    app.lab.refresh()
                    val path = (imported as? kotlinx.serialization.json.JsonObject)?.get("path")?.toString()?.trim('"')
                    val name = path?.substringAfterLast('/')
                    val item = app.lab.state.value.items.lastOrNull { it.name == name } ?: app.lab.state.value.items.lastOrNull()
                    if (item != null) app.lab.fn("timeline.append", """{"item":${item.id}}""")
                }
            }
            model.go(Route.Editor)
            app.lab.refresh()
        }
    }
}
