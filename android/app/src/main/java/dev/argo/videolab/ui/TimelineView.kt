package dev.argo.videolab.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.core.Clip
import dev.argo.videolab.core.ClipKind
import dev.argo.videolab.core.Lab
import dev.argo.videolab.core.Timeline
import dev.argo.videolab.core.formatTime
import kotlinx.coroutines.android.awaitFrame
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.floor
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt

/** A clip edge being dragged: the drawn clip follows [delta] seconds until release. */
private data class TrimDrag(val clip: Long, val edge: String, val delta: Double)

/** One drawn row: a track and where it sits. */
private data class Row(val clips: List<Clip>, val top: Float, val height: Float, val main: Boolean)

/**
 * The phone timeline: the playhead stays in the middle and the tracks slide under it.
 *
 *  * one-finger drag scrubs (with fling); pinch zooms; tap selects a clip (or clears);
 *  * the selected clip shows handles at both ends; dragging one trims that edge;
 *  * rows from the top: titles and overlays (V2+), the main track (V1) as a filmstrip, then sound
 *    (linked audio in blue, music and other audio in green) with waveforms.
 */
@Composable
fun TimelineView(
    lab: Lab,
    timeline: Timeline,
    /** Read when drawing, so a moving playhead redraws the canvas without recomposing. */
    playhead: () -> Double,
    selected: Long?,
    modifier: Modifier,
    onScrub: (Double) -> Unit,
    onScrubEnd: (Double) -> Unit,
    onSelect: (Long?) -> Unit,
    onTrim: (Long, String, Double) -> Unit,
    onAdd: () -> Unit,
) {
    val density = LocalDensity.current
    val media by lab.mediaVersion.collectAsState()
    val measurer = rememberTextMeasurer()
    val scope = rememberCoroutineScope()
    val slop = LocalViewConfiguration.current.touchSlop
    // Pixels per second; starts so a few seconds fill the screen.
    var pps by remember { mutableFloatStateOf(with(density) { 64.dp.toPx() }) }
    var trim by remember { mutableStateOf<TrimDrag?>(null) }
    val current by rememberUpdatedState(playhead)
    val tl by rememberUpdatedState(timeline)
    val sel by rememberUpdatedState(selected)

    val rulerH = with(density) { 26.dp.toPx() }
    val mainH = with(density) { 56.dp.toPx() }
    val rowH = with(density) { 34.dp.toPx() }
    val gap = with(density) { 6.dp.toPx() }
    val handleW = with(density) { 14.dp.toPx() }

    fun rows(t: Timeline): List<Row> {
        val out = mutableListOf<Row>()
        var y = rulerH + gap
        // Overlays and titles above the main track, top track first.
        t.videoTracks.drop(1).reversed().filter { it.clips.isNotEmpty() }.forEach { out += Row(it.clips, y, rowH, false); y += rowH + gap }
        val main = t.videoTracks.firstOrNull()
        out += Row(main?.clips.orEmpty(), y, mainH, true); y += mainH + gap
        t.audioTracks.filter { it.clips.isNotEmpty() }.forEach { out += Row(it.clips, y, rowH, false); y += rowH + gap }
        return out
    }

    BoxWithConstraints(modifier.background(Color(0xFF111114))) {
        val widthPx = with(density) { maxWidth.toPx() }
        val drawn = rows(timeline)
        val mainRow = drawn.firstOrNull { it.main }
        Canvas(
            Modifier.fillMaxSize().pointerInput(Unit) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    val cx = size.width / 2f
                    val t0 = current()
                    val rowsNow = rows(tl)
                    // A handle of the selected clip?
                    val handle = sel?.let { id ->
                        val row = rowsNow.firstOrNull { r -> r.clips.any { it.id == id } }
                        val c = tl.clip(id)
                        if (row == null || c == null || down.position.y !in row.top..(row.top + row.height)) null
                        else {
                            val x0 = cx + ((c.start - t0) * pps).toFloat()
                            val x1 = cx + ((c.end - t0) * pps).toFloat()
                            when {
                                abs(down.position.x - (x0 - handleW / 2)) < handleW * 1.4f -> "in"
                                abs(down.position.x - (x1 + handleW / 2)) < handleW * 1.4f -> "out"
                                else -> null
                            }
                        }
                    }
                    val tracker = VelocityTracker()
                    tracker.addPosition(down.uptimeMillis, down.position)
                    var moved = false
                    var dxTotal = 0f
                    var zooming = false
                    var lastSpan = 0f
                    var scrub = t0
                    while (true) {
                        val ev = awaitPointerEvent()
                        val pressed = ev.changes.filter { it.pressed }
                        if (pressed.isEmpty()) break
                        if (pressed.size >= 2 && handle == null) {
                            val span = (pressed[0].position - pressed[1].position).getDistance()
                            if (zooming && lastSpan > 0f) pps = (pps * span / lastSpan).coerceIn(4f, 2400f)
                            zooming = true
                            lastSpan = span
                            ev.changes.forEach { it.consume() }
                            continue
                        }
                        val ch = pressed.first()
                        val dx = ch.positionChange().x
                        dxTotal += dx
                        if (!moved && abs(dxTotal) > slop) moved = true
                        if (moved) {
                            tracker.addPosition(ch.uptimeMillis, ch.position)
                            if (handle != null && sel != null) {
                                trim = TrimDrag(sel!!, handle, dxTotal / pps.toDouble())
                            } else if (!zooming) {
                                scrub = (scrub - dx / pps).coerceIn(0.0, max(0.0, tl.duration))
                                onScrub(scrub)
                            }
                            ch.consume()
                        }
                    }
                    when {
                        handle != null && moved -> {
                            val d = trim
                            trim = null
                            if (d != null && abs(d.delta) > 0.01) onTrim(d.clip, d.edge, d.delta)
                        }
                        zooming -> {}
                        moved -> {
                            // Fling: keep scrubbing with decaying velocity, then settle.
                            val v = -tracker.calculateVelocity().x / pps
                            scope.launch {
                                var vel = v.toDouble()
                                var pos = scrub
                                var last = System.nanoTime()
                                while (abs(vel) > 0.05) {
                                    awaitFrame()
                                    val now = System.nanoTime()
                                    val dt = (now - last) / 1e9
                                    last = now
                                    pos = (pos + vel * dt).coerceIn(0.0, max(0.0, tl.duration))
                                    vel *= 0.92.pow(dt * 60)
                                    onScrub(pos)
                                    if (pos <= 0.0 || pos >= tl.duration) break
                                }
                                onScrubEnd(pos)
                            }
                        }
                        else -> {
                            // Tap: select the clip under the finger.
                            val tAt = t0 + (down.position.x - cx) / pps
                            val row = rowsNow.firstOrNull { down.position.y in it.top..(it.top + it.height) }
                            onSelect(row?.clips?.firstOrNull { tAt in it.start..it.end }?.id)
                        }
                    }
                }
            },
        ) {
            media.let { } // redraw when thumbnails/waveforms land
            val now = playhead()
            val cx = size.width / 2f
            drawRuler(timeline, now, pps, cx, rulerH, measurer)
            for (row in drawn) {
                for (c0 in row.clips) {
                    val c = trim?.takeIf { it.clip == c0.id }?.let { d ->
                        if (d.edge == "in") c0.copy(start = c0.start + d.delta, sourceIn = c0.sourceIn + d.delta * c0.speed)
                        else c0.copy(end = c0.end + d.delta, sourceOut = c0.sourceOut + d.delta * c0.speed)
                    } ?: c0
                    val x0 = cx + ((c.start - now) * pps).toFloat()
                    val x1 = cx + ((c.end - now) * pps).toFloat()
                    if (x1 < -handleW || x0 > size.width + handleW || x1 - x0 < 0.5f) continue
                    drawClip(lab, timeline, c, Rect(x0, row.top, x1, row.top + row.height), pps, row.main, measurer)
                    if (c.id == selected) {
                        drawSelection(Rect(x0, row.top, x1, row.top + row.height), handleW)
                        // Keyframe diamonds along the bottom of the selected clip.
                        for (k in c.keyframes) {
                            val kx = cx + ((k - now) * pps).toFloat()
                            if (kx in x0..x1) drawKeyframe(Offset(kx, row.top + row.height - 8.dp.toPx()), 5.dp.toPx())
                        }
                    }
                }
                if (row.main) {
                    // Transitions on the main track: a small white diamond at each one.
                    timeline.videoTracks.firstOrNull()?.transitions?.forEach { tr ->
                        val x = cx + ((tr.start + tr.duration / 2 - now) * pps).toFloat()
                        if (x in 0f..size.width) drawTransitionMark(Offset(x, row.top + row.height / 2))
                    }
                }
            }
            // The playhead.
            drawLine(Playhead, Offset(cx, rulerH * 0.55f), Offset(cx, size.height), strokeWidth = 2.dp.toPx())
        }
        // Timecode badge riding the playhead.
        val badge by remember { derivedStateOf { formatTime(current(), tenths = true) } }
        Box(
            Modifier.align(Alignment.TopCenter).padding(top = 2.dp).clip(RoundedCornerShape(4.dp)).background(Accent).padding(horizontal = 6.dp, vertical = 1.dp),
        ) { androidx.compose.material3.Text(badge, color = Color.Black, fontSize = 11.sp, fontWeight = FontWeight.SemiBold) }
        // "+" at the end of the main track (fixed at the right edge, as phone editors do).
        if (mainRow != null) {
            val y = with(density) { (mainRow.top + mainRow.height / 2).toDp() }
            Box(
                Modifier.align(Alignment.TopEnd).offset(y = y - 18.dp).padding(end = 8.dp).size(36.dp).clip(RoundedCornerShape(8.dp)).background(Color.White),
                contentAlignment = Alignment.Center,
            ) {
                androidx.compose.material3.IconButton(onClick = onAdd) { SymIcon(Sym.add, "Add media", tint = Color.Black) }
            }
        }
        if (widthPx <= 0f) return@BoxWithConstraints
    }
}

