package dev.argo.videolab.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

/** What kind of picture or sound a clip carries, for its colour and how it is drawn. */
enum class ClipKind { Video, Still, Graphic, Audio, Music }

data class Clip(
    val id: Long,
    val item: Long?,
    val name: String,
    val start: Double,
    val end: Double,
    val sourceIn: Double,
    val sourceOut: Double,
    val speed: Double,
    val gainDb: Double,
    val link: Long?,
    val effects: List<String>,
    val kind: ClipKind,
) {
    val duration get() = end - start
}

data class Transition(val id: Long, val effect: String, val start: Double, val duration: Double)

data class Track(val id: Long, val name: String, val video: Boolean, val muted: Boolean, val clips: List<Clip>, val transitions: List<Transition>)

data class Marker(val id: Long, val name: String, val at: Double, val color: String)

data class Item(val id: Long, val name: String, val type: String, val duration: Double)

/** The active sequence as `videolab-core`'s compact projection describes it (seconds). */
data class Timeline(
    val sequence: Long,
    val name: String,
    val width: Int,
    val height: Int,
    val fps: Double,
    val duration: Double,
    val playhead: Double,
    val selection: List<Long>,
    val markers: List<Marker>,
    val tracks: List<Track>,
    /** FilmCraft's document epoch: item ids are reused across projects, caches by item are per epoch. */
    val epoch: Long = 0,
) {
    val videoTracks get() = tracks.filter { it.video }
    val audioTracks get() = tracks.filter { !it.video }
    fun clip(id: Long): Clip? = tracks.firstNotNullOfOrNull { t -> t.clips.firstOrNull { it.id == id } }
    fun trackOf(id: Long): Track? = tracks.firstOrNull { t -> t.clips.any { it.id == id } }
    val aspect: Float get() = if (height > 0) width.toFloat() / height else 16f / 9f

    companion object {
        fun parse(e: JsonElement?, items: Map<Long, Item>): Timeline? {
            val o = e as? JsonObject ?: return null
            val tracks = (o["tracks"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { t ->
                val video = t.s("kind") == "video"
                Track(
                    id = t.l("id") ?: 0,
                    name = t.s("name") ?: "",
                    video = video,
                    muted = t.b("muted") ?: false,
                    clips = (t["clips"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { c ->
                        val item = c.l("item")
                        val type = item?.let { items[it]?.type }.orEmpty()
                        Clip(
                            id = c.l("clip") ?: 0,
                            item = item,
                            name = c.s("name") ?: "",
                            start = c.d("start") ?: 0.0,
                            end = c.d("end") ?: 0.0,
                            sourceIn = c.d("sourceIn") ?: 0.0,
                            sourceOut = c.d("sourceOut") ?: 0.0,
                            speed = c.d("speed") ?: 1.0,
                            gainDb = c.d("gainDb") ?: 0.0,
                            link = c.l("link"),
                            effects = (c["effects"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
                            kind = kindOf(video, type, c.l("link") != null),
                        )
                    },
                    transitions = (t["transitions"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { x ->
                        Transition(x.l("id") ?: 0, x.s("effect") ?: "", x.d("start") ?: 0.0, x.d("duration") ?: 0.0)
                    },
                )
            }
            return Timeline(
                sequence = o.l("sequence") ?: 0,
                name = o.s("name") ?: "Untitled",
                width = (o.l("width") ?: 1920).toInt(),
                height = (o.l("height") ?: 1080).toInt(),
                fps = o.d("fps") ?: 30.0,
                duration = o.d("duration") ?: 0.0,
                playhead = o.d("playhead") ?: 0.0,
                selection = (o["selection"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.longOrNull },
                markers = (o["markers"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { m ->
                    Marker(m.l("id") ?: 0, m.s("name") ?: "", m.d("at") ?: 0.0, m.s("color") ?: "")
                },
                tracks = tracks,
                epoch = o.l("epoch") ?: 0,
            )
        }

        private fun kindOf(video: Boolean, type: String, linked: Boolean): ClipKind = when {
            !video && linked -> ClipKind.Audio
            !video -> ClipKind.Music
            type.equals("Movie", true) -> ClipKind.Video
            type.equals("Still", true) || type.equals("Image", true) -> ClipKind.Still
            type.isEmpty() || type.contains("Graphic", true) || type.contains("Title", true) -> ClipKind.Graphic
            else -> ClipKind.Video
        }
    }
}

fun parseItems(e: JsonElement?): List<Item> =
    (e as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.map { i ->
        Item(i.l("item") ?: 0, i.s("name") ?: "", i.s("type") ?: "", i.d("duration") ?: 0.0)
    }

internal fun JsonObject.s(k: String) = (this[k] as? JsonPrimitive)?.contentOrNull
internal fun JsonObject.l(k: String) = (this[k] as? JsonPrimitive)?.longOrNull
internal fun JsonObject.d(k: String) = (this[k] as? JsonPrimitive)?.doubleOrNull
internal fun JsonObject.b(k: String) = (this[k] as? JsonPrimitive)?.booleanOrNull

/** `mm:ss` (or `h:mm:ss`) for the transport; `mm:ss.f` with [tenths]. */
fun formatTime(seconds: Double, tenths: Boolean = false): String {
    val s = seconds.coerceAtLeast(0.0)
    val whole = s.toLong()
    val h = whole / 3600
    val m = (whole / 60) % 60
    val sec = whole % 60
    val base = if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%02d:%02d".format(m, sec)
    return if (tenths) base + ".%d".format(((s - whole) * 10).toInt().coerceIn(0, 9)) else base
}
