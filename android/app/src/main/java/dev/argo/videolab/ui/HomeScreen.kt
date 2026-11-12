package dev.argo.videolab.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.argo.videolab.VideoLabApp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** Home: start a project (vertical, landscape, square), open the demo or a saved project, or import video. */
@Composable
fun HomeScreen(app: VideoLabApp, onOpen: () -> Unit, onSettings: () -> Unit, onImport: (List<Uri>, Boolean) -> Unit) {
    val state by app.lab.state.collectAsState()
    val scope = rememberCoroutineScope()
    var projects by remember { mutableStateOf<List<File>>(emptyList()) }
    LaunchedEffect(Unit) {
        projects = withContext(Dispatchers.IO) {
            File(app.workspace, "projects").listFiles { f -> f.extension == "fcproj" }?.sortedByDescending { it.lastModified() }.orEmpty()
        }
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> if (uris.isNotEmpty()) onImport(uris, true) }

    fun newProject(w: Int, h: Int) = scope.launch {
        if (app.lab.fn("doc.new", """{"name":"My video","width":$w,"height":$h,"fps":30}""").isSuccess) {
            app.lab.refresh()
            onOpen()
        }
    }

    Surface(Modifier.fillMaxSize(), color = Ink) {
        Column(Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 16.dp)) {
            Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("VideoLab", fontSize = 26.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                if (!state.ready) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                SymButton(Sym.settings, "Settings", onSettings)
            }
            Text("FilmCraft's engine on your phone, with an agent, an MCP server and the FilmCraft CLI.", color = Color.Gray, fontSize = 13.sp)
            Spacer(Modifier.height(16.dp))
            Box(
                Modifier.fillMaxWidth().height(120.dp).clip(RoundedCornerShape(16.dp))
                    .background(Brush.linearGradient(listOf(Color(0xFF2A1F4D), Color(0xFF0F3A47))))
                    .clickable(enabled = state.ready) { picker.launch(arrayOf("video/*", "audio/*", "image/*")) },
                contentAlignment = Alignment.Center,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    SymIcon(Sym.add, null, Modifier.size(32.dp), tint = Color.White)
                    Column(Modifier.padding(start = 12.dp)) {
                        Text("New project from videos", fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
                        Text("Pick clips; they go on the timeline in order", color = Color(0xFFBBBBCC), fontSize = 12.sp)
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                AspectCard("9:16", "Vertical", 9f / 16f, Modifier.weight(1f), state.ready) { newProject(1080, 1920) }
                AspectCard("16:9", "Landscape", 16f / 9f, Modifier.weight(1f), state.ready) { newProject(1920, 1080) }
                AspectCard("1:1", "Square", 1f, Modifier.weight(1f), state.ready) { newProject(1080, 1080) }
            }
            Spacer(Modifier.height(12.dp))
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surface)
                    .clickable(enabled = state.ready) {
                        scope.launch {
                            if (app.lab.fn("doc.demo").isSuccess) {
                                app.lab.refresh()
                                onOpen()
                            }
                        }
                    }.padding(14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                SymIcon(Sym.movie, null, tint = Accent)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text("Demo project", fontWeight = FontWeight.SemiBold)
                    Text("FilmCraft's demo: generated footage, a score, transitions and markers", color = Color.Gray, fontSize = 12.sp)
                }
                SymIcon(Sym.chevronRight, null)
            }
            if (state.timeline != null) {
                Spacer(Modifier.height(8.dp))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surface).clickable(onClick = onOpen).padding(14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    SymIcon(Sym.library, null, tint = Accent)
                    Text("Continue “${state.timeline?.name}”", Modifier.weight(1f).padding(start = 12.dp), fontWeight = FontWeight.SemiBold)
                    SymIcon(Sym.chevronRight, null)
                }
            }
            if (projects.isNotEmpty()) {
                Text("Projects", style = MaterialTheme.typography.titleSmall, color = Color.Gray, modifier = Modifier.padding(top = 20.dp, bottom = 6.dp))
                LazyColumn(Modifier.fillMaxWidth().weight(1f).widthIn(max = 720.dp)) {
                    items(projects) { f ->
                        Row(
                            Modifier.fillMaxWidth().clickable {
                                scope.launch {
                                    if (app.lab.fn("doc.open", """{"path":"projects/${f.name}"}""").isSuccess) {
                                        app.lab.refresh()
                                        onOpen()
                                    }
                                }
                            }.padding(vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            SymIcon(Sym.folder, null, tint = Color.Gray)
                            Text(f.nameWithoutExtension, Modifier.weight(1f).padding(start = 12.dp))
                            Text(java.text.DateFormat.getDateInstance().format(f.lastModified()), color = Color.Gray, fontSize = 12.sp)
                        }
                    }
                }
            }
            state.message?.let { Text(it, color = MaterialTheme.colorScheme.error, fontSize = 12.sp, modifier = Modifier.padding(vertical = 8.dp)) }
        }
    }
}

@Composable
private fun AspectCard(ratio: String, label: String, aspect: Float, modifier: Modifier, enabled: Boolean, onClick: () -> Unit) {
    Column(
        modifier.clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surface).clickable(enabled = enabled, onClick = onClick).padding(12.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(Modifier.height(48.dp), contentAlignment = Alignment.Center) {
            Box(Modifier.height(if (aspect < 1) 44.dp else (44 / aspect).dp).width(if (aspect < 1) (44 * aspect).dp else 44.dp).aspectRatio(aspect).border(2.dp, Color.White, RoundedCornerShape(4.dp)))
        }
        Text(ratio, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp))
        Text(label, color = Color.Gray, fontSize = 11.sp)
    }
}
