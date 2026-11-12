package dev.argo.videolab.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.Clip
import dev.argo.videolab.core.d
import dev.argo.videolab.core.s
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlin.math.roundToInt

/*
 * Sheets for the compose tools (the core's `compose.*` and `project.*`): crop, opacity and blend,
 * chroma key, mask, voice effects and the canvas. Each reads the clip's state when it opens and
 * sends a change when a slider is let go or a chip is tapped.
 */

private fun q(s: String) = JsonPrimitive(s).toString()

private suspend fun VideoLabApp.state(clip: Clip): JsonObject? = lab.fn("compose.state", """{"clip":${clip.id}}""").getOrNull() as? JsonObject

@Composable
private fun Knob(label: String, value: Float, range: ClosedFloatingPointRange<Float>, onDone: (Float) -> Unit) {
    var v by remember(value) { mutableFloatStateOf(value) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).height(36.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("$label ${v.roundToInt()}", fontSize = 11.sp, modifier = Modifier.width(110.dp), color = Color(0xFFCCCCD2))
        Slider(v.coerceIn(range), { v = it }, valueRange = range, onValueChangeFinished = { onDone(v) }, modifier = Modifier.weight(1f))
    }
}

// ── Crop ──────────────────────────────────────────────────────────────────────────────────────

@Composable
fun CropSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var edges by remember(clip.id) { mutableStateOf(mapOf("left" to 0f, "top" to 0f, "right" to 0f, "bottom" to 0f)) }
    LaunchedEffect(clip.id) {
        val c = app.state(clip)?.get("crop") as? JsonObject ?: return@LaunchedEffect
        edges = edges.keys.associateWith { (c.d(it) ?: 0.0).toFloat() }
    }
    fun send(e: Map<String, Float>) = scope.launch {
        app.lab.fn("compose.crop", """{"clip":${clip.id},${e.entries.joinToString(",") { (k, v) -> "${q(k)}:$v" }}}""")
    }
    SheetHeader("Crop", onDone)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        // quick crops: trim the sides to a shape (of a 16:9 picture), or none
        listOf("None" to listOf(0f, 0f, 0f, 0f), "Square" to listOf(21.9f, 0f, 21.9f, 0f), "Vertical" to listOf(34.2f, 0f, 34.2f, 0f), "Letterbox" to listOf(0f, 12f, 0f, 12f)).forEach { (label, v) ->
            FilterChip(selected = false, onClick = {
                val e = mapOf("left" to v[0], "top" to v[1], "right" to v[2], "bottom" to v[3])
                edges = e
                send(e)
            }, label = { Text(label) })
        }
    }
    listOf("left" to "Left", "right" to "Right", "top" to "Top", "bottom" to "Bottom").forEach { (k, label) ->
        Knob("$label %", edges[k] ?: 0f, 0f..45f) { v -> edges = edges + (k to v); send(edges) }
    }
}

// ── Opacity and blend ─────────────────────────────────────────────────────────────────────────

@Composable
fun BlendSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var blends by remember { mutableStateOf<List<String>>(emptyList()) }
    var blend by remember(clip.id) { mutableStateOf("Normal") }
    var opacity by remember(clip.id) { mutableFloatStateOf(100f) }
    LaunchedEffect(clip.id) {
        val st = app.state(clip) ?: return@LaunchedEffect
        blends = (st["blends"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }
        blend = st.s("blend") ?: "Normal"
        opacity = (st.d("opacity") ?: 100.0).toFloat()
    }
    SheetHeader("Opacity & blend", onDone)
    Knob("Opacity %", opacity, 0f..100f) { v -> opacity = v; scope.launch { app.lab.fn("compose.blend", """{"clips":[${clip.id}],"opacity":$v}""") } }
    FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        blends.forEach { b ->
            FilterChip(selected = b == blend, onClick = { blend = b; scope.launch { app.lab.fn("compose.blend", """{"clips":[${clip.id}],"mode":${q(b)}}""") } }, label = { Text(b) })
        }
    }
    Text("Put a clip on a track above another (drag it up) to see it blend over the one below.", color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
}

// ── Chroma key ────────────────────────────────────────────────────────────────────────────────

private val KEY_COLORS = listOf("#00b140" to Color(0xFF00B140), "#0047bb" to Color(0xFF0047BB), "#000000" to Color.Black, "#ffffff" to Color.White, "#ff0000" to Color.Red)