private fun DrawScope.drawRuler(t: Timeline, playhead: Double, pps: Float, cx: Float, h: Float, measurer: androidx.compose.ui.text.TextMeasurer) {
    val fps = t.fps.coerceIn(1.0, 240.0)
    val minLabelPx = 56.dp.toPx()
    // Frame labels when zoomed in far enough, else seconds.
    val frameSteps = listOf(1, 2, 5, 10, 15)
    val secSteps = listOf(1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0)
    val frameStep = frameSteps.firstOrNull { it / fps * pps >= minLabelPx && it < fps }
    val step = frameStep?.let { it / fps } ?: secSteps.firstOrNull { it * pps >= minLabelPx } ?: 600.0
    val visStart = playhead - cx / pps
    val visEnd = playhead + (size.width - cx) / pps
    var k = floor(visStart / step).toLong()
    val style = TextStyle(color = Color(0xFF9A9AA2), fontSize = 10.sp)
    while (k * step <= visEnd) {
        val s = k * step
        if (s >= 0) {
            val x = cx + ((s - playhead) * pps).toFloat()
            val frame = (s * fps).roundToInt()
            val whole = frame % fps.roundToInt() == 0
            val label = if (frameStep != null && !whole) "${frame % fps.roundToInt()}f" else formatTime(s)
            val m = measurer.measure(label, style)
            drawText(m, topLeft = Offset(x - m.size.width / 2f, h - m.size.height - 4.dp.toPx()))
            // Dot halfway to the next label.
            val xm = cx + ((s + step / 2 - playhead) * pps).toFloat()
            drawCircle(Color(0xFF55555C), 1.5.dp.toPx(), Offset(xm, h - m.size.height / 2f - 4.dp.toPx()))
        }
        k++
    }
    // Markers.
    for (mk in t.markers) {
        val x = cx + ((mk.at - playhead) * pps).toFloat()
        if (x !in -10f..size.width + 10f) continue
        val p = Path().apply { moveTo(x - 4.dp.toPx(), h - 2.dp.toPx()); lineTo(x + 4.dp.toPx(), h - 2.dp.toPx()); lineTo(x, h + 4.dp.toPx()); close() }
        drawPath(p, markerColor(mk.color))
    }
}

