package com.argo.android.core

/**
 * The names and numbers both ends of the `:core` Binder contract agree on
 * (`IArgoCore.aidl`, `IArgoTab.aidl`, `IArgoTabCallback.aidl`;
 * docs/design/android-core-service.md §2.5).
 *
 * In one place because two processes read them — the service in `:core` and
 * every client (the main app, later the overlay and the browser APK) — and a
 * key spelt twice is a key that drifts.
 */
object CoreContract {
    /** What `IArgoCore.contractVersion` answers. Raised on any change a client could notice. */
    const val VERSION = 1

    /** The permission `ArgoCoreService` is behind (signature|knownSigner). */
    const val PERMISSION_BIND_CORE = "com.argo.android.permission.BIND_CORE"

    /**
     * The service, by component: the ARGO app's package and the service
     * class in it. What a client binds — this app's own processes, and a
     * client in another package (the browser APK), which also needs
     * `<queries><package android:name="com.argo.android"/></queries>` to
     * see it and `BIND_CORE` to be let in.
     */
    const val CORE_PACKAGE = "com.argo.android"
    const val CORE_SERVICE = "com.argo.android.core.ArgoCoreService"

    /**
     * Started with `startForegroundService` by a client about to send the
     * frame that starts work, before it binds (§2.4 "Promotion"). The service
     * always answers it with `startForeground`, even if the work has ended.
     */
    const val ACTION_WORK = "com.argo.android.core.action.WORK"

    // ── openTab's hello (requests only) ─────────────────────────────────────

    const val HELLO_KIND = "kind"
    const val HELLO_RELAY = "relay"
    const val KIND_APP = "app"
    const val KIND_OVERLAY = "overlay"
    const val KIND_BROWSER = "browser"
    const val KIND_CLIENT = "client"

    // ── openTab's Bundle ────────────────────────────────────────────────────

    /** The `IArgoTab` binder. */
    const val TAB = "tab"

    /** The caller's end of the tab's bulk socket, a `ParcelFileDescriptor`. */
    const val BULK = "bulk"

    /** The epoch of the booted core, a `String`; absent while it boots. */
    const val CORE_EPOCH = "coreEpoch"

    // ── where ARGO runs (`IArgoCore.executionState`, `setExecution`) ────────

    /** The phone's own harness; nothing remote runs. The default and the fail-safe. */
    const val MODE_STANDALONE = "standalone"

    /** A host device on the mesh runs the harness; this phone is its client. */
    const val MODE_SELF_HOSTED = "self-hosted"

    /**
     * A cloud tenant behind a gateway: `host` of `setExecution` is the
     * gateway's address (`wss://host[:port]`), and the sign-in is the token
     * given to `setGatewayToken`. Falls back to the phone's own harness, and
     * says why (`link.reason`), when it cannot be reached or signed in to.
     */
    const val MODE_CLOUD = "cloud"

    /** `executionState`'s members. */
    const val EXECUTION_MODE = "mode"
    const val EXECUTION_HOST = "host"
    const val EXECUTION_LINK = "link"
    const val EXECUTION_LINK_STATE = "state"
    const val EXECUTION_LINK_HOST = "host"
    const val EXECUTION_LINK_DETAIL = "detail"

    /**
     * The link's failure as one word: `unreachable`, `auth`, `busy`,
     * `not-approved` or `refused`; absent when the link is not bad news.
     */
    const val EXECUTION_LINK_REASON = "reason"

    /** The gateway's address the cloud choice names (`cloud` mode); absent otherwise. */
    const val EXECUTION_GATEWAY = "gateway"

    /** Whether a gateway token is held on this phone (never the token). */
    const val EXECUTION_SIGNED_IN = "signedIn"
    const val EXECUTION_PEERS = "peers"
    const val EXECUTION_PEER_NAME = "name"
    const val EXECUTION_PEER_CLASS = "class"
    const val EXECUTION_CLOUD_AVAILABLE = "cloudAvailable"

    // ── the realtime model (`IArgoCore.voiceModelState`, `setVoiceModel`) ────

    /** VIA's existing GPT Realtime connection. The default. */
    const val VOICE_MODEL_GPT = "gpt"