@Composable
fun ChromaKeySheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var on by remember(clip.id) { mutableStateOf(false) }
    var color by remember(clip.id) { mutableStateOf("#00b140") }
    var strength by remember(clip.id) { mutableFloatStateOf(50f) }
    LaunchedEffect(clip.id) {
        val k = app.state(clip)?.get("chromaKey") as? JsonObject
        on = k != null
        k?.s("color")?.let { color = it }
        k?.d("strength")?.let { strength = it.toFloat() }
    }
    fun send(extra: String) = scope.launch { app.lab.fn("compose.chromaKey", """{"clip":${clip.id}$extra}""") }
    SheetHeader("Chroma key", onDone)
    Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Remove a colour", fontSize = 13.sp, modifier = Modifier.weight(1f))
        Switch(on, { v -> on = v; send(if (v) ""","color":${q(color)},"strength":$strength""" else ""","on":false""") })
    }
    Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        KEY_COLORS.forEach { (hex, c) ->
            Box(
                Modifier.size(32.dp).clip(CircleShape).background(c)
                    .border(if (hex == color) 3.dp else 1.dp, if (hex == color) Accent else Color.Gray, CircleShape)
                    .clickable { color = hex; on = true; send(""","color":${q(hex)}""") },
            )
        }
    }
    Knob("Strength", strength, 0f..100f) { v -> strength = v; on = true; send(""","strength":$v""") }
    Text("Green screen: the chosen colour turns transparent and the track below shows through.", color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
}

// ── Mask ──────────────────────────────────────────────────────────────────────────────────────

@Composable
fun MaskSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var shape by remember(clip.id) { mutableStateOf("none") }
    var size by remember(clip.id) { mutableFloatStateOf(60f) }
    var feather by remember(clip.id) { mutableFloatStateOf(20f) }
    var inverted by remember(clip.id) { mutableStateOf(false) }
    LaunchedEffect(clip.id) {
        val m = app.state(clip)?.get("mask") as? JsonObject ?: return@LaunchedEffect
        shape = m.s("shape") ?: "none"
        inverted = (m["inverted"] as? JsonPrimitive)?.booleanOrNull == true
    }
    fun send() = scope.launch {
        app.lab.fn("compose.mask", """{"clip":${clip.id},"shape":${q(shape)},"size":$size,"feather":$feather,"inverted":$inverted}""")
    }
    SheetHeader("Mask", onDone)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        listOf("none" to "None", "circle" to "Circle", "rectangle" to "Rectangle", "rounded" to "Rounded", "band" to "Band").forEach { (k, label) ->
            FilterChip(selected = shape == k, onClick = { shape = k; send() }, label = { Text(label) })
        }
    }
    if (shape != "none") {
        Knob("Size %", size, 10f..120f) { v -> size = v; send() }
        Knob("Feather", feather, 0f..100f) { v -> feather = v; send() }
        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Invert (show the outside)", fontSize = 13.sp, modifier = Modifier.weight(1f))
            Switch(inverted, { inverted = it; send() })
        }
    }
}

// ── Voice effects ─────────────────────────────────────────────────────────────────────────────

@Composable
fun VoiceSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var voices by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    var chosen by remember(clip.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        voices = (app.lab.fn("compose.voices").getOrNull() as? JsonArray).orEmpty().mapNotNull { o -> (o as? JsonObject)?.let { j -> j.s("key")?.let { it to (j.s("name") ?: it) } } }
    }
    fun pick(key: String) {
        chosen = key
        scope.launch { app.lab.fn("compose.voice", """{"clips":[${clip.id}],"key":${q(key)}}""") }
    }
    SheetHeader("Voice effects", onDone)
    FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        FilterChip(selected = chosen == "none", onClick = { pick("none") }, label = { Text("None") })
        voices.forEach { (k, name) -> FilterChip(selected = chosen == k, onClick = { pick(k) }, label = { Text(name) }) }
    }
    TextButton(onClick = { scope.launch { app.lab.fn("compose.enhanceVoice", """{"clips":[${clip.id}]}""") } }, modifier = Modifier.padding(horizontal = 8.dp)) {
        Text("Enhance voice (reduce noise) — on / off")
    }
}

// ── Canvas ────────────────────────────────────────────────────────────────────────────────────

@Composable
fun CanvasSheet(app: VideoLabApp, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var ratios by remember { mutableStateOf<List<String>>(emptyList()) }
    var ratio by remember { mutableStateOf<String?>(null) }
    var size by remember { mutableStateOf("") }
    suspend fun read() {
        val c = app.lab.fn("project.canvas").getOrNull() as? JsonObject ?: return
        ratios = (c["ratios"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }
        ratio = c.s("ratio")
        size = "${c.d("width")?.toInt()} × ${c.d("height")?.toInt()}"
    }
    LaunchedEffect(Unit) { read() }
    SheetHeader("Canvas · $size", onDone)
    FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        ratios.forEach { r ->
            FilterChip(selected = r == ratio, onClick = {
                ratio = r
                scope.launch { app.lab.fn("project.setCanvas", """{"ratio":${q(r)}}"""); read() }
            }, label = { Text(r) })
        }
    }
    Text("Clips are fitted to the new shape. Auto reframe (on a clip) keeps the action in view.", color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
}
