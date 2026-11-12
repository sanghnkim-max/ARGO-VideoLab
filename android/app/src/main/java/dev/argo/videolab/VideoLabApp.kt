package dev.argo.videolab

import android.app.Activity
import android.app.Application
import android.os.Bundle
import dev.argo.videolab.agent.Agent
import dev.argo.videolab.agent.Settings
import dev.argo.videolab.core.Lab
import dev.argo.videolab.core.Player
import dev.argo.videolab.core.VideoLab
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File
import java.lang.ref.WeakReference

/**
 * Process-wide singletons: one core (one FilmCraft session) shared by every screen, the in-app
 * agent, the MCP server, FilmCraft's control channel and the CLI. The session lives here, not in
 * an Activity, so rotation and fold/unfold never lose it; screens keep only view state.
 */
class VideoLabApp : Application() {
    val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val workspace: File by lazy { File(filesDir, "VideoLab").apply { mkdirs() } }
    val settings: Settings by lazy { Settings(this) }

    /** FilmCraft's own CLI, bundled unmodified (extracted with the native libraries). */
    val cliPath: String get() = File(applicationInfo.nativeLibraryDir, "libfilmcraft_cli.so").absolutePath
    val videolabCliPath: String get() = File(applicationInfo.nativeLibraryDir, "libvideolab_cli.so").absolutePath

    val lab: Lab by lazy {
        Lab({
            VideoLab(workspace.absolutePath, File(cacheDir, "videolab").apply { mkdirs() }.absolutePath, cliPath, settings.gpu).also { core ->
                core.setUiCallbacks(UiHost(this))
            }
        }, appScope)
    }
    val player: Player by lazy { Player(lab, appScope) }
    val agent: Agent by lazy { Agent(lab, settings) }

    /** The activity on screen, for screenshots requested over FilmCraft's control channel. */
    @Volatile var currentActivity: WeakReference<Activity>? = null
    @Volatile var route: String = "home"

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(object : ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) { currentActivity = WeakReference(activity) }
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {
                if (currentActivity?.get() === activity) currentActivity = null
            }
        })
    }
}