private fun markerColor(name: String): Color = when (name.lowercase()) {
    "green" -> Color(0xFF55C46B)
    "red" -> Color(0xFFE5534B)
    "rose" -> Color(0xFFE57BA0)
    "violet" -> Color(0xFF9B7BE5)
    "orange" -> Color(0xFFE59A4B)
    "yellow" -> Color(0xFFE5D24B)
    "blue" -> Color(0xFF4B8BE5)
    "cyan" -> Color(0xFF4BD0E5)
    else -> Color(0xFF55C46B)
}

private fun DrawScope.drawClip(lab: Lab, t: Timeline, c: Clip, r: Rect, pps: Float, main: Boolean, measurer: androidx.compose.ui.text.TextMeasurer) {
    val radius = CornerRadius(5.dp.toPx())
    val visible = Rect(max(r.left, -4f), r.top, min(r.right, size.width + 4f), r.bottom)
    when (c.kind) {
        ClipKind.Video, ClipKind.Still -> {
            drawRoundRect(ClipVideo, visible.topLeft, visible.size, radius)
            val item = c.item
            if (item != null) clipRect(visible.left, visible.top, visible.right, visible.bottom) { drawFilmstrip(lab, t, c, r, pps, item) }
            if (!main) drawLabel(c.name, r, visible, measurer, Color.White)
        }
        ClipKind.Graphic -> {
            drawRoundRect(ClipText, visible.topLeft, visible.size, radius)
            drawLabel(c.name.ifBlank { "Text" }, r, visible, measurer, Color.White)
        }
        ClipKind.Audio, ClipKind.Music -> {
            val color = if (c.kind == ClipKind.Audio) ClipAudio else ClipMusic
            drawRoundRect(color.copy(alpha = 0.35f), visible.topLeft, visible.size, radius)
            val item = c.item
            if (item != null) clipRect(visible.left, visible.top, visible.right, visible.bottom) { drawWaveform(lab, t, c, r, pps, item, color) }
            if (c.kind == ClipKind.Music || r.width > 120.dp.toPx()) drawLabel(c.name, r, visible, measurer, Color.White)
        }
    }
    // A thin dark seam between neighbours.
    drawRect(Color(0xFF111114), Offset(r.left, r.top), Size(1.dp.toPx(), r.height))
}

