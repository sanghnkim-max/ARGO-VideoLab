package com.argo.android.core.client

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.os.BundleCompat
import com.argo.android.core.BulkInbox
import com.argo.android.core.BulkRecords
import com.argo.android.core.CoreContract
import com.argo.android.core.IArgoCore
import com.argo.android.core.IArgoTab
import com.argo.android.core.IArgoTabCallback
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.OutputStream
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * One client's tab on `:core`, from a process that is not `:core`: bind,
 * `openTab`, read batches in order, acknowledge them, and send frames —
 * inline, or over the bulk socket when they are large (§2.5).
 *
 * The client half of the contract `ArgoCoreService` serves, for the main
 * app's native UI (`client.tab.TabArgoStore`, kept bound by
 * `client.tab.CoreLinkHost`) and for the device test that drives `:core`
 * from outside. Rebinding after `:core` dies — with a capped backoff (§2.5
 * "Death") — is the host's (`CoreLinkHost`); this reports the loss through
 * [Listener.onDisconnected] and stops.
 *
 * Each tab it opens is read from a fresh [TabReceiver]: a new tab numbers
 * its batches from 1, so a receiver that remembered the last tab's `seq`
 * would drop every batch of the next one as a duplicate. Android re-delivers
 * `onServiceConnected` on its own when a `BIND_AUTO_CREATE` binding's
 * process comes back, and once [close] has run nothing it delivers opens a
 * tab.
 *
 * Threads: [Listener] is called on this connection's own thread, in order.
 */