    /** Samsung's SR Realtime connection; offered only when its credentials are on the device. */
    const val VOICE_MODEL_SR = "sr"

    /** `voiceModelState`'s members: the chosen profile, whether SR can be offered, and [VOICE_MODEL_VIA_ON]. */
    const val VOICE_MODEL_PROFILE = "profile"
    const val VOICE_MODEL_SR_AVAILABLE = "srAvailable"

    /** `voiceModelState`'s member saying whether the running core mounts VIA; the choice is shown only then. */
    const val VOICE_MODEL_VIA_ON = "viaOn"

    /** `link.state`'s values. */
    const val LINK_OFF = "off"
    const val LINK_CONNECTING = "connecting"
    const val LINK_ATTACHED = "attached"
    const val LINK_HOST_UNREACHABLE = "host-unreachable"

    // ── the two syncs (`IArgoCore.syncState`, `setSync`) ─────────────────────

    /** `setSync`'s `kind`: this phone's main conversation, synced with its host. */
    const val SYNC_MAIN = "main"

    /** `setSync`'s `kind`: what ARGO remembers, synced across the person's devices. */
    const val SYNC_MEMORY = "memory"

    /** `syncState`'s members. */
    const val SYNC_PAIRED = "paired"
    const val SYNC_MAIN_STATE = "main"
    const val SYNC_MEMORY_STATE = "memory"
    const val SYNC_ENABLED = "enabled"
    const val SYNC_VIA = "via"
    const val SYNC_HOST = "host"
    const val SYNC_MODE = "mode"
    const val SYNC_STATE = "sync"

    /** Why the main sync is `offline`: `unreachable`, `not-serving`, `auth`, `protocol` or `unknown`. */
    const val SYNC_FAILURE = "failure"
    const val SYNC_RING_KEY = "ringKey"

    // ── frames ──────────────────────────────────────────────────────────────

    /**
     * The largest frame, in UTF-8 bytes, a client sends inline with
     * `IArgoTab.send`. Anything larger goes on the bulk socket and is named
     * by `sendBulk`, so no request transaction can approach the Binder
     * buffer (about 1 MiB per process, shared). The same figure the core
     * batches at.
     */
    const val INLINE_FRAME_BYTES = 16 * 1024

    /** The reason `onReset` names when a tab opened during a boot gets the booted core. */
    const val RESET_CORE_READY = "core-ready"

    // ── notifications into the main app ─────────────────────────────────────

    /**
     * What a tapped `:core` notification opens the main Activity with
     * (§7.2.5): [EXTRA_SESSION_ID] names the session, `""` for Home. The
     * Activity hands it to the page as the `notification` push.
     */
    const val ACTION_OPEN_SESSION = "com.argo.android.action.OPEN_SESSION"

    /**
     * What a habit's milestone notification opens the main Activity with:
     * the Calendar (`docs/design/daily-guide.md`). Carries nothing else.
     */
    const val ACTION_OPEN_CALENDAR = "com.argo.android.action.OPEN_CALENDAR"

    /** Open the run screen (the widget's Start run, the run's notification, the "ARGO Run" launcher alias). */
    const val ACTION_OPEN_RUN = "com.argo.android.action.OPEN_RUN"

    /** Open the Calendar's Day view (the "ARGO Today" launcher alias, the widget). */
    const val ACTION_OPEN_TODAY = "com.argo.android.action.OPEN_TODAY"

    /**
     * Open a web page in ARGO's browser (the assist overlay's hand-off):
     * [EXTRA_URL] is the address, [EXTRA_SESSION_ID] the session it was
     * opened from, if any.
     */
    const val ACTION_OPEN_PAGE = "com.argo.android.action.OPEN_PAGE"

    /**
     * What a scout's notification opens the main Activity with: the Calendar's
     * Watching section, the scout named by [EXTRA_SCOUT_ID] expanded.
     */
    const val ACTION_OPEN_WATCHING = "com.argo.android.action.OPEN_WATCHING"

    /** The scout a [ACTION_OPEN_WATCHING] intent names: its own page; blank opens the Calendar's Watching section alone. */
    const val EXTRA_SCOUT_ID = "scoutId"