private fun DrawScope.drawFilmstrip(lab: Lab, t: Timeline, c: Clip, r: Rect, pps: Float, item: Long) {
    val h = r.height
    val tw = max(h * t.aspect.coerceIn(0.4f, 2.4f), 24.dp.toPx())
    val thumbH = (h * 1.5f).roundToInt().coerceIn(48, 200)
    // Media time per tile, rounded to a power of two so zooming reuses cached frames.
    val per = (tw / pps * c.speed).coerceAtLeast(1.0 / 30)
    val q = 2.0.pow(floor(ln(per) / ln(2.0)))
    val first = floor((max(r.left, 0f) - r.left) / tw).toInt()
    var k = first
    while (true) {
        val x = r.left + k * tw
        if (x > min(r.right, size.width)) break
        val mt = c.sourceIn + ((k + 0.5) * tw / pps) * c.speed
        val key = (floor(mt / q) * q).coerceAtLeast(0.0)
        val bmp = lab.thumbnail(item, key, thumbH)
        if (bmp != null) {
            val img = bmp.asImageBitmap()
            // Cover the tile, cropping the frame's sides.
            val scale = max(tw / img.width, h / img.height)
            val sw = (tw / scale).roundToInt().coerceIn(1, img.width)
            val sh = (h / scale).roundToInt().coerceIn(1, img.height)
            drawImage(
                img,
                srcOffset = IntOffset((img.width - sw) / 2, (img.height - sh) / 2),
                srcSize = IntSize(sw, sh),
                dstOffset = IntOffset(x.roundToInt(), r.top.roundToInt()),
                dstSize = IntSize(tw.roundToInt() + 1, h.roundToInt()),
            )
        }
        k++
    }
}

