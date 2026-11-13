package dev.argo.videolab.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.Lab
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Settings: agent (model, sealed API key, tools), engine (GPU), Developer (MCP server, FilmCraft control channel, CLI console). */
@Composable
fun SettingsScreen(app: VideoLabApp, onBack: () -> Unit) {
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                SymButton(Sym.chevronLeft, "Back", onBack)
                Text("Settings", style = MaterialTheme.typography.titleLarge)
            }
            Column(
                Modifier.fillMaxWidth().weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).widthIn(max = 720.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                AgentSettings(app)
                HorizontalDivider()
                EngineSettings(app)
                HorizontalDivider()
                Section("Developer")
                McpSettings(app)
                ControlChannel(app)
                Text("FilmCraft CLI console", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 8.dp))
                CliConsole(app, Modifier.fillMaxWidth().height(360.dp))
            }
        }
    }
}

@Composable
private fun Section(title: String) {
    Text(title, style = MaterialTheme.typography.titleMedium, color = Accent, modifier = Modifier.padding(top = 12.dp))
}

@Composable
private fun AgentSettings(app: VideoLabApp) {
    val settings = app.settings
    val scope = rememberCoroutineScope()
    var key by remember { mutableStateOf("") }
    var loaded by remember { mutableStateOf(false) }
    var model by remember { mutableStateOf(settings.model) }
    var saved by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        key = withContext(Dispatchers.IO) { runCatching { settings.apiKey }.getOrDefault("") }
        loaded = true
    }
    Section("Agent")
    OutlinedTextField(
        key, { key = it; saved = null },
        label = { Text("Anthropic API key") },
        singleLine = true,
        enabled = loaded,
        visualTransformation = PasswordVisualTransformation(),
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(model, { model = it; saved = null }, label = { Text("Model") }, singleLine = true, modifier = Modifier.fillMaxWidth())
    var fcTools by remember { mutableStateOf(settings.filmcraftTools) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("FilmCraft MCP tools for the agent")
            Text("command_run, render_frame, doc_inspect… from FilmCraft's own MCP server (bridge mode). Next conversation.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
        }
        Switch(checked = fcTools, onCheckedChange = { fcTools = it; settings.filmcraftTools = it })
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Button(onClick = {
            scope.launch {
                val r = withContext(Dispatchers.IO) { runCatching { settings.apiKey = key; settings.model = model } }
                saved = if (r.isSuccess) "Saved. The key is sealed with the Android Keystore." else "Could not save: ${r.exceptionOrNull()?.javaClass?.simpleName}"
            }
        }, enabled = loaded) { Text("Save") }
        saved?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = Color.Gray, modifier = Modifier.padding(start = 12.dp)) }
    }
}

@Composable
private fun EngineSettings(app: VideoLabApp) {
    val scope = rememberCoroutineScope()
    var gpu by remember { mutableStateOf(app.settings.gpu) }
    var gpuActive by remember { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(Unit) { gpuActive = app.lab.io { it.gpuStatus().getOrNull(1) }.getOrNull() }
    Section("Engine")
    var stats by remember { mutableStateOf(app.settings.showStats) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("Show playback stats on the preview")
            Text("Renderer, hardware-decoded frames and frame rate, for checking performance.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
        }
        Switch(checked = stats, onCheckedChange = { stats = it; app.settings.showStats = it })
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("GPU rendering (FilmCraft's wgpu compositor on Vulkan)")
            Text(
                "Previews and exports. Probed with one frame first; falls back to the CPU renderer." + when (gpuActive) {
                    true -> " Currently on."
                    false -> " Currently off (not available or turned off)."
                    null -> ""
                },
                style = MaterialTheme.typography.bodySmall, color = Color.Gray,
            )
        }
        Switch(checked = gpu, onCheckedChange = { on ->
            gpu = on
            app.settings.gpu = on
            scope.launch {
                app.lab.io { it.setGpu(on) }
                gpuActive = app.lab.io { it.gpuStatus().getOrNull(1) }.getOrNull()
                app.lab.requestRefresh()
            }
        })
    }
}

