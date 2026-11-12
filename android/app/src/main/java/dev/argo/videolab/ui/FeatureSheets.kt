package dev.argo.videolab.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.Clip
import dev.argo.videolab.core.d
import dev.argo.videolab.core.formatTime
import dev.argo.videolab.core.l
import dev.argo.videolab.core.s
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlin.math.roundToInt

private fun q(s: String) = JsonPrimitive(s).toString()

private suspend fun VideoLabApp.obj(fn: String, params: String): JsonObject? = lab.fn(fn, params).getOrNull() as? JsonObject
private suspend fun VideoLabApp.arr(fn: String, params: String = "{}"): List<JsonObject> =
    (lab.fn(fn, params).getOrNull() as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }

// ── Filters ───────────────────────────────────────────────────────────────────────────────────

/** FilmCraft's grading presets as a strip of swatches (cut from the grid FilmCraft renders). */
@Composable
fun FilterSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var presets by remember { mutableStateOf<List<String>>(emptyList()) }
    val swatches = remember { mutableStateMapOf<String, ImageBitmap>() }
    var chosen by remember(clip.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(clip.id) {
        val has = app.obj("look.state", """{"clip":${clip.id}}""")?.get("filter") as? JsonPrimitive
        if (has?.booleanOrNull == false) chosen = ""
    }
    LaunchedEffect(Unit) {
        val t = app.obj("look.filterThumbnails", """{"width":160}""") ?: return@LaunchedEffect
        val names = (t["presets"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }
        presets = names
        val cell = (t["cell"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.doubleOrNull?.toInt() }.orEmpty()
        val cols = t.l("columns")?.toInt() ?: 6
        val gap = t.l("gap")?.toInt() ?: 4
        val grid = withContext(Dispatchers.IO) { t.s("path")?.let { BitmapFactory.decodeFile(it) } } ?: return@LaunchedEffect
        if (cell.size < 2) return@LaunchedEffect
        val (w, h) = cell[0] to cell[1]
        names.forEachIndexed { i, n ->
            val x = gap + (i % cols) * (w + gap)
            val y = gap + (i / cols) * (h + gap)
            if (x + w <= grid.width && y + h <= grid.height) swatches[n] = Bitmap.createBitmap(grid, x, y, w, h).asImageBitmap()
        }
    }
    fun apply(name: String) {
        chosen = name
        scope.launch { app.lab.fn("look.filter", """{"clips":[${clip.id}],"name":${if (name.isEmpty()) "null" else q(name)}}""") }
    }
    SheetHeader("Filters", onDone)
    LazyRow(Modifier.fillMaxWidth().padding(vertical = 6.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        item { Swatch("None", null, chosen == "") { apply("") } }
        items(presets) { n -> Swatch(n, swatches[n], chosen == n) { apply(n) } }
    }
    Text("FilmCraft colour grading. Adjustments are kept when you change the filter.", color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
}

@Composable
private fun Swatch(name: String, image: ImageBitmap?, selected: Boolean, onClick: () -> Unit) {
    Column(Modifier.width(76.dp).clickable(onClick = onClick), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.size(72.dp, 72.dp).clip(RoundedCornerShape(8.dp)).background(Color(0xFF26262B))
                .then(if (selected) Modifier.border(2.dp, Accent, RoundedCornerShape(8.dp)) else Modifier),
            contentAlignment = Alignment.Center,
        ) {
            if (image != null) Image(image, name, contentScale = ContentScale.Crop, modifier = Modifier.size(72.dp))
            else if (name == "None") SymIcon(Sym.close, null, Modifier.size(24.dp), tint = Color.Gray)
        }
        Text(name, fontSize = 10.sp, maxLines = 2, textAlign = TextAlign.Center, color = if (selected) Accent else Color(0xFFDDDDE2), modifier = Modifier.padding(top = 3.dp))
    }
}

// ── Effects ───────────────────────────────────────────────────────────────────────────────────

/** The effects shelf: tap to turn an effect on the clip on or off. */
@Composable
fun EffectsSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var shelf by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    var on by remember(clip.id) { mutableStateOf<Set<String>>(emptySet()) }
    suspend fun refresh() {
        on = (app.obj("look.state", """{"clip":${clip.id}}""")?.get("effects") as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }.toSet()
    }
    LaunchedEffect(clip.id) {
        shelf = app.arr("look.effects").mapNotNull { o -> o.s("key")?.let { it to (o.s("name") ?: it) } }
        refresh()
    }
    SheetHeader("Effects", onDone)
    FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        shelf.forEach { (key, name) ->
            FilterChip(selected = key in on, onClick = {
                on = if (key in on) on - key else on + key
                scope.launch {
                    app.lab.fn("look.effect", """{"clips":[${clip.id}],"key":${q(key)}}""")
                    refresh()
                }
            }, label = { Text(name) })
        }
    }
}

// ── Adjust ────────────────────────────────────────────────────────────────────────────────────

private data class AdjustKnob(val key: String, val label: String, val min: Float, val max: Float, val neutral: Float)

private val KNOBS = listOf(
    AdjustKnob("exposure", "Brightness", -2f, 2f, 0f),
    AdjustKnob("contrast", "Contrast", -100f, 100f, 0f),
    AdjustKnob("saturation", "Saturation", 0f, 200f, 100f),
    AdjustKnob("temperature", "Warmth", -100f, 100f, 0f),
    AdjustKnob("tint", "Tint", -100f, 100f, 0f),
    AdjustKnob("highlights", "Highlights", -100f, 100f, 0f),
    AdjustKnob("shadows", "Shadows", -100f, 100f, 0f),
    AdjustKnob("vibrance", "Vibrance", -100f, 100f, 0f),
    AdjustKnob("sharpen", "Sharpen", 0f, 100f, 0f),
    AdjustKnob("fade", "Fade", 0f, 100f, 0f),
    AdjustKnob("vignette", "Vignette", -100f, 0f, 0f),
)

/** One slider at a time, chosen from a row of knobs (as phone editors do). */
@Composable
fun AdjustSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val values = remember(clip.id) { mutableStateMapOf<String, Float>() }
    var knob by remember { mutableStateOf(KNOBS[0]) }
    LaunchedEffect(clip.id) {
        val adj = app.obj("look.state", """{"clip":${clip.id}}""")?.get("adjust") as? JsonObject ?: return@LaunchedEffect
        KNOBS.forEach { k -> (adj[k.key] as? JsonPrimitive)?.doubleOrNull?.let { values[k.key] = it.toFloat() } }
    }
    fun send(vararg kv: Pair<String, Float>) = scope.launch {
        app.lab.fn("look.adjust", """{"clip":${clip.id},"values":{${kv.joinToString(",") { (k, v) -> "${q(k)}:$v" }}}}""")
    }
    val v = values[knob.key] ?: knob.neutral
    SheetHeader("Adjust · ${knob.label} ${(v - knob.neutral).let { if (knob.max - knob.min <= 10f) "%+.1f".format(it) else "%+d".format(it.roundToInt()) }}", onDone)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        KNOBS.forEach { k ->
            val changed = (values[k.key] ?: k.neutral) != k.neutral
            FilterChip(selected = k == knob, onClick = { knob = k }, label = { Text(if (changed) "${k.label} •" else k.label) })
        }
    }
    Slider(v, { values[knob.key] = it }, valueRange = knob.min..knob.max, onValueChangeFinished = { send(knob.key to (values[knob.key] ?: knob.neutral)) },
        modifier = Modifier.padding(horizontal = 16.dp))
    Row(Modifier.padding(horizontal = 12.dp)) {
        TextButton(onClick = { values[knob.key] = knob.neutral; send(knob.key to knob.neutral) }) { Text("Reset ${knob.label.lowercase()}") }
        TextButton(onClick = {
            KNOBS.forEach { values[it.key] = it.neutral }
            send(*KNOBS.map { it.key to it.neutral }.toTypedArray())
        }) { Text("Reset all") }
    }
}

