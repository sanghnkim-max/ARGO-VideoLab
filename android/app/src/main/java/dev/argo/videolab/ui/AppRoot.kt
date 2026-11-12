package dev.argo.videolab.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.lifecycle.ViewModel
import dev.argo.videolab.VideoLabApp
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

enum class Route { Home, Editor, Settings }

/** View state that survives rotation; the document itself lives in the core. */
class AppViewModel : ViewModel() {
    private val _route = MutableStateFlow(Route.Home)
    val route: StateFlow<Route> = _route.asStateFlow()
    private var previous = Route.Home
    fun go(r: Route) {
        previous = _route.value
        _route.value = r
    }
    fun back() {
        _route.value = if (_route.value == Route.Settings) previous.takeIf { it != Route.Settings } ?: Route.Home else Route.Home
    }
}

@Composable
fun AppRoot(app: VideoLabApp, model: AppViewModel, onImport: (List<Uri>, Boolean) -> Unit) {
    val route by model.route.collectAsState()
    LaunchedEffect(route) { app.route = route.name.lowercase() }
    BackHandler(enabled = route != Route.Home) { model.back() }
    when (route) {
        Route.Home -> HomeScreen(app, onOpen = { model.go(Route.Editor) }, onSettings = { model.go(Route.Settings) }, onImport = onImport)
        Route.Editor -> EditorScreen(app, onClose = { model.go(Route.Home) }, onSettings = { model.go(Route.Settings) }, onImport = onImport)
        Route.Settings -> SettingsScreen(app, onBack = model::back)
    }
}
