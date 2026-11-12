package dev.argo.videolab.ui

import android.content.Intent
import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.Clip
import dev.argo.videolab.core.d
import dev.argo.videolab.core.s
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import java.io.File
import kotlin.math.ln
import kotlin.math.pow
import kotlin.math.roundToInt

private fun q(s: String) = JsonPrimitive(s).toString()

@Composable
fun ToolSheet(app: VideoLabApp, tool: Tool, clip: Clip?, playhead: Double, onDone: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Color(0xFF151518)).padding(bottom = 8.dp)) {
        when (tool) {
            Tool.Volume -> clip?.let { VolumeSheet(app, it, onDone) }
            Tool.Speed -> clip?.let { SpeedSheet(app, it, onDone) }
            Tool.Fade -> clip?.let { FadeSheet(app, it, onDone) }
            Tool.Transition -> clip?.let { TransitionSheet(app, it, onDone) }
            Tool.Text -> TextSheet(app, playhead, onDone)
            Tool.Templates -> TemplatesSheet(app, playhead, onDone)
            Tool.Captions -> CaptionSheet(app, playhead, onDone)
            Tool.Export -> ExportSheet(app, onDone)
        }
    }
}

@Composable
private fun VolumeSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var db by remember(clip.id) { mutableFloatStateOf(clip.gainDb.toFloat().coerceIn(-30f, 12f)) }
    SheetHeader("Volume · ${"%+.1f".format(db)} dB", onDone)
    Slider(db, { db = it }, valueRange = -30f..12f, onValueChangeFinished = {
        scope.launch { app.lab.fn("timeline.volume", """{"clips":[${clip.id}],"db":$db}""") }
    }, modifier = Modifier.padding(horizontal = 16.dp))
    Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        TextButton(onClick = { db = 0f; scope.launch { app.lab.fn("timeline.volume", """{"clips":[${clip.id}],"db":0}""") } }) { Text("Reset") }
        TextButton(onClick = { scope.launch { app.lab.fn("timeline.volume", """{"clips":[${clip.id}],"db":-96}""") } }) { Text("Mute") }
    }
}

@Composable
private fun SpeedSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    // Slider position is log2(speed): -2 (0.25x) .. +2 (4x).
    var pos by remember(clip.id) { mutableFloatStateOf((ln(clip.speed.coerceIn(0.25, 4.0)) / ln(2.0)).toFloat()) }
    var reverse by remember(clip.id) { mutableStateOf(false) }
    val speed = 2.0.pow(pos.toDouble())
    fun apply() = scope.launch { app.lab.fn("timeline.speed", """{"clips":[${clip.id}],"percent":${(speed * 100).roundToInt()},"reverse":$reverse}""") }
    SheetHeader("Speed · ${"%.2f".format(speed)}×", onDone)
    Slider(pos, { pos = it }, valueRange = -2f..2f, onValueChangeFinished = { apply() }, modifier = Modifier.padding(horizontal = 16.dp))
    Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        listOf(0.5, 1.0, 2.0).forEach { s -> FilterChip(selected = speed in s - 0.01..s + 0.01, onClick = { pos = (ln(s) / ln(2.0)).toFloat(); apply() }, label = { Text("${s}×") }) }
        Text("Reverse", Modifier.padding(start = 12.dp))
        Switch(reverse, { reverse = it; apply() })
    }
}

@Composable
private fun FadeSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var fadeIn by remember(clip.id) { mutableFloatStateOf(0.5f) }
    var fadeOut by remember(clip.id) { mutableFloatStateOf(0.5f) }
    val max = (clip.duration / 2).toFloat().coerceIn(0.1f, 3f)
    SheetHeader("Fade", onDone)
    Text("In · ${"%.1f".format(fadeIn)} s", Modifier.padding(horizontal = 16.dp), fontSize = 12.sp)
    Slider(fadeIn, { fadeIn = it }, valueRange = 0.1f..max, onValueChangeFinished = {
        scope.launch { app.lab.fn("timeline.fade", """{"clip":${clip.id},"edge":"in","seconds":$fadeIn}""") }
    }, modifier = Modifier.padding(horizontal = 16.dp))
    Text("Out · ${"%.1f".format(fadeOut)} s", Modifier.padding(horizontal = 16.dp), fontSize = 12.sp)
    Slider(fadeOut, { fadeOut = it }, valueRange = 0.1f..max, onValueChangeFinished = {
        scope.launch { app.lab.fn("timeline.fade", """{"clip":${clip.id},"edge":"out","seconds":$fadeOut}""") }
    }, modifier = Modifier.padding(horizontal = 16.dp))
}