private fun DrawScope.drawWaveform(lab: Lab, t: Timeline, c: Clip, r: Rect, pps: Float, item: Long, color: Color) {
    val itemDur = lab.state.value.items.firstOrNull { it.id == item }?.duration ?: return
    val peaks = lab.peaks(item, itemDur) ?: return
    val per = lab.peaksPerSecond(item, itemDur)
    val n = peaks.size / 2
    val mid = r.top + r.height * 0.55f
    val amp = r.height * 0.42f
    val gain = 10.0.pow(c.gainDb / 20).toFloat().coerceIn(0f, 4f)
    val step = 2.dp.toPx()
    var x = max(r.left, 0f)
    val end = min(r.right, size.width)
    while (x < end) {
        val mt = c.sourceIn + ((x - r.left) / pps) * c.speed
        val b = (mt * per).toInt()
        if (b in 0 until n) {
            val a = (max(abs(peaks[2 * b]), abs(peaks[2 * b + 1])) * gain).coerceAtMost(1f)
            val hgt = max(1f, a * amp)
            drawLine(color, Offset(x, mid - hgt), Offset(x, mid + hgt * 0.6f), strokeWidth = step * 0.7f)
        }
        x += step
    }
}

private fun DrawScope.drawLabel(text: String, r: Rect, visible: Rect, measurer: androidx.compose.ui.text.TextMeasurer, color: Color) {
    if (visible.width < 28.dp.toPx() || text.isBlank()) return
    val style = TextStyle(color = color, fontSize = 10.sp, fontWeight = FontWeight.Medium)
    val m = measurer.measure(text, style, maxLines = 1, softWrap = false, constraints = androidx.compose.ui.unit.Constraints(maxWidth = (visible.width - 8.dp.toPx()).roundToInt().coerceAtLeast(1)))
    val pad = 4.dp.toPx()
    drawRect(Color(0x66000000), Offset(max(r.left, 0f) + pad - 2, r.bottom - m.size.height - pad), Size(m.size.width + 4f, m.size.height.toFloat()))
    drawText(m, topLeft = Offset(max(r.left, 0f) + pad, r.bottom - m.size.height - pad))
}

private fun DrawScope.drawSelection(r: Rect, handleW: Float) {
    val radius = CornerRadius(5.dp.toPx())
    drawRoundRect(Selected, Offset(r.left, r.top - 1), Size(r.width, r.height + 2), radius, style = Stroke(2.dp.toPx()))
    for (left in listOf(true, false)) {
        val x = if (left) r.left - handleW else r.right
        drawRoundRect(Selected, Offset(x, r.top - 1), Size(handleW, r.height + 2), CornerRadius(4.dp.toPx()))
        drawLine(Color(0xFF444448), Offset(x + handleW / 2, r.top + r.height * 0.3f), Offset(x + handleW / 2, r.bottom - r.height * 0.3f), strokeWidth = 2.dp.toPx())
    }
}

private fun DrawScope.drawTransitionMark(c: Offset) {
    val s = 7.dp.toPx()
    val p = Path().apply { moveTo(c.x, c.y - s); lineTo(c.x + s, c.y); lineTo(c.x, c.y + s); lineTo(c.x - s, c.y); close() }
    drawPath(p, Color.White)
    drawPath(p, Color(0xFF111114), style = Stroke(1.5.dp.toPx()))
}

/** A keyframe marker: a white diamond with a dark outline. */
private fun DrawScope.drawKeyframe(at: Offset, r: Float) {
    val path = androidx.compose.ui.graphics.Path().apply {
        moveTo(at.x, at.y - r); lineTo(at.x + r, at.y); lineTo(at.x, at.y + r); lineTo(at.x - r, at.y); close()
    }
    drawPath(path, Color.White)
    drawPath(path, Color(0xFF111114), style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.dp.toPx()))
}