/** MCP over loopback HTTP, for agents on a laptop via `adb forward`. */
@Composable
private fun McpSettings(app: VideoLabApp) {
    val settings = app.settings
    val scope = rememberCoroutineScope()
    var running by remember { mutableStateOf(false) }
    var allowEdits by remember { mutableStateOf(settings.mcpAllowEdits) }
    var argo by remember { mutableStateOf(settings.argoCompanion) }
    var token by remember { mutableStateOf(settings.mcpToken) }
    var error by remember { mutableStateOf<String?>(null) }
    val port = settings.mcpPort
    LaunchedEffect(Unit) { running = app.lab.io { it.mcpHttpPort() != null }.getOrDefault(false) }

    fun start() = scope.launch {
        error = null
        app.lab.io { it.startMcpHttp(port.toUShort(), token, allowEdits) }
            .onSuccess { running = true }
            .onFailure { error = Lab.describe(it); running = false }
    }

    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("Let ARGO use VideoLab")
            Text("The ARGO app's agent can edit videos here (its tools appear there as videolab__…). Only ARGO's own signed app can connect.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
        }
        Switch(checked = argo, onCheckedChange = { argo = it; settings.argoCompanion = it })
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("MCP server on 127.0.0.1:$port")
            Text("VideoLab's tools at /mcp; with edits allowed, FilmCraft's own MCP server at /filmcraft/mcp. On the document open here.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
        }
        Switch(checked = running, onCheckedChange = { on ->
            if (on) start() else scope.launch { app.lab.io { it.stopMcpHttp() }; running = false }
        })
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("Allow edits")
            Text("Off: only lab_read and lab_help; FilmCraft's endpoint is refused.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
        }
        Switch(checked = allowEdits, onCheckedChange = {
            allowEdits = it
            settings.mcpAllowEdits = it
            if (running) start()
        })
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    SelectionContainer {
        Text(
            "adb forward tcp:$port tcp:$port\n" +
                "claude mcp add --transport http videolab http://localhost:$port/mcp \\\n  --header \"Authorization: Bearer $token\"\n" +
                "claude mcp add --transport http filmcraft http://localhost:$port/filmcraft/mcp \\\n  --header \"Authorization: Bearer $token\"",
            fontFamily = FontFamily.Monospace, fontSize = 11.sp,
            modifier = Modifier.fillMaxWidth().background(Color(0xFF151517), RoundedCornerShape(4.dp)).padding(8.dp),
        )
    }
    TextButton(onClick = {
        token = settings.rotateMcpToken()
        if (running) start()
    }) { Text("Rotate token") }
}

/** FilmCraft's control channel: `filmcraft-cli --bridge` from a laptop drives this document too. */
@Composable
private fun ControlChannel(app: VideoLabApp) {
    var info by remember { mutableStateOf<Pair<Int, String>?>(null) }
    LaunchedEffect(Unit) { info = app.lab.io { it.controlInfo() }.getOrNull()?.let { it.port.toInt() to it.token } }
    val (port, token) = info ?: return
    Text("FilmCraft control channel on 127.0.0.1:$port", modifier = Modifier.padding(top = 8.dp))
    Text("The protocol FilmCraft's desktop app speaks (filmcraft --control). FilmCraft's MCP bridge and CLI drive this document through it.", style = MaterialTheme.typography.bodySmall, color = Color.Gray)
    SelectionContainer {
        Text(
            "adb forward tcp:$port tcp:$port\n" +
                "export FILMCRAFT_CONTROL_TOKEN=$token\n" +
                "filmcraft-cli --bridge 127.0.0.1:$port inspect sequence\n" +
                "filmcraft-cli mcp --bridge 127.0.0.1:$port   # stdio MCP\n\n" +
                "On the phone: adb shell ${app.cliPath} --help",
            fontFamily = FontFamily.Monospace, fontSize = 11.sp,
            modifier = Modifier.fillMaxWidth().background(Color(0xFF151517), RoundedCornerShape(4.dp)).padding(8.dp),
        )
    }
}

private const val MAX_CONSOLE_LINES = 1500

/** A console for FilmCraft's own `filmcraft-cli` (bundled), bridged to the document on screen. */
@Composable
private fun CliConsole(app: VideoLabApp, modifier: Modifier) {
    val lines = remember { mutableStateListOf("filmcraft-cli --bridge (this document) in ${app.workspace.name}/ — try: commands razor · describe timeline.trim · inspect sequence · exec timeline.razor seconds=2 · help") }
    var input by remember { mutableStateOf("") }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(lines.size) { listState.animateScrollToItem(lines.size - 1) }
    Column(modifier.background(Color(0xFF151517), RoundedCornerShape(4.dp)).padding(8.dp)) {
        SelectionContainer(Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize(), state = listState) {
                // One row per line: a single huge Text can exceed the GPU's texture size.
                items(lines) { Text(it, fontFamily = FontFamily.Monospace, fontSize = 11.sp) }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                input, { input = it }, prefix = { Text("$ ") }, singleLine = true, modifier = Modifier.weight(1f),
                textStyle = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            )
            Button(enabled = input.isNotBlank(), onClick = {
                val cmd = input
                input = ""
                lines += "$ $cmd"
                scope.launch {
                    val out = app.lab.io { it.cli(cmd, 120_000UL) }
                    out.onSuccess { r ->
                        for (stream in listOf(r.stdout, r.stderr)) {
                            val all = stream.trimEnd().lines().filter { it.isNotEmpty() }
                            lines += all.take(MAX_CONSOLE_LINES).map { it.take(400) }
                            if (all.size > MAX_CONSOLE_LINES) lines += "… ${all.size - MAX_CONSOLE_LINES} more lines"
                        }
                        lines += "[exit ${r.code}]"
                    }.onFailure { lines += "error: ${Lab.describe(it)}" }
                    app.lab.requestRefresh()
                }
            }, modifier = Modifier.padding(start = 6.dp)) { Text("Run") }
        }
    }
}