// ── Transform / keyframes ─────────────────────────────────────────────────────────────────────

/**
 * Position, scale, rotation and opacity. With keyframes (the ◇ button), each change sets the
 * keyframe at the playhead, so the picture animates between them.
 */
@Composable
fun TransformSheet(app: VideoLabApp, clip: Clip, playhead: Double, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val timeline = app.lab.state.value.timeline
    val w = (timeline?.width ?: 1080).toFloat()
    val h = (timeline?.height ?: 1920).toFloat()
    var scale by remember(clip.id) { mutableFloatStateOf(100f) }
    var rotation by remember(clip.id) { mutableFloatStateOf(0f) }
    var opacity by remember(clip.id) { mutableFloatStateOf(100f) }
    var x by remember(clip.id) { mutableFloatStateOf(w / 2) }
    var y by remember(clip.id) { mutableFloatStateOf(h / 2) }
    var animated by remember(clip.id) { mutableStateOf(false) }
    LaunchedEffect(clip.id, clip.keyframes, playhead) {
        val t = app.obj("clip.transform", """{"clip":${clip.id},"seconds":$playhead}""") ?: return@LaunchedEffect
        t.d("scale")?.let { scale = it.toFloat() }
        t.d("rotation")?.let { rotation = it.toFloat() }
        t.d("opacity")?.let { opacity = it.toFloat() }
        (t["position"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.doubleOrNull }?.takeIf { it.size == 2 }?.let { x = it[0].toFloat(); y = it[1].toFloat() }
        animated = (t["animated"] as? JsonPrimitive)?.booleanOrNull == true
    }
    fun send(field: String, value: String) = scope.launch {
        app.lab.fn("clip.setTransform", """{"clip":${clip.id},"seconds":$playhead,${q(field)}:$value}""")
    }
    SheetHeader(if (animated) "Transform · keyframe at ${formatTime(playhead, tenths = true)}" else "Transform", onDone)
    Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        listOf("fit" to "Fit", "fill" to "Fill").forEach { (mode, label) ->
            FilterChip(selected = false, onClick = {
                scope.launch {
                    app.lab.fn("clip.fit", """{"clips":[${clip.id}],"mode":"$mode"}""")
                    app.obj("clip.transform", """{"clip":${clip.id},"seconds":$playhead}""")?.d("scale")?.let { scale = it.toFloat() }
                }
            }, label = { Text(label) })
        }
    }
    LabeledSlider("Scale ${scale.roundToInt()}%", scale, 10f..400f) { scale = it; send("scale", "$it") }
    LabeledSlider("Rotate ${rotation.roundToInt()}°", rotation, -180f..180f) { rotation = it; send("rotation", "$it") }
    LabeledSlider("Opacity ${opacity.roundToInt()}%", opacity, 0f..100f) { opacity = it; send("opacity", "$it") }
    LabeledSlider("Left / right", x, -w * 0.5f..w * 1.5f) { x = it; send("position", "[$it,$y]") }
    LabeledSlider("Up / down", y, -h * 0.5f..h * 1.5f) { y = it; send("position", "[$x,$it]") }
    Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Button(onClick = { scope.launch { app.lab.fn("clip.keyframe", """{"clip":${clip.id},"seconds":$playhead}""") } }) {
            SymIcon(Sym.keyframe, null, Modifier.size(16.dp), tint = Color.Black)
            Text("  Keyframe here")
        }
        TextButton(onClick = {
            scale = 100f; rotation = 0f; opacity = 100f; x = w / 2; y = h / 2
            scope.launch { app.lab.fn("clip.setTransform", """{"clip":${clip.id},"seconds":$playhead,"scale":100,"rotation":0,"opacity":100,"position":[${w / 2},${h / 2}]}""") }
        }) { Text("Reset") }
    }
    Text(
        if (animated) "${clip.keyframes.size} keyframes: changes here set the keyframe at the playhead." else "Add a keyframe, move the playhead, change something: the picture animates between them.",
        color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp),
    )
}

