package dev.argo.videolab.ui

import androidx.annotation.DrawableRes
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import dev.argo.videolab.R

/** A dark editing theme: pictures read best on a near-black surround. */
@Composable
fun VideoLabTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Accent,
            onPrimary = Color.Black,
            secondary = Color(0xFF8AB4F8),
            background = Ink,
            surface = Color(0xFF18181B),
            surfaceVariant = Color(0xFF26262A),
            surfaceContainer = Color(0xFF18181B),
            surfaceContainerHigh = Color(0xFF222226),
            error = Color(0xFFE5807A),
        ),
        content = content,
    )
}

val Ink = Color(0xFF0E0E10)
val Accent = Color(0xFF3FD0E8)
val Playhead = Color.White
val ClipVideo = Color(0xFF2B2B30)
val ClipAudio = Color(0xFF1F7FB8)
val ClipMusic = Color(0xFF2E9E5B)
val ClipText = Color(0xFF7B3FE4)
val Selected = Color.White
val OkGreen = Color(0xFF8FBF8F)
val WarnAmber = Color(0xFFE0B060)

/** Material Symbols Outlined glyphs (android/scripts/material-symbols.sh). */
object Sym {
    @DrawableRes val play = R.drawable.ic_sym_play_arrow
    @DrawableRes val pause = R.drawable.ic_sym_pause
    @DrawableRes val skipPrev = R.drawable.ic_sym_skip_previous
    @DrawableRes val skipNext = R.drawable.ic_sym_skip_next
    @DrawableRes val undo = R.drawable.ic_sym_undo
    @DrawableRes val redo = R.drawable.ic_sym_redo
    @DrawableRes val export = R.drawable.ic_sym_ios_share
    @DrawableRes val agent = R.drawable.ic_sym_auto_awesome
    @DrawableRes val settings = R.drawable.ic_sym_settings
    @DrawableRes val add = R.drawable.ic_sym_add
    @DrawableRes val close = R.drawable.ic_sym_close
    @DrawableRes val send = R.drawable.ic_sym_send
    @DrawableRes val chevronLeft = R.drawable.ic_sym_chevron_left
    @DrawableRes val chevronRight = R.drawable.ic_sym_chevron_right
    @DrawableRes val expandMore = R.drawable.ic_sym_expand_more
    @DrawableRes val expandLess = R.drawable.ic_sym_expand_less
    @DrawableRes val code = R.drawable.ic_sym_code
    @DrawableRes val image = R.drawable.ic_sym_image
    @DrawableRes val terminal = R.drawable.ic_sym_terminal
    @DrawableRes val delete = R.drawable.ic_sym_delete
    @DrawableRes val split = R.drawable.ic_sym_content_cut
    @DrawableRes val speed = R.drawable.ic_sym_speed
    @DrawableRes val volume = R.drawable.ic_sym_volume_up
    @DrawableRes val mute = R.drawable.ic_sym_volume_off
    @DrawableRes val music = R.drawable.ic_sym_music_note
    @DrawableRes val captions = R.drawable.ic_sym_closed_caption
    @DrawableRes val text = R.drawable.ic_sym_text_fields
    @DrawableRes val title = R.drawable.ic_sym_title
    @DrawableRes val movie = R.drawable.ic_sym_movie
    @DrawableRes val library = R.drawable.ic_sym_video_library
    @DrawableRes val fullscreen = R.drawable.ic_sym_fullscreen
    @DrawableRes val back = R.drawable.ic_sym_keyboard_double_arrow_left
    @DrawableRes val fade = R.drawable.ic_sym_gradient
    @DrawableRes val stop = R.drawable.ic_sym_stop
    @DrawableRes val check = R.drawable.ic_sym_check
    @DrawableRes val home = R.drawable.ic_sym_home
    @DrawableRes val adjust = R.drawable.ic_sym_tune
    @DrawableRes val effects = R.drawable.ic_sym_auto_fix_high
    @DrawableRes val transition = R.drawable.ic_sym_animation
    @DrawableRes val addMedia = R.drawable.ic_sym_library_add
    @DrawableRes val filters = R.drawable.ic_sym_photo_filter
    @DrawableRes val transform = R.drawable.ic_sym_open_with
    @DrawableRes val freeze = R.drawable.ic_sym_pause_presentation
    @DrawableRes val reverse = R.drawable.ic_sym_fast_rewind
    @DrawableRes val mirror = R.drawable.ic_sym_flip
    @DrawableRes val rotate = R.drawable.ic_sym_rotate_90_degrees_cw
    @DrawableRes val autoCut = R.drawable.ic_sym_auto_fix_normal
    @DrawableRes val duck = R.drawable.ic_sym_volume_down
    @DrawableRes val loudness = R.drawable.ic_sym_graphic_eq
    @DrawableRes val narrate = R.drawable.ic_sym_record_voice_over
    @DrawableRes val slowmo = R.drawable.ic_sym_slow_motion_video
    @DrawableRes val autoCaptions = R.drawable.ic_sym_subtitles
    @DrawableRes val keyframe = R.drawable.ic_sym_diamond
    @DrawableRes val keyframeOn = R.drawable.ic_sym_diamond_fill
    @DrawableRes val folder = R.drawable.ic_sym_folder_open
    @DrawableRes val templates = R.drawable.ic_sym_dashboard_customize
    @DrawableRes val swap = R.drawable.ic_sym_swap_horiz
    @DrawableRes val crop = R.drawable.ic_sym_crop
    @DrawableRes val mask = R.drawable.ic_sym_vignette
    @DrawableRes val opacity = R.drawable.ic_sym_opacity
    @DrawableRes val chromaKey = R.drawable.ic_sym_format_color_reset
    @DrawableRes val stabilize = R.drawable.ic_sym_video_stable
    @DrawableRes val reframe = R.drawable.ic_sym_crop_free
    @DrawableRes val voice = R.drawable.ic_sym_voice_selection
    @DrawableRes val duplicate = R.drawable.ic_sym_content_copy
    @DrawableRes val detach = R.drawable.ic_sym_link_off
    @DrawableRes val canvas = R.drawable.ic_sym_aspect_ratio
    @DrawableRes val closeGaps = R.drawable.ic_sym_space_bar
}

@Composable
fun SymIcon(@DrawableRes res: Int, contentDescription: String?, modifier: Modifier = Modifier, tint: Color? = null) {
    if (tint != null) Icon(painterResource(res), contentDescription, modifier, tint = tint)
    else Icon(painterResource(res), contentDescription, modifier)
}

@Composable
fun SymButton(@DrawableRes res: Int, label: String, onClick: () -> Unit, enabled: Boolean = true, tint: Color? = null) {
    IconButton(onClick = onClick, enabled = enabled) { SymIcon(res, label, tint = tint) }
}