class CoreConnection(
    private val context: Context,
    private val listener: Listener,
    /** Ask for the Activity relay role (the main process only; granted there only). */
    private val relay: Boolean = false,
) {
    /** What happens on the tab, in order, on one thread. */
    interface Listener : TabReceiver.Listener {
        /** The tab is open; [coreEpoch] is null while the core boots (it comes with `onReset`). */
        fun onOpened(coreEpoch: String?)

        /** `:core` went away, or refused this client. */
        fun onDisconnected(reason: String)
    }

    private val thread = Executors.newSingleThreadExecutor { runnable -> Thread(runnable, "argo-core-client").apply { isDaemon = true } }

    @Volatile private var tab: IArgoTab? = null

    /** The core's binder while bound: for the calls that are not a tab's. */
    @Volatile private var core: IArgoCore? = null
    @Volatile private var bulkOut: OutputStream? = null
    @Volatile private var bulkFd: ParcelFileDescriptor? = null
    @Volatile private var inbox: BulkInbox? = null
    @Volatile private var closed = false
    private var bound = false
    private val nextBulk = AtomicLong(0)

    /** The current tab's reader; replaced for every tab opened (see the class note). Connection thread only. */
    private var receiver = newReceiver()

    private fun newReceiver() = TabReceiver(
        bulk = { id -> inbox?.take(id, BULK_WAIT_MS) ?: throw IllegalStateException("no bulk socket") },
        listener = listener,
    )

    private val callback = object : IArgoTabCallback.Stub() {
        override fun onBatch(seq: Long, framesJson: String) = onThread {
            val answer = try {
                receiver.onBatch(seq, framesJson)
            } catch (error: Exception) {
                Log.w(TAG, "batch $seq could not be read; asking for it again", error)
                TabReceiver.Answer.Resume(receiver.last)
            }
            when (answer) {
                is TabReceiver.Answer.Ack -> tab?.ack(answer.seq)
                is TabReceiver.Answer.Resume -> tab?.resume(answer.lastSeq)
                TabReceiver.Answer.Nothing -> {}
            }
        }

        override fun onReset(coreEpoch: String, reason: String) = onThread { receiver.onReset(coreEpoch, reason) }

        override fun onClosed(reason: String) = onThread { receiver.onClosed(reason) }
    }

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) = onThread {
            if (closed) return@onThread
            val core = IArgoCore.Stub.asInterface(service)
            this@CoreConnection.core = core
            val hello = buildJsonObject {
                put(CoreContract.HELLO_KIND, CoreContract.KIND_CLIENT)
                put(CoreContract.HELLO_RELAY, relay)
            }.toString()
            val opened = when (val attempt = openOn(core, hello, callback)) {
                is Attempt.Opened -> attempt.tab
                is Attempt.Failed -> {
                    listener.onDisconnected(attempt.reason)
                    return@onThread
                }
            }
            // A tab before this one (the service came back): its socket goes with it.
            runCatching { bulkFd?.close() }
            bulkFd = null
            bulkOut = null
            inbox = null
            receiver = newReceiver()
            val fd = BundleCompat.getParcelable(opened, CoreContract.BULK, ParcelFileDescriptor::class.java)
            if (fd != null) {
                bulkFd = fd
                bulkOut = FileOutputStream(fd.fileDescriptor)
                inbox = BulkInbox(FileInputStream(fd.fileDescriptor)).also { reading ->
                    Thread(reading::run, "argo-core-client-bulk").apply { isDaemon = true }.start()
                }
            }
            tab = IArgoTab.Stub.asInterface(opened.getBinder(CoreContract.TAB))
            listener.onOpened(opened.getString(CoreContract.CORE_EPOCH))
        }

        override fun onServiceDisconnected(name: ComponentName?) = onThread {
            tab = null
            core = null
            listener.onDisconnected("the core went away")
        }

        // The binding itself is dead (the app was updated under it, say):
        // Android will not reconnect it, so the host must bind again.
        override fun onBindingDied(name: ComponentName?) = onThread {
            tab = null
            listener.onDisconnected("the core's binding died")
        }

        override fun onNullBinding(name: ComponentName?) = onThread {
            listener.onDisconnected("the core refused to bind")
        }
    }

    /** Bind to `:core` and open the tab. */
    fun connect() {
        // By component, not by class: this client also ships in the browser
        // APK, where the service is another package's.
        val intent = Intent().setComponent(ComponentName(CoreContract.CORE_PACKAGE, CoreContract.CORE_SERVICE))
        // BIND_INCLUDE_CAPABILITIES: a client on screen lends `:core` its
        // while-in-use capability, which the `microphone` foreground type of
        // a voice conversation needs (VIA#311, design §7.3.2).
        // BIND_ALLOW_ACTIVITY_STARTS: a client on screen lends `:core` its
        // right to start screens, so `app_open` asked from that client opens
        // the app; from Android 14 a bound service gets it only by this opt-in.
        bound = context.bindService(
            intent,
            connection,
            Context.BIND_AUTO_CREATE or Context.BIND_INCLUDE_CAPABILITIES or Context.BIND_ALLOW_ACTIVITY_STARTS,
        )
        if (!bound) onThread { listener.onDisconnected("the core service could not be bound") }
    }

    /**
     * Send one frame: inline up to [CoreContract.INLINE_FRAME_BYTES] of
     * UTF-8, otherwise as a bulk record named by `sendBulk`. False when no
     * tab is open.
     */
    fun send(frame: String): Boolean {
        val tab = tab ?: return false
        if (frame.toByteArray(Charsets.UTF_8).size <= CoreContract.INLINE_FRAME_BYTES) {
            tab.send(frame)
            return true
        }
        val out = bulkOut ?: return false
        val id = nextBulk.incrementAndGet()
        // The record before the call that names it; `:core` waits for it.
        synchronized(out) { BulkRecords.write(out, id, frame) }
        tab.sendBulk(id)
        return true
    }

    /**
     * Where ARGO runs and how that stands (`IArgoCore.executionState`), as
     * JSON; null when not bound or the core is older than the call. Blocking
     * Binder call: not on the main thread.
     */
    fun executionState(): String? = try {
        core?.executionState()
    } catch (error: Exception) {
        Log.w(TAG, "the core would not say where it runs", error)
        null
    }

    /** `IArgoCore.setExecution`; false when refused or not reachable. Blocking. */
    fun setExecution(mode: String, host: String?): Boolean = try {
        core?.setExecution(mode, host.orEmpty()) ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not take the choice of where it runs", error)
        false
    }

    /**
     * The realtime model VIA uses (`IArgoCore.voiceModelState`), as JSON; null
     * when not bound or the core is older than the call. Blocking Binder call.
     */
    fun voiceModelState(): String? = try {
        core?.voiceModelState()
    } catch (error: Exception) {
        Log.w(TAG, "the core would not say which realtime model it uses", error)
        null
    }

    /** `IArgoCore.setVoiceModel`; false when refused or not reachable. Blocking. */
    fun setVoiceModel(profile: String): Boolean = try {
        core?.setVoiceModel(profile) ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not take the realtime model", error)
        false
    }

    /**
     * How the two syncs stand (`IArgoCore.syncState`), as JSON; null when not
     * bound or the core is older than the call. Blocking Binder call.
     */
    fun syncState(): String? = try {
        core?.syncState()
    } catch (error: Exception) {
        Log.w(TAG, "the core would not say how its syncs stand", error)
        null
    }

    /** `IArgoCore.setGatewayToken`; false when refused, not reachable, or older. Blocking. */
    fun setGatewayToken(token: String): Boolean = try {
        core?.setGatewayToken(token) ?: false
    } catch (error: Exception) {
        // Never the token itself.
        Log.w(TAG, "the core would not take the gateway sign-in", error)
        false
    }

    /** `IArgoCore.llmAccount`: the broker's account as JSON; null when not bound or the core is older than the call. Blocking. */
    fun llmAccount(): String? = try {
        core?.llmAccount()
    } catch (error: Exception) {
        Log.w(TAG, "the core would not say how the LLM account stands", error)
        null
    }

    /** `IArgoCore.llmSignIn`; false when refused, not reachable, or older. Blocking (bounded by the core). */
    fun llmSignIn(googleIdToken: String): Boolean = try {
        core?.llmSignIn(googleIdToken) ?: false
    } catch (error: Exception) {
        // Never the token.
        Log.w(TAG, "the core would not take the LLM sign-in", error)
        false
    }

    /** `IArgoCore.companionDownloadToken`: the Firebase ID token for the companion download; null when refused, signed out, not reachable, or older. Blocking. */
    fun companionDownloadToken(): String? = try {
        core?.companionDownloadToken()
    } catch (error: Exception) {
        Log.w(TAG, "the core gave no companion download token", error)
        null
    }

    /** `IArgoCore.llmSignOut`; false when refused, not reachable, or older. Blocking. */
    fun llmSignOut(): Boolean = try {
        core?.llmSignOut() ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not sign the LLM account out", error)
        false
    }

    /** `IArgoCore.setSync`; false when refused or not reachable. Blocking. */
    fun setSync(kind: String, enabled: Boolean, address: String?, ringKey: String?): Boolean = try {
        core?.setSync(kind, enabled, address.orEmpty(), ringKey.orEmpty()) ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not take the sync change", error)
        false
    }

    /** `IArgoCore.wipeMemory`; false when refused or not reachable. Blocking. */
    fun wipeMemory(everywhere: Boolean): Boolean = try {
        core?.wipeMemory(everywhere) ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not wipe the synced memory", error)
        false
    }

    /** `IArgoCore.leaveMesh`; false when refused or not reachable. Blocking. */
    fun leaveMesh(): Boolean = try {
        core?.leaveMesh() ?: false
    } catch (error: Exception) {
        Log.w(TAG, "the core would not leave the mesh", error)
        false
    }

    /** Close the tab and unbind. */
    fun close() {
        closed = true
        runCatching { tab?.close() }
        tab = null
        if (bound) {
            runCatching { context.unbindService(connection) }
            bound = false
        }
        runCatching { bulkFd?.close() }
        thread.shutdown()
    }

    private fun onThread(task: () -> Unit) {
        try {
            thread.execute(task)
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            // Closed.
        }
    }

    companion object {
        private const val TAG = "argo-core-client"
        private const val BULK_WAIT_MS = 30_000L

        /**
         * How [Listener.onDisconnected]'s reason begins when the core refused
         * this client (not on its allow-list, or another contract version).
         * Retrying does not change that: only an update of either app does.
         */
        const val REFUSED = "the core refused this client"

        /** Whether a disconnect [reason] is a refusal ([REFUSED]) rather than a loss. */
        @JvmStatic
        fun isRefusal(reason: String): Boolean = reason.startsWith(REFUSED)

        /** How [openOn] ended: the tab's bundle, or the [Listener.onDisconnected] reason. */
        internal sealed interface Attempt {
            class Opened(val tab: Bundle) : Attempt

            class Failed(val reason: String) : Attempt
        }

        /**
         * The setup call sequence on a bound [core]: check its contract
         * version, then `openTab`. The version is read once and compared
         * here rather than by `check`, whose IllegalStateException is the
         * type the core throws for a transient loss, which must not be told
         * as a refusal. A failure of either call is classified by
         * [setupFailure].
         */
        internal fun openOn(core: IArgoCore, hello: String, callback: IArgoTabCallback?): Attempt {
            val version = try {
                core.contractVersion()
            } catch (error: Exception) {
                return Attempt.Failed(setupFailure(error))
            }
            if (version != CoreContract.VERSION) return Attempt.Failed(contractRefusal(version))
            return try {
                Attempt.Opened(core.openTab(hello, callback))
            } catch (error: Exception) {
                Attempt.Failed(setupFailure(error))
            }
        }

        /** The refusal reason for a core that speaks contract [version] instead of ours. */
        internal fun contractRefusal(version: Int): String = "$REFUSED: the core speaks contract $version"

        /**
         * The disconnect reason for an [error] while binding and opening the
         * tab. Only a SecurityException is a refusal: it is what `openTab`
         * throws for a caller off the allow-list, and Binder carries it to the
         * client as the same type. Everything else — the core dying mid-call
         * (`DeadObjectException`, any `RemoteException`), or its own "the
         * client died while opening its tab" — is a loss, which a client
         * answers by retrying, so it must not begin with [REFUSED].
         */
        internal fun setupFailure(error: Throwable): String = when (error) {
            is SecurityException -> "$REFUSED: ${error.message ?: error}"
            else -> "the core link failed while opening: ${error.message ?: error}"
        }
    }
}
