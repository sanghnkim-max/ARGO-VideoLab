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
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
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
import dev.argo.videolab.core.Track
import dev.argo.videolab.core.Transition
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
private data class Row(val clips: List<Clip>, val top: Float, val height: Float, val main: Boolean, val track: Track?, val video: Boolean)

/** Where a dragged clip would land: a track (by id), a new track, or its own track (null). */
private sealed interface Drop {
    data class OnTrack(val id: Long) : Drop
    data class NewTrack(val reuse: Long?) : Drop
}

/** A clip being moved (long press, then drag): it follows the finger until release. */
/** A swipe down across a clip: where it would split, and whether letting go splits it. */
private data class CutSwipe(val clip: Long, val at: Double, val armed: Boolean)

/** The quick menu: for a clip (or empty space: paste only), at a time and a place on screen. */
private data class QuickMenu(val clip: Long?, val at: Double, val position: Offset)

private data class ClipDrag(val clip: Clip, val video: Boolean, val start: Double, val y: Float, val drop: Drop?)

/**
 * The phone timeline: the playhead stays in the middle and the tracks slide under it.
 *
 *  * one-finger drag scrubs (with fling); pinch zooms; tap selects a clip (or clears);
 *  * the selected clip shows handles at both ends; dragging one trims that edge;
 *  * long-press a clip and drag to move it in time or onto another track of its kind; drop it
 *    above the top video row (sound: below the bottom audio row) for a new track;
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
    /** Move a clip to start at the given seconds; the track is an id, "new", or null (its own). */
    onMove: (Long, Double, Any?) -> Unit,
    onAdd: () -> Unit,
    /** Split a clip at the given seconds (swipe down across it). */
    onSplitAt: (Long, Double) -> Unit = { _, _ -> },
    /** A quick-menu action: split, cut, copy, paste, duplicate, delete; the clip (null: empty space) and seconds. */
    onQuick: (String, Long?, Double) -> Unit = { _, _, _ -> },
    /** Whether the clipboard has clips (the menu offers Paste). */
    canPaste: Boolean = false,
    /** Two-finger tap. */
    onUndo: () -> Unit = {},
    /** A transition was tapped. */
    onTransition: (Long) -> Unit = {},
    /** The transition being edited (outlined). */
    selectedTransition: Long? = null,
) {
    val density = LocalDensity.current
    val media by lab.mediaVersion.collectAsState()
    val measurer = rememberTextMeasurer()
    val scope = rememberCoroutineScope()
    val slop = LocalViewConfiguration.current.touchSlop
    // Pixels per second; starts so a few seconds fill the screen.
    var pps by remember { mutableFloatStateOf(with(density) { 64.dp.toPx() }) }
    var trim by remember { mutableStateOf<TrimDrag?>(null) }
    var moving by remember { mutableStateOf<ClipDrag?>(null) }
    var cut by remember { mutableStateOf<CutSwipe?>(null) }
    var menu by remember { mutableStateOf<QuickMenu?>(null) }
    val paste by rememberUpdatedState(canPaste)
    val haptics = LocalHapticFeedback.current
    val current by rememberUpdatedState(playhead)
    val tl by rememberUpdatedState(timeline)
    val sel by rememberUpdatedState(selected)

    val rulerH = with(density) { 26.dp.toPx() }
    val mainH = with(density) { 56.dp.toPx() }
    val rowH = with(density) { 34.dp.toPx() }
    val gap = with(density) { 6.dp.toPx() }
    val handleW = with(density) { 14.dp.toPx() }

    // A transition's box on its row: its span, at least a thumb wide so it can be tapped zoomed out.
    val minTransitionW = with(density) { 24.dp.toPx() }
    fun transitionRect(tr: Transition, row: Row, now: Double, cx: Float): Rect {
        val x0 = cx + ((tr.start - now) * pps).toFloat()
        val x1 = cx + ((tr.start + tr.duration - now) * pps).toFloat()
        val mid = (x0 + x1) / 2
        val half = max((x1 - x0) / 2, minTransitionW / 2)
        val inset = row.height * 0.12f
        return Rect(mid - half, row.top + inset, mid + half, row.top + row.height - inset)
    }
    /** The transition under a tap, if any. */
    fun transitionAt(p: Offset, rowsNow: List<Row>, now: Double, cx: Float): Long? =
        rowsNow.filter { it.video && p.y in it.top..(it.top + it.height) }.firstNotNullOfOrNull { row ->
            row.track?.transitions?.firstOrNull { transitionRect(it, row, now, cx).contains(p) }?.id
        }

    fun rows(t: Timeline): List<Row> {
        val out = mutableListOf<Row>()
        var y = rulerH + gap
        // Overlays and titles above the main track, top track first.
        t.videoTracks.drop(1).reversed().filter { it.clips.isNotEmpty() }.forEach { out += Row(it.clips, y, rowH, false, it, true); y += rowH + gap }
        val main = t.videoTracks.firstOrNull()
        out += Row(main?.clips.orEmpty(), y, mainH, true, main, true); y += mainH + gap
        t.audioTracks.filter { it.clips.isNotEmpty() }.forEach { out += Row(it.clips, y, rowH, false, it, false); y += rowH + gap }
        return out
    }

    /**
     * Where a clip of kind [video] dropped at height [y] lands: above the top video row (sound:
     * below the bottom audio row) a new track — an empty track past the last used one of that
     * kind when there is one, else a new one; on a row of its kind, that track; else its own.
     */
    fun dropAt(t: Timeline, rowsNow: List<Row>, video: Boolean, y: Float): Drop? {
        val mine = rowsNow.filter { it.video == video }
        val tracks = if (video) t.videoTracks else t.audioTracks
        val lastUsed = tracks.indexOfLast { it.clips.isNotEmpty() }
        val spare = tracks.drop(lastUsed + 1).firstOrNull { it.clips.isEmpty() }?.id
        if (mine.isEmpty()) return Drop.NewTrack(spare)
        if (video && y < mine.first().top) return Drop.NewTrack(spare)
        if (!video && y > mine.last().top + mine.last().height) return Drop.NewTrack(spare)
        return mine.firstOrNull { y in (it.top - gap / 2)..(it.top + it.height + gap / 2) }?.track?.let { Drop.OnTrack(it.id) }
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
                    val hitRow = rowsNow.firstOrNull { down.position.y in it.top..(it.top + it.height) }
                    val tAtDown = t0 + (down.position.x - cx) / pps
                    val hit = hitRow?.clips?.firstOrNull { tAtDown in it.start..it.end }?.takeIf { hitRow.track != null }
                    if (handle == null) {
                        // Long press: on a clip, drag it to move it (or let go in place for the quick
                        // menu); on empty space, the quick menu (paste).
                        // A long press only while the finger stays put: sliding past the touch slop
                        // scrolls (or swipes) at once, a second finger zooms, lifting taps.
                        var lastPos = down.position
                        val held = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
                            var stayed = true
                            while (stayed) {
                                val ev = awaitPointerEvent()
                                val ch = ev.changes.firstOrNull { it.id == down.id }
                                if (ev.changes.count { it.pressed } >= 2 || ch == null || !ch.pressed) stayed = false
                                else {
                                    lastPos = ch.position
                                    if ((ch.position - down.position).getDistance() > slop) stayed = false
                                }
                            }
                            false
                        } == null
                        val long = if (held) lastPos else null
                        if (long != null) {
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            if (hit == null || hitRow == null) {
                                while (currentEvent.changes.any { it.pressed }) awaitPointerEvent().changes.forEach { it.consume() }
                                if (paste) menu = QuickMenu(null, max(0.0, tAtDown), long)
                                return@awaitEachGesture
                            }
                            onSelect(hit.id)
                            var start = hit.start
                            var y = long.y
                            var travel = 0f
                            moving = ClipDrag(hit, hitRow.video, start, y, null)
                            // Edges it snaps to: the playhead, 0, and other clips' ends.
                            val edges = listOf(t0, 0.0) + tl.tracks.flatMap { tr -> tr.clips.filter { it.id != hit.id }.flatMap { listOf(it.start, it.end) } }
                            val snapPx = 10.dp.toPx()
                            var raw = start
                            while (true) {
                                val ev = awaitPointerEvent()
                                val ch = ev.changes.firstOrNull { it.id == down.id } ?: ev.changes.first()
                                if (!ch.pressed) break
                                travel += ch.positionChange().getDistance()
                                raw = (raw + ch.positionChange().x / pps).coerceAtLeast(0.0)
                                y = ch.position.y
                                // Snap either edge of the clip to the nearest edge within reach.
                                val len = hit.end - hit.start
                                val best = edges.flatMap { e -> listOf(e to e, e to e - len) }.minByOrNull { (_, s) -> abs(s - raw) }
                                start = if (best != null && abs(best.second - raw) * pps < snapPx) best.second.coerceAtLeast(0.0) else raw
                                moving = ClipDrag(hit, hitRow.video, start, y, dropAt(tl, rowsNow, hitRow.video, y))
                                ch.consume()
                            }
                            val m = moving
                            moving = null
                            if (travel < slop) {
                                // Held and let go in place: the quick menu.
                                menu = QuickMenu(hit.id, tAtDown, long)
                                return@awaitEachGesture
                            }
                            if (m != null) {
                                val track: Any? = when (val d = m.drop) {
                                    is Drop.NewTrack -> d.reuse ?: "new"
                                    is Drop.OnTrack -> d.id.takeIf { it != hitRow.track!!.id }
                                    null -> null
                                }
                                if (track != null || abs(m.start - hit.start) > 1e-3) onMove(hit.id, m.start, track)
                            }
                            return@awaitEachGesture
                        }
                        // Released before the long press: a tap selects the clip; tapping the
                        // selected clip again opens its quick menu.
                        if (currentEvent.changes.none { it.pressed }) {
                            transitionAt(down.position, rowsNow, t0, cx)?.let { onTransition(it); return@awaitEachGesture }
                            if (hit != null && hit.id == sel) menu = QuickMenu(hit.id, tAtDown, down.position) else onSelect(hit?.id)
                            return@awaitEachGesture
                        }
                    }
                    val tracker = VelocityTracker()
                    tracker.addPosition(down.uptimeMillis, down.position)
                    var moved = false
                    var dxTotal = 0f
                    var zooming = false
                    var lastSpan = 0f
                    var firstSpan = 0f
                    var spanChange = 0f
                    var fingers = 1
                    var scrub = t0
                    var cutting = false
                    // The magnet (CapCut's): while scrubbing slowly the playhead sticks to an edge —
                    // a cut, a clip's start or end on any track, a marker, the start or end — within
                    // a few pixels, with a tick, and lets go once the finger moves past. A fast
                    // scrub or a fling passes over edges.
                    val magnetPx = 12.dp.toPx()
                    val slowPx = 900f // finger speed (px/s) under which the magnet holds
                    val edges = (listOf(0.0, tl.duration) + tl.tracks.flatMap { tr -> tr.clips.flatMap { listOf(it.start, it.end) } } + tl.markers.map { it.at }).distinct()
                    var raw = t0
                    var stuck: Double? = null
                    var speed = 0f
                    var lastAt = down.uptimeMillis
                    // Trimming snaps the edge being dragged to the playhead and other clips' edges.
                    val trimClip = if (handle != null) sel?.let { tl.clip(it) } else null
                    val trimEdges = trimClip?.let { c -> listOf(t0) + tl.tracks.flatMap { tr -> tr.clips.filter { it.id != c.id }.flatMap { listOf(it.start, it.end) } } }.orEmpty()
                    var trimStuck: Double? = null
                    // Where a swipe down splits: under the finger, snapped to the playhead nearby,
                    // kept a frame inside the clip.
                    val cutAt = hit?.let { c ->
                        val frame = 1.0 / tl.fps.coerceIn(1.0, 240.0)
                        val near = abs(down.position.x - cx) < 16.dp.toPx()
                        (if (near) t0 else tAtDown).coerceIn(c.start + frame, max(c.start + frame, c.end - frame))
                    }
                    val cutDistance = 44.dp.toPx()
                    while (true) {
                        val ev = awaitPointerEvent()
                        val pressed = ev.changes.filter { it.pressed }
                        if (pressed.isEmpty()) break
                        fingers = max(fingers, pressed.size)
                        if (pressed.size >= 2 && handle == null && !cutting) {
                            val span = (pressed[0].position - pressed[1].position).getDistance()
                            if (firstSpan == 0f) firstSpan = span
                            spanChange = max(spanChange, abs(span - firstSpan))
                            if (zooming && lastSpan > 0f) pps = (pps * span / lastSpan).coerceIn(4f, 2400f)
                            zooming = true
                            lastSpan = span
                            ev.changes.forEach { it.consume() }
                            continue
                        }
                        val ch = pressed.first()
                        val total = ch.position - down.position
                        // A swipe down that starts on a clip: the cut line follows, armed past a
                        // thumb's length.
                        if (!moved && !cutting && !zooming && handle == null && hit != null && cutAt != null &&
                            total.y > slop && total.y > abs(total.x) * 1.5f
                        ) cutting = true
                        if (cutting && hit != null && cutAt != null) {
                            val armed = total.y > cutDistance
                            if (armed && cut?.armed != true) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            cut = CutSwipe(hit.id, cutAt, armed)
                            ch.consume()
                            continue
                        }
                        val dx = ch.positionChange().x
                        // From where the finger went down: the wait for a long press saw the first
                        // few pixels, and the scroll should not lose them.
                        dxTotal = total.x
                        val justMoved = !moved && abs(dxTotal) > slop
                        if (justMoved) moved = true
                        if (moved) {
                            tracker.addPosition(ch.uptimeMillis, ch.position)
                            val dt = (ch.uptimeMillis - lastAt).coerceAtLeast(1L)
                            lastAt = ch.uptimeMillis
                            speed = speed * 0.6f + abs(dx) / dt * 1000f * 0.4f
                            if (handle != null && sel != null) {
                                var delta = dxTotal / pps.toDouble()
                                if (trimClip != null) {
                                    val base = if (handle == "in") trimClip.start else trimClip.end
                                    val near = trimEdges.minByOrNull { abs(it - (base + delta)) }?.takeIf { abs(it - (base + delta)) * pps < magnetPx }
                                    if (near != null && near != trimStuck) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                    trimStuck = near
                                    if (near != null) delta = near - base
                                }
                                trim = TrimDrag(sel!!, handle, delta)
                            } else if (!zooming) {
                                raw = (if (justMoved) t0 - dxTotal / pps else raw - dx / pps).coerceIn(0.0, max(0.0, tl.duration))
                                val near = if (speed < slowPx) edges.minByOrNull { abs(it - raw) }?.takeIf { abs(it - raw) * pps < magnetPx } else null
                                if (near != null && near != stuck) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                stuck = near
                                scrub = near ?: raw
                                onScrub(scrub)
                            }
                            ch.consume()
                        }
                    }
                    when {
                        cutting -> {
                            val c = cut
                            cut = null
                            if (c != null && c.armed) {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                onSplitAt(c.clip, c.at)
                            }
                        }
                        handle != null && moved -> {
                            val d = trim
                            trim = null
                            if (d != null && abs(d.delta) > 0.01) onTrim(d.clip, d.edge, d.delta)
                        }
                        // Two fingers down and up without pinching or sliding: undo.
                        zooming && fingers >= 2 && spanChange < slop && !moved && (currentEvent.changes.firstOrNull()?.uptimeMillis ?: down.uptimeMillis) - down.uptimeMillis < 300 -> {
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            onUndo()
                        }
                        zooming -> {}
                        moved -> {
                            // Fling: keep scrubbing with decaying velocity, then settle. Let go while
                            // the magnet holds an edge: it stays there.
                            val v = if (stuck != null) 0f else -tracker.calculateVelocity().x / pps
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
                            // Tap: a transition, or select the clip under the finger (the selected one: its menu).
                            val tr = if (handle == null) transitionAt(down.position, rowsNow, t0, cx) else null
                            if (tr != null) onTransition(tr)
                            else if (hit != null && hit.id == sel && handle == null) menu = QuickMenu(hit.id, tAtDown, down.position)
                            else onSelect(hit?.id)
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
                // Transitions on this track: a block over the cut as long as the transition, with
                // its type and length when there is room (a tappable badge when zoomed out).
                if (row.video) row.track?.transitions?.forEach { tr ->
                    val r = transitionRect(tr, row, now, cx)
                    if (r.right < 0f || r.left > size.width) return@forEach
                    drawTransition(tr, r, tr.id == selectedTransition, measurer)
                }
            }
            // A clip being moved: the target row (or the new-track band) and the clip under the finger.
            moving?.let { m ->
                val accent = Accent
                val h = if (m.video && tl.videoTracks.firstOrNull()?.clips?.any { it.id == m.clip.id } == true) mainH else rowH
                val x0 = cx + ((m.start - now) * pps).toFloat()
                val x1 = x0 + ((m.clip.end - m.clip.start) * pps).toFloat()
                when (val d = m.drop) {
                    is Drop.NewTrack -> {
                        val mine = drawn.filter { it.video == m.video }
                        val bandY = if (m.video) (mine.firstOrNull()?.top ?: (rulerH + gap)) - gap / 2 else (mine.lastOrNull()?.let { it.top + it.height } ?: rulerH) + gap / 2
                        drawLine(accent, Offset(0f, bandY), Offset(size.width, bandY), strokeWidth = 3.dp.toPx())
                        val label = measurer.measure("New track", TextStyle(color = accent, fontSize = 10.sp, fontWeight = FontWeight.SemiBold))
                        drawText(label, topLeft = Offset(8.dp.toPx(), if (m.video) bandY - label.size.height - 2.dp.toPx() else bandY + 2.dp.toPx()))
                    }
                    is Drop.OnTrack -> drawn.firstOrNull { it.track?.id == d.id }?.let { r ->
                        drawRoundRect(accent, Offset(0f, r.top - 2), Size(size.width, r.height + 4), CornerRadius(6.dp.toPx()), style = Stroke(1.5.dp.toPx()))
                    }
                    null -> {}
                }
                val top = (m.y - h / 2).coerceIn(0f, size.height - h)
                drawRoundRect(Color.White.copy(alpha = 0.22f), Offset(x0, top), Size(max(x1 - x0, 4f), h), CornerRadius(6.dp.toPx()))
                drawRoundRect(Color.White, Offset(x0, top), Size(max(x1 - x0, 4f), h), CornerRadius(6.dp.toPx()), style = Stroke(2.dp.toPx()))
            }
            // A swipe-down split: the cut line through the clip (red once letting go splits).
            cut?.let { c ->
                val r = drawn.firstOrNull { row -> row.clips.any { it.id == c.clip } } ?: return@let
                val x = cx + ((c.at - now) * pps).toFloat()
                val color = if (c.armed) Color(0xFFFF5A5F) else Color.White.copy(alpha = 0.6f)
                drawLine(color, Offset(x, r.top - 6.dp.toPx()), Offset(x, r.top + r.height + 6.dp.toPx()), strokeWidth = 3.dp.toPx())
                val label = measurer.measure(if (c.armed) "Let go to split" else "Swipe down to split", TextStyle(color = color, fontSize = 10.sp, fontWeight = FontWeight.SemiBold))
                drawText(label, topLeft = Offset((x - label.size.width / 2f).coerceIn(4f, size.width - label.size.width - 4f), max(rulerH, r.top - label.size.height - 8.dp.toPx())))
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
        // The quick menu, where the finger was.
        menu?.let { m ->
            val (x, y) = with(density) { m.position.x.toDp() to m.position.y.toDp() }
            Box(Modifier.offset(x = x, y = y).size(1.dp)) {
                androidx.compose.material3.DropdownMenu(expanded = true, onDismissRequest = { menu = null }) {
                    @Composable fun item(label: String, action: String, icon: Int) = androidx.compose.material3.DropdownMenuItem(
                        text = { androidx.compose.material3.Text(label) },
                        leadingIcon = { SymIcon(icon, null, Modifier.size(20.dp)) },
                        onClick = { menu = null; onQuick(action, m.clip, m.at) },
                    )
                    if (m.clip != null) {
                        item("Split", "split", Sym.split)
                        item("Cut", "cut", Sym.cutClip)
                        item("Copy", "copy", Sym.duplicate)
                        if (canPaste) item("Paste after", "paste", Sym.paste)
                        item("Duplicate", "duplicate", Sym.duplicate)
                        item("Delete", "delete", Sym.delete)
                    } else if (canPaste) {
                        item("Paste here", "paste", Sym.paste)
                    }
                }
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

/** A transition block: translucent over the cut, a bow-tie mark, and "Name · 0.6 s" when it fits. */
private fun DrawScope.drawTransition(tr: Transition, r: Rect, selected: Boolean, measurer: androidx.compose.ui.text.TextMeasurer) {
    val radius = CornerRadius(5.dp.toPx())
    drawRoundRect(Color(0xCC1E1E24), r.topLeft, r.size, radius)
    drawRoundRect(if (selected) Accent else Color.White, r.topLeft, r.size, radius, style = Stroke(if (selected) 2.dp.toPx() else 1.dp.toPx()))
    // Bow tie: two triangles meeting at the cut.
    val s = min(r.height * 0.28f, 7.dp.toPx())
    val c = Offset(r.center.x, r.top + r.height * 0.38f)
    val tie = Path().apply {
        moveTo(c.x - s, c.y - s); lineTo(c.x, c.y); lineTo(c.x - s, c.y + s); close()
        moveTo(c.x + s, c.y - s); lineTo(c.x, c.y); lineTo(c.x + s, c.y + s); close()
    }
    drawPath(tie, if (selected) Accent else Color.White)
    val style = TextStyle(color = Color.White, fontSize = 9.sp, fontWeight = FontWeight.Medium)
    val full = "${tr.name} · ${"%.1f".format(tr.duration)} s"
    val short = "${"%.1f".format(tr.duration)}s"
    val room = (r.width - 6.dp.toPx()).toInt()
    val label = listOf(full, short).map { measurer.measure(it, style, maxLines = 1) }.firstOrNull { it.size.width <= room }
    if (label != null) drawText(label, topLeft = Offset(r.center.x - label.size.width / 2f, r.bottom - label.size.height - 2.dp.toPx()))
}

/** A keyframe marker: a white diamond with a dark outline. */
private fun DrawScope.drawKeyframe(at: Offset, r: Float) {
    val path = androidx.compose.ui.graphics.Path().apply {
        moveTo(at.x, at.y - r); lineTo(at.x + r, at.y); lineTo(at.x, at.y + r); lineTo(at.x - r, at.y); close()
    }
    drawPath(path, Color.White)
    drawPath(path, Color(0xFF111114), style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.dp.toPx()))
}
