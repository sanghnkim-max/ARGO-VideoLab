package dev.argo.videolab.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameNanos
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.VideoLabApp
import dev.argo.videolab.core.ClipKind
import dev.argo.videolab.core.formatTime
import kotlinx.coroutines.launch

/** Bottom-bar tools; a non-null tool opens its sheet in place of the bar. */
enum class Tool { Volume, Speed, Fade, Transition, Text, Templates, Captions, Export, Filters, Effects, Adjust, Transform, Narrate, Crop, Blend, ChromaKey, Mask, Voice, Canvas }

@Composable
fun EditorScreen(app: VideoLabApp, onClose: () -> Unit, onSettings: () -> Unit, onImport: (List<Uri>, Boolean) -> Unit) {
    val lab = app.lab
    val player = app.player
    val state by lab.state.collectAsState()
    val playing by player.playing.collectAsState()
    val scope = rememberCoroutineScope()
    val timeline = state.timeline
    var scrub by remember { mutableStateOf<Double?>(null) }
    var selected by rememberSaveable { mutableStateOf<Long?>(null) }
    var tool by rememberSaveable { mutableStateOf<Tool?>(null) }
    var agentOpen by rememberSaveable { mutableStateOf(false) }
    // The playhead, read where it is drawn (the timeline's canvas, the time label) rather than
    // here: while playing it moves every display frame, and only those should redraw for it.
    val live = remember { mutableDoubleStateOf(0.0) }
    LaunchedEffect(playing) {
        if (playing) while (true) withFrameNanos { live.doubleValue = player.clock() }
    }
    val playheadNow: () -> Double = { if (playing) live.doubleValue else scrub ?: timeline?.playhead ?: 0.0 }
    // Follow the engine's selection (an agent may select clips) and drop clips that are gone.
    LaunchedEffect(timeline?.selection, timeline?.tracks) {
        val sel = timeline?.selection?.firstOrNull()
        if (sel != null) selected = sel
        if (selected != null && timeline?.clip(selected!!) == null) selected = null
    }
    val pickMedia = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> if (uris.isNotEmpty()) onImport(uris, true) }

    fun commitSeek(t: Double) = scope.launch {
        lab.exec("playhead.set", """{"seconds":$t}""")
        scrub = null
    }

    fun save() = scope.launch {
        val name = (timeline?.name ?: "Untitled").replace(Regex("[^A-Za-z0-9 _-]"), "_").trim().ifEmpty { "Untitled" }
        val r = lab.fn("doc.save")
        if (r.isFailure) {
            lab.clearMessage()
            lab.fn("doc.save", """{"path":"projects/$name.fcproj"}""")
        }
    }

    Surface(Modifier.fillMaxSize(), color = Ink) {
        BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding()) {
            val wide = maxWidth >= 840.dp
            Row(Modifier.fillMaxSize()) {
                Column(Modifier.weight(1f).fillMaxHeight()) {
                    // ── top bar ────────────────────────────────────────────────────────────
                    Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        SymButton(Sym.close, "Close project", { player.stop(); save(); onClose() })
                        Text(timeline?.name ?: "", maxLines = 1, fontSize = 14.sp, color = Color(0xFFCCCCD2), modifier = Modifier.weight(1f))
                        if (state.busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                        SymButton(Sym.agent, "Agent", { agentOpen = !agentOpen }, tint = if (agentOpen) Accent else null)
                        SymButton(Sym.settings, "Settings", onSettings)
                        Box(
                            Modifier.padding(end = 6.dp).clip(RoundedCornerShape(8.dp)).background(Accent).clickable { tool = Tool.Export }.padding(horizontal = 12.dp, vertical = 6.dp),
                        ) { Text("Export", color = Color.Black, fontWeight = FontWeight.SemiBold, fontSize = 13.sp) }
                    }
                    // ── preview ────────────────────────────────────────────────────────────
                    Box(
                        Modifier.fillMaxWidth().weight(1f).background(Color.Black).clickable { player.toggle() },
                        contentAlignment = Alignment.Center,
                    ) {
                        ProgramMonitor(app, playing, state.version)
                    }
                    // ── transport ──────────────────────────────────────────────────────────
                    Row(Modifier.fillMaxWidth().height(44.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        TimeLabel(playheadNow, timeline?.duration ?: 0.0)
                        Spacer(Modifier.weight(1f))
                        SymButton(Sym.skipPrev, "Go to start", { player.stop(); commitSeek(0.0) })
                        SymButton(if (playing) Sym.pause else Sym.play, if (playing) "Pause" else "Play", { player.toggle() })
                        SymButton(Sym.skipNext, "Go to end", { player.stop(); commitSeek(timeline?.duration ?: 0.0) })
                        Spacer(Modifier.weight(1f))
                        KeyframeButton(lab, selected?.let { timeline?.clip(it) }, timeline?.fps ?: 30.0, playheadNow)
                        SymButton(Sym.undo, "Undo", { scope.launch { lab.fn("doc.undo") } })
                        SymButton(Sym.redo, "Redo", { scope.launch { lab.fn("doc.redo") } })
                    }
                    // ── timeline ───────────────────────────────────────────────────────────
                    if (timeline != null) {
                        TimelineView(
                            lab = lab,
                            timeline = timeline,
                            playhead = playheadNow,
                            selected = selected,
                            modifier = Modifier.fillMaxWidth().height(if (tool != null) 168.dp else 232.dp),
                            onScrub = { t ->
                                if (playing) player.stop()
                                scrub = t
                                lab.requestFrame(t)
                            },
                            onScrubEnd = { t -> commitSeek(t) },
                            onSelect = { id ->
                                selected = id
                                tool = null
                                scope.launch { lab.exec("timeline.select", """{"clips":[${id ?: ""}]}""") }
                            },
                            onTrim = { id, edge, delta -> scope.launch { lab.fn("timeline.trim", """{"clip":$id,"edge":"$edge","delta":$delta}""") } },
                            onMove = { id, seconds, track ->
                                val t = when (track) { null -> ""; is String -> ""","track":"$track""""; else -> ""","track":$track""" }
                                scope.launch { lab.fn("timeline.move", """{"clip":$id,"seconds":$seconds$t}""") }
                            },
                            onAdd = { pickMedia.launch(arrayOf("video/*", "audio/*", "image/*")) },
                        )
                    } else {
                        Box(Modifier.fillMaxWidth().height(232.dp), contentAlignment = Alignment.Center) {
                            TextButton(onClick = { pickMedia.launch(arrayOf("video/*", "audio/*", "image/*")) }) { Text("Add media") }
                        }
                    }
                    state.message?.let { msg ->
                        Row(Modifier.fillMaxWidth().background(Color(0xFF3A1F1F)).padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(msg, color = Color(0xFFFFB4AB), fontSize = 12.sp, maxLines = 2, modifier = Modifier.weight(1f))
                            TextButton(onClick = lab::clearMessage) { Text("OK") }
                        }
                    }
                    // ── bottom bar / sheet ─────────────────────────────────────────────────
                    val clip = selected?.let { timeline?.clip(it) }
                    val current = tool
                    if (current != null) {
                        ToolSheet(app, current, clip, playheadNow(), onDone = { tool = null })
                    } else if (clip != null) {
                        ToolBar {
                            ToolButton(Sym.back, "Back") { selected = null; scope.launch { lab.exec("timeline.select", """{"clips":[]}""") } }
                            ToolButton(Sym.split, "Split") { scope.launch { lab.fn("timeline.split", """{"seconds":${playheadNow()},"clip":${clip.id}}""") } }
                            if (clip.kind != ClipKind.Graphic) ToolButton(Sym.volume, "Volume") { tool = Tool.Volume }
                            if (clip.kind != ClipKind.Graphic) ToolButton(Sym.speed, "Speed") { tool = Tool.Speed }
                            ToolButton(Sym.fade, "Fade") { tool = Tool.Fade }
                            if (clip.kind == ClipKind.Video || clip.kind == ClipKind.Still) ToolButton(Sym.transition, "Transition") { tool = Tool.Transition }
                            val picture = clip.kind == ClipKind.Video || clip.kind == ClipKind.Still
                            val ids = """{"clips":[${clip.id}]}"""
                            if (picture) {
                                ToolButton(Sym.filters, "Filters") { tool = Tool.Filters }
                                ToolButton(Sym.effects, "Effects") { tool = Tool.Effects }
                                ToolButton(Sym.adjust, "Adjust") { tool = Tool.Adjust }
                            }
                            if (clip.kind != ClipKind.Audio) ToolButton(Sym.transform, "Transform") { tool = Tool.Transform }
                            if (picture) {
                                ToolButton(Sym.crop, "Crop") { tool = Tool.Crop }
                                ToolButton(Sym.mask, "Mask") { tool = Tool.Mask }
                                ToolButton(Sym.opacity, "Opacity") { tool = Tool.Blend }
                                ToolButton(Sym.chromaKey, "Chroma key") { tool = Tool.ChromaKey }
                            }
                            if (clip.kind == ClipKind.Video) {
                                ToolButton(Sym.stabilize, "Stabilize") { scope.launch { lab.fn("compose.stabilize", ids).onSuccess { lab.showMessage(if ("true" in it.toString()) "Stabilizing: the first play analyses the clip" else "Stabilize off") } } }
                                ToolButton(Sym.reframe, "Reframe") { scope.launch { lab.fn("compose.autoReframe", ids).onSuccess { lab.showMessage(if ("true" in it.toString()) "Auto reframe on" else "Auto reframe off") } } }
                            }
                            if (clip.kind == ClipKind.Video) {
                                ToolButton(Sym.freeze, "Freeze") { scope.launch { lab.fn("clip.freeze", """{"clip":${clip.id},"seconds":${playheadNow()}}""") } }
                                ToolButton(Sym.reverse, "Reverse") { scope.launch { lab.fn("clip.reverse", ids) } }
                            }
                            if (picture) {
                                ToolButton(Sym.mirror, "Mirror") { scope.launch { lab.fn("clip.mirror", ids) } }
                                ToolButton(Sym.rotate, "Rotate") { scope.launch { lab.fn("clip.rotate", ids) } }
                            }
                            if (clip.kind == ClipKind.Video) ToolButton(Sym.autoCut, "Auto cut") { scope.launch { lab.fn("clip.autoCut", ids) } }
                            if (clip.kind != ClipKind.Graphic) ToolButton(Sym.voice, "Voice fx") { tool = Tool.Voice }
                            if (clip.kind == ClipKind.Video) ToolButton(Sym.detach, "Detach audio") {
                                scope.launch { lab.fn("compose.detachAudio", ids).onSuccess { lab.showMessage(if ("[]" in it.toString()) "This clip's sound is already separate" else "Sound detached: it is its own clip now") } }
                            }
                            ToolButton(Sym.duplicate, "Duplicate") { scope.launch { lab.fn("compose.duplicate", """{"clip":${clip.id}}""") } }
                            if (clip.kind == ClipKind.Audio) {
                                ToolButton(Sym.duck, "Auto duck") { scope.launch { lab.fn("audio.duck", """{"music":[${clip.id}]}""") } }
                                ToolButton(Sym.loudness, "Loudness") { scope.launch { lab.fn("audio.normalize", ids) } }
                            }
                            ToolButton(Sym.delete, "Delete") {
                                scope.launch { lab.fn("timeline.delete", """{"clips":[${clip.id}],"ripple":${clip.kind == ClipKind.Video}}""") }
                                selected = null
                            }
                            ToolButton(Sym.agent, "Ask agent") { agentOpen = true }
                        }
                    } else {
                        ToolBar {
                            ToolButton(Sym.addMedia, "Media") { pickMedia.launch(arrayOf("video/*", "image/*")) }
                            ToolButton(Sym.music, "Audio") { pickMedia.launch(arrayOf("audio/*")) }
                            ToolButton(Sym.text, "Text") { tool = Tool.Text }
                            ToolButton(Sym.templates, "Templates") { tool = Tool.Templates }
                            ToolButton(Sym.captions, "Captions") { tool = Tool.Captions }
                            // Picture tools on the clip under the playhead (it becomes the selection).
                            fun onClipHere(t: Tool) {
                                val here = timeline?.videoTracks?.firstOrNull()?.clips?.firstOrNull { playheadNow() >= it.start && playheadNow() < it.end }
                                if (here == null) lab.showMessage("Move the playhead over a video clip first") else {
                                    selected = here.id
                                    tool = t
                                    scope.launch { lab.exec("timeline.select", """{"clips":[${here.id}]}""") }
                                }
                            }
                            ToolButton(Sym.effects, "Effects") { onClipHere(Tool.Effects) }
                            ToolButton(Sym.filters, "Filters") { onClipHere(Tool.Filters) }
                            ToolButton(Sym.adjust, "Adjust") { onClipHere(Tool.Adjust) }
                            ToolButton(Sym.narrate, "Narration") { tool = Tool.Narrate }
                            ToolButton(Sym.canvas, "Canvas") { tool = Tool.Canvas }
                            ToolButton(Sym.closeGaps, "Close gaps") { scope.launch { lab.fn("project.closeGaps").onSuccess { if ("\"moved\":0" in it.toString()) lab.showMessage("No gaps on the main track") } } }
                            ToolButton(Sym.split, "Split all") { scope.launch { lab.fn("timeline.split", """{"seconds":${playheadNow()},"all":true}""") } }
                            ToolButton(Sym.agent, "Agent") { agentOpen = true }
                        }
                    }
                }
                if (wide && agentOpen) {
                    AgentPane(app, Modifier.width(400.dp).fillMaxHeight(), onClose = { agentOpen = false }, onOpenSettings = onSettings)
                }
            }
            if (!wide) {
                AnimatedVisibility(agentOpen, enter = slideInVertically { it }, exit = slideOutVertically { it }, modifier = Modifier.align(Alignment.BottomCenter)) {
                    AgentPane(
                        app,
                        Modifier.fillMaxWidth().fillMaxHeight(0.62f).clip(RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp)),
                        onClose = { agentOpen = false },
                        onOpenSettings = onSettings,
                    )
                }
            }
        }
    }
}

@Composable
private fun ToolBar(content: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(72.dp).background(Color(0xFF151518)).horizontalScroll(rememberScrollState()).padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) { content() }
}

@Composable
private fun ToolButton(icon: Int, label: String, onClick: () -> Unit) {
    Column(
        Modifier.width(68.dp).clip(RoundedCornerShape(8.dp)).clickable(onClick = onClick).padding(vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        SymIcon(icon, null, Modifier.size(24.dp), tint = Color.White)
        Text(label, fontSize = 11.sp, color = Color(0xFFDDDDE2), modifier = Modifier.padding(top = 4.dp), maxLines = 1)
    }
}

@Composable
fun SheetHeader(title: String, onDone: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        SymButton(Sym.check, "Done", onDone)
    }
}

/** The program monitor and its status line; the only part of the editor that redraws per frame. */
@Composable
private fun BoxScope.ProgramMonitor(app: VideoLabApp, playing: Boolean, version: Long) {
    val lab = app.lab
    val frame by lab.frame.collectAsState()
    val fps by app.player.fps.collectAsState()
    val f = frame
    if (f != null) {
        // Keyed on the serial: the player refills the same bitmaps.
        val image = remember(f.serial) { f.bitmap.asImageBitmap() }
        Image(image, contentDescription = "Program monitor", contentScale = ContentScale.Fit, modifier = Modifier.fillMaxSize())
    } else if (lab.state.value.timeline == null) {
        Text("No sequence open", color = Color.Gray)
    }
    var hw by remember { mutableStateOf("") }
    LaunchedEffect(playing, version) {
        while (true) {
            hw = lab.io { it.decodeStats() }.getOrNull()?.let { j ->
                val o = lab.parse(j) as? kotlinx.serialization.json.JsonObject
                val stats = (o?.get("stats") as? kotlinx.serialization.json.JsonPrimitive)?.content.orEmpty()
                Regex("frames: (\\d+).*fallbacks: (\\d+)").find(stats)?.let { m -> "HW ${m.groupValues[1]}f/${m.groupValues[2]}fb" }
            }.orEmpty()
            if (!playing) break
            kotlinx.coroutines.delay(1000)
        }
    }
    // Playback resolution, as phone editors put it on the monitor: tap to cycle.
    val divisor by app.player.divisor.collectAsState()
    Text(
        when (divisor) { 1 -> "Full"; 2 -> "1/2"; else -> "1/4" },
        color = Color.White, fontSize = 11.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.align(Alignment.TopEnd).padding(8.dp).clip(RoundedCornerShape(6.dp)).background(Color(0x99000000))
            .clickable {
                val next = when (divisor) { 1 -> 2; 2 -> 4; else -> 1 }
                app.player.divisor.value = next
                app.settings.playbackDivisor = next
            }
            .padding(horizontal = 8.dp, vertical = 4.dp),
    )
    Text(
        buildString {
            append(f?.renderer.orEmpty().uppercase())
            if (hw.isNotEmpty()) append(" · ").append(hw)
            if (playing) append(" · %.0f fps".format(fps))
        },
        color = Color(0x99FFFFFF), fontSize = 10.sp,
        modifier = Modifier.align(Alignment.BottomStart).padding(6.dp),
    )
}

/** "00:07 / 00:12", recomposed only when the shown second changes. */
@Composable
private fun TimeLabel(playhead: () -> Double, duration: Double) {
    val read by rememberUpdatedState(playhead)
    val text by remember(duration) { derivedStateOf { "${formatTime(read())} / ${formatTime(duration)}" } }
    Text(text, fontSize = 12.sp, color = Color(0xFFCCCCD2), modifier = Modifier.width(92.dp), maxLines = 1)
}

/**
 * CapCut's ◇: adds (or removes) a transform keyframe on the selected clip at the playhead; filled
 * when the playhead sits on one. Recomposes only when that flips, not every frame.
 */
@Composable
private fun KeyframeButton(lab: dev.argo.videolab.core.Lab, clip: dev.argo.videolab.core.Clip?, fps: Double, playhead: () -> Double) {
    val scope = rememberCoroutineScope()
    val read by rememberUpdatedState(playhead)
    val usable = clip != null && clip.kind != ClipKind.Audio
    val keys = clip?.keyframes.orEmpty()
    val onKey by remember(keys, fps) { derivedStateOf { keys.any { kotlin.math.abs(it - read()) < 0.5 / fps } } }
    SymButton(
        if (onKey) Sym.keyframeOn else Sym.keyframe,
        if (onKey) "Remove keyframe" else "Add keyframe",
        { if (clip != null && usable) scope.launch { lab.fn("clip.keyframe", """{"clip":${clip.id},"seconds":${read()}}""") } },
        tint = if (!usable) Color(0xFF55555C) else if (onKey) Accent else null,
    )
}