    /**
     * What a triggers notice's tap opens the main Activity with: the main session, the offer's
     * request sent as the person's message with the notice as context
     * (`docs/design/triggers.md` §8.3). [EXTRA_TRIGGER_SEAL] is the HMAC `:core` made over the
     * rest, so an intent from another app cannot speak for the person.
     */
    const val ACTION_OPEN_SUGGESTION = "com.argo.android.action.OPEN_SUGGESTION"

    /** The candidate id (`<kind>:<key>`) a triggers notice is about. */
    const val EXTRA_TRIGGER_ID = "triggerId"

    /** The offer's one-line request, sent as the person's message; blank for information only. */
    const val EXTRA_TRIGGER_REQUEST = "triggerRequest"

    /** The one-line summary of the context row. */
    const val EXTRA_TRIGGER_SUMMARY = "triggerSummary"

    /** The context text (kind, tool, the notice's words and why). */
    const val EXTRA_TRIGGER_TEXT = "triggerText"

    /** The seal over id, request, summary, text and the offer's `url` and `package` arguments. */
    const val EXTRA_TRIGGER_SEAL = "triggerSeal"

    /**
     * The offer's `url` argument when it has one. Its own extra, not [EXTRA_URL]: that one is
     * what [ACTION_OPEN_PAGE] opens in the browser, and a suggestion must never be read as one.
     * It rides under the seal beside [EXTRA_TRIGGER_PACKAGE], and the host re-checks both
     * before the request is sent (`OfferGuard`).
     */
    const val EXTRA_TRIGGER_URL = "triggerUrl"

    /**
     * The offer's `package` argument when it has one, under the seal beside
     * [EXTRA_TRIGGER_URL]; a notice with neither carries neither.
     */
    const val EXTRA_TRIGGER_PACKAGE = "triggerPackage"

    const val EXTRA_SESSION_ID = "sessionId"

    /**
     * Words to put in the input of the session [ACTION_OPEN_SESSION] opens (or
     * of a new chat, when [EXTRA_SESSION_ID] is blank), **not sent**: a
     * notice's follow-up (`docs/design/notify-then-act.md`).
     */
    const val EXTRA_PREFILL = "prefill"

    /**
     * The event in the session [ACTION_OPEN_SESSION] opens that the thread scrolls to (a `notify`
     * notice's message). A navigation hint only: unsealed, and ignored when no such row exists.
     */
    const val EXTRA_EVENT_ID = "eventId"

    /** The address [ACTION_OPEN_PAGE] opens. */
    const val EXTRA_URL = "url"

    /**
     * The seal on an [ACTION_OPEN_SESSION] or [ACTION_OPEN_PAGE] launch: an HMAC this install
     * made, with a key kept in its own private storage, over the action, [EXTRA_SESSION_ID], the
     * data URI, [EXTRA_PREFILL] and [EXTRA_URL] together. The main Activity is exported, so
     * without it another app could choose the session the person sees, put words in its input,
     * aim at an ask, or load any page in ARGO's browser; an unsealed launch only brings ARGO
     * forward (`LaunchRequest`). Never read by anyone but the app that made it.
     */
    const val EXTRA_LAUNCH_SEAL = "launchSeal"

    // ── llmAccount (the LLM session broker's account) ───────────────────────

    /** `llmAccount`'s members. Never a token. */
    const val LLM_CONFIGURED = "configured"
    const val LLM_SIGNED_IN = "signedIn"
    const val LLM_LABEL = "label"
    const val LLM_PHASE = "phase"
    const val LLM_PROBLEM = "problem"

    /** `LLM_PHASE`'s values. */
    const val LLM_PHASE_SIGNED_OUT = "signed-out"
    const val LLM_PHASE_FETCHING = "fetching"
    const val LLM_PHASE_READY = "ready"
    const val LLM_PHASE_FAILED = "failed"

    /** `LLM_PROBLEM`'s values (when the phase is failed). */
    const val LLM_PROBLEM_DENIED = "denied"
    const val LLM_PROBLEM_REJECTED = "rejected"
    const val LLM_PROBLEM_UNAVAILABLE = "unavailable"
    const val LLM_PROBLEM_BAD_REPLY = "bad-reply"
}