@Composable
private fun TransitionSheet(app: VideoLabApp, clip: Clip, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var all by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    LaunchedEffect(Unit) {
        val list = app.lab.io { it.execute("effects.list", """{"kind":"VideoTransition"}""") }.getOrNull()?.let(app.lab::parse) as? JsonArray
        all = list.orEmpty().mapNotNull { it as? JsonObject }.mapNotNull { o -> o.s("id")?.let { id -> id to (o.s("name") ?: id) } }
    }
    val common = listOf("cross_dissolve", "dip_to_black", "dip_to_white", "push", "slide", "wipe", "zoom", "iris_round", "film_dissolve", "cube_spin", "page_peel")
    val shown = all.sortedBy { (id, _) -> common.indexOf(id).let { if (it < 0) 999 else it } }.take(40)
    SheetHeader("Transition into this clip", onDone)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        shown.forEach { (id, name) ->
            FilterChip(selected = false, onClick = {
                scope.launch { app.lab.fn("timeline.transition", """{"clip":${clip.id},"effect":${q(id)},"edge":"in","seconds":0.6}""") }
            }, label = { Text(name) })
        }
    }
}

/** Style presets for titles: FilmCraft graphic properties. */
private val TEXT_STYLES = listOf(
    "Classic" to """{"font":"Inter","font_style":"Bold","size":110,"fill_color":"#ffffff","shadow":true}""",
    "Pop" to """{"font":"Inter","font_style":"Black","size":130,"fill_color":"#ffcc00","stroke":true,"stroke_width":8}""",
    "Boxed" to """{"font":"Inter","font_style":"SemiBold","size":90,"fill_color":"#111111","background":true}""",
    "Neon" to """{"font":"Inter","font_style":"Bold","size":120,"fill_color":"#3fd0e8","stroke":true,"stroke_width":3,"shadow":true}""",
)

@Composable
private fun TextSheet(app: VideoLabApp, playhead: Double, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var style by remember { mutableStateOf(0) }
    SheetHeader("Add text", onDone)
    OutlinedTextField(text, { text = it }, placeholder = { Text("Your title") }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), singleLine = true)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        TEXT_STYLES.forEachIndexed { i, (name, _) -> FilterChip(selected = style == i, onClick = { style = i }, label = { Text(name) }) }
    }
    Button(onClick = {
        val t = text
        scope.launch {
            app.lab.fn("text.add", """{"text":${q(t)},"seconds":$playhead,"duration":3,"style":${TEXT_STYLES[style].second}}""")
            onDone()
        }
    }, enabled = text.isNotBlank(), modifier = Modifier.padding(horizontal = 12.dp)) { Text("Add at ${dev.argo.videolab.core.formatTime(playhead)}") }
}