@Composable
private fun LabeledSlider(label: String, value: Float, range: ClosedFloatingPointRange<Float>, onDone: (Float) -> Unit) {
    var v by remember(value) { mutableFloatStateOf(value) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).height(36.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, fontSize = 11.sp, modifier = Modifier.width(96.dp), color = Color(0xFFCCCCD2))
        Slider(v.coerceIn(range), { v = it }, valueRange = range, onValueChangeFinished = { onDone(v) }, modifier = Modifier.weight(1f))
    }
}

// ── Text to speech ────────────────────────────────────────────────────────────────────────────

@Composable
fun NarrateSheet(app: VideoLabApp, playhead: Double, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var voices by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    var voice by remember { mutableStateOf<String?>(null) }
    var pace by remember { mutableFloatStateOf(1f) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        voices = app.arr("audio.voices").mapNotNull { o -> o.s("id")?.let { it to (o.s("name") ?: it) } }
        voice = voices.firstOrNull()?.first
    }
    SheetHeader("Text to speech", onDone)
    OutlinedTextField(text, { text = it }, placeholder = { Text("What should be said") }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), minLines = 2, maxLines = 4)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        voices.forEach { (id, name) -> FilterChip(selected = voice == id, onClick = { voice = id }, label = { Text(name) }) }
    }
    LabeledSlider("Pace ${"%.1f".format(pace)}×", pace, 0.5f..2f) { pace = it }
    Button(onClick = {
        val t = text
        busy = true
        scope.launch {
            app.lab.fn("audio.narrate", """{"text":${q(t)},"seconds":$playhead,"pace":$pace${voice?.let { ""","voice":${q(it)}""" } ?: ""}}""")
            busy = false
            onDone()
        }
    }, enabled = text.isNotBlank() && !busy, modifier = Modifier.padding(horizontal = 12.dp)) { Text(if (busy) "Speaking…" else "Add at ${formatTime(playhead)}") }
}