@Composable
private fun TemplatesSheet(app: VideoLabApp, playhead: Double, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var templates by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var category by remember { mutableStateOf<String?>(null) }
    val thumbs = remember { mutableStateMapOf<String, ImageBitmap>() }
    LaunchedEffect(Unit) {
        templates = (app.lab.io { it.call("text.templates", "{}", "ui") }.getOrNull()?.let(app.lab::parse) as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
        category = templates.firstNotNullOfOrNull { it.s("category") }
        // FilmCraft renders each template's thumbnail itself.
        for (t in templates) {
            val id = t.s("id") ?: t.s("name") ?: continue
            val r = app.lab.io { it.execute("graphics.template.thumbnail", """{"template":${q(id)},"width":320}""") }.getOrNull()?.let(app.lab::parse) as? JsonObject
            val b64 = r?.s("pngBase64") ?: continue
            val bytes = runCatching { Base64.decode(b64, Base64.DEFAULT) }.getOrNull() ?: continue
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { thumbs[id] = it.asImageBitmap() }
        }
    }
    val categories = templates.mapNotNull { it.s("category") }.distinct()
    SheetHeader("Templates", onDone)
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        categories.forEach { c -> FilterChip(selected = c == category, onClick = { category = c }, label = { Text(c) }) }
    }
    LazyVerticalGrid(GridCells.Fixed(2), Modifier.fillMaxWidth().heightIn(max = 260.dp).padding(horizontal = 12.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        items(templates.filter { it.s("category") == category }) { t ->
            val id = t.s("id") ?: t.s("name").orEmpty()
            Column(Modifier.clip(RoundedCornerShape(8.dp)).background(Color(0xFF222226)).clickable {
                scope.launch {
                    app.lab.fn("text.applyTemplate", """{"template":${q(id)},"seconds":$playhead}""")
                    onDone()
                }
            }) {
                Box(Modifier.fillMaxWidth().aspectRatio(16f / 9f).background(Color.Black), contentAlignment = Alignment.Center) {
                    thumbs[id]?.let { Image(it, null, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxWidth()) }
                }
                Text(t.s("name") ?: id, fontSize = 11.sp, maxLines = 1, modifier = Modifier.padding(6.dp))
            }
        }
    }
}

@Composable
private fun CaptionSheet(app: VideoLabApp, playhead: Double, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    SheetHeader("Caption at ${dev.argo.videolab.core.formatTime(playhead)}", onDone)
    Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        OutlinedTextField(text, { text = it }, placeholder = { Text("What is said here") }, modifier = Modifier.weight(1f), singleLine = true)
        Button(onClick = {
            val t = text
            text = ""
            scope.launch { app.lab.fn("text.caption", """{"text":${q(t)},"seconds":$playhead,"duration":2.5}""") }
        }, enabled = text.isNotBlank(), modifier = Modifier.padding(start = 8.dp)) { Text("Add") }
    }
    Text("Ask the agent to caption a whole video from its dialogue.", color = Color.Gray, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
}

@Composable
private fun ExportSheet(app: VideoLabApp, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val timeline = app.lab.state.value.timeline
    val heights = listOf(480, 720, 1080)
    var res by remember { mutableStateOf(720) }
    var format by remember { mutableStateOf("mp4") }
    var job by remember { mutableStateOf<Long?>(null) }
    var progress by remember { mutableFloatStateOf(0f) }
    var status by remember { mutableStateOf("") }
    var done by remember { mutableStateOf<File?>(null) }
    val short = minOf(timeline?.width ?: 1080, timeline?.height ?: 1920).coerceAtLeast(1)
    SheetHeader("Export", onDone)
    Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        heights.forEach { h -> FilterChip(selected = res == h, onClick = { res = h }, label = { Text("${h}p") }, enabled = job == null) }
        listOf("mp4", "gif").forEach { f -> FilterChip(selected = format == f, onClick = { format = f }, label = { Text(f.uppercase()) }, enabled = job == null) }
    }
    if (job != null || done != null) {
        LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp))
        Text(status, fontSize = 12.sp, color = Color.Gray, modifier = Modifier.padding(horizontal = 16.dp))
    }
    Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (done == null) {
            Button(enabled = job == null && timeline != null, onClick = {
                val name = (timeline?.name ?: "video").replace(Regex("[^A-Za-z0-9_-]"), "_")
                val path = "exports/$name-${res}p-${System.currentTimeMillis() / 1000}.$format"
                val scale = (res.toDouble() / short).coerceAtMost(1.0)
                scope.launch {
                    val r = app.lab.fn("export.start", """{"path":${q(path)},"scale":$scale${if (format == "gif") ""","format":"gif"""" else ""}}""")
                    val o = r.getOrNull() as? JsonObject ?: return@launch
                    job = (o["job"] as? JsonPrimitive)?.content?.toLongOrNull()
                    val file = File(app.workspace, path)
                    while (job != null) {
                        delay(400)
                        val jobs = app.lab.io { it.call("export.jobs", "{}", "ui") }.getOrNull()?.let(app.lab::parse) as? JsonArray
                        val j = jobs.orEmpty().mapNotNull { it as? JsonObject }.firstOrNull { (it["id"] as? JsonPrimitive)?.content?.toLongOrNull() == job } ?: break
                        progress = (j.d("progress") ?: 0.0).toFloat()
                        val eta = j.d("etaSeconds")
                        status = (j.s("status") ?: "") + (eta?.let { " · ${it.roundToInt()} s left" } ?: "")
                        if ((j["finished"] as? JsonPrimitive)?.booleanOrNull == true) {
                            val err = (j["result"] as? JsonObject)?.s("error")
                            status = err?.let { "Failed: $it" } ?: "Saved to ${file.name}"
                            if (err == null) { done = file; progress = 1f }
                            job = null
                        }
                    }
                }
            }) { Text("Export") }
            if (job != null) TextButton(onClick = { val j = job; scope.launch { app.lab.fn("export.cancel", """{"job":$j}""") }; job = null; status = "Cancelled" }) { Text("Cancel") }
        } else {
            Button(onClick = {
                val f = done ?: return@Button
                val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", f)
                val send = Intent(Intent.ACTION_SEND).setType(if (f.extension == "gif") "image/gif" else "video/mp4")
                    .putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                context.startActivity(Intent.createChooser(send, "Share video"))
            }) { Text("Share") }
            TextButton(onClick = { done = null; progress = 0f; status = "" }) { Text("Export again") }
        }
    }
}
