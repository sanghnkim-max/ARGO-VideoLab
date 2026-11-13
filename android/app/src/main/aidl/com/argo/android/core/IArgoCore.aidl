package com.argo.android.core;

import android.app.PendingIntent;
import com.argo.android.core.IArgoTabCallback;

/**
 * What binding `ArgoCoreService` returns: the one ARGO core on this device,
 * in the `:core` process (docs/design/android-core-service.md §2.5).
 *
 * Both methods are synchronous and return at once; everything after them is
 * oneway. The constants every value below names are in `CoreContract`.
 */
interface IArgoCore {
    /** The contract version this service speaks: `CoreContract.VERSION`. */
    int contractVersion();

    /**
     * Attach the caller as one tab of the hub.
     *
     * `helloJson` is `{"kind": "app" | "overlay" | "browser", "relay": bool}`,
     * and every member of it is a request: the kind a tab gets, and whether
     * it holds the Activity relay, follow from the caller's uid, pid and
     * signing certificate, never from what it says. A caller the service
     * does not recognise is refused with a `SecurityException`.
     *
     * Returns a Bundle with `tab` (the `IArgoTab` binder), `bulk` (the
     * caller's end of the tab's bulk socket, a
     * `ParcelFileDescriptor`) and, once the core has booted, `coreEpoch`
     * (String). A tab opened while the core boots gets `core/state` frames
     * first and `onReset(coreEpoch, "core-ready")` when it is up.
     *
     * `callback` may be called before this returns.
     */
    Bundle openTab(String helloJson, IArgoTabCallback callback);

    /**
     * Where ARGO runs for this phone and how that stands, as JSON
     * (`CoreContract.EXECUTION_*`): the chosen mode and host, the link's
     * state and reason, and the mesh devices a host can be picked from.
     * Answers at once; `"{}"` to a caller that is not the app.
     *
     * Added after contract version 1, at the end of the interface so older
     * clients keep their transaction codes; a client calling it on an older
     * core gets an exception and treats it as "not offered".
     */
    String executionState();

    /**
     * Choose where ARGO runs: `mode` is one of `CoreContract.MODE_*`, `host`
     * the device name for `MODE_SELF_HOSTED` (otherwise ignored). Returns
     * true when the choice was accepted and kept; the core then rebuilds its
     * tree in the background (the process stays), so every open tab sees the
     * usual `core/state` booting and a new epoch. False for a caller that is
     * not the app or a choice that makes no sense.
     */
    boolean setExecution(String mode, String host);

    /**
     * How this phone's two syncs stand, as JSON (`CoreContract.SYNC_*`): the
     * main conversation's (mode, state, whether it is on, the host's
     * address) and memory sync's (whether it is on, the host's address,
     * whether the shared ring key is held). They are independent switches.
     * Answers at once; `"{}"` to a caller that is not the app.
     *
     * Added after `setExecution`, at the end of the interface, so older
     * clients keep their transaction codes.
     */
    String syncState();

    /**
     * Turn one sync on or off. `kind` is `CoreContract.SYNC_MAIN` or
     * `SYNC_MEMORY`; `address` the host's sync address (`host:port`, empty
     * keeps the one set); `ringKey` is memory sync's shared key as 64 hex
     * digits (empty keeps the one held; ignored for the main conversation).
     * True when the change was accepted and kept. Neither switch moves,
     * replaces or deletes this phone's own main session or its memories.
     */
    boolean setSync(String kind, boolean enabled, String address, String ringKey);

    /**
     * Wipe what memory sync holds (D3 of 2026-10-11): this phone's replica
     * only, or on every device when `everywhere`. The caller has confirmed it
     * with the person. Records marked device-local, or never synced, are left
     * alone. False when refused, off or not reachable. Added at the end, so
     * older clients keep their transaction codes.
     */
    boolean wipeMemory(boolean everywhere);

    /**
     * Leave the mesh (L1 of 2026-10-11): the host is told, the mesh key,
     * membership and sync settings are removed, and this phone is standalone
     * with its own main session and memories untouched. The caller has
     * confirmed it with the person. False when refused or not reachable.
     */
    boolean leaveMesh();

    /**
     * Keep the sign-in for cloud mode: the bearer token the tenant gateway
     * accepts (a Firebase ID token, or a pasted one), written owner-only under
     * the core's home where every attach and every exchange reads it; empty
     * signs out. The token is never in a state answer, a log or an error.
     * True when it was kept. If the cloud was chosen and not attached, the
     * core tries again at once.
     *
     * Added after `leaveMesh`, at the end of the interface, so older clients
     * keep their transaction codes.
     */
    boolean setGatewayToken(String token);

    /**
     * The sealed way back to one session of the ARGO app, for a client that is not the app and so
     * holds no key to the app's launch seal (the browser's "Open in ARGO"): an immutable
     * `PendingIntent` that starts the app's main Activity on `sessionId` and that the app honours
     * (`LaunchSeal`). The client sends it, with the background-start opt-in, and can neither read
     * nor change what is inside.
     *
     * Null to a caller that is not the browser, for a blank `sessionId`, or when the app could not
     * seal it; the client then only brings the app forward. Added after `setGatewayToken`, at the
     * end of the interface, so older clients keep their transaction codes; a client calling it on
     * an older core gets an exception and does the same.
     */
    PendingIntent openSessionIntent(String sessionId);

    /**
     * The LLM session broker's account, as JSON (`CoreContract.LLM_*`): whether
     * this build has a broker at all, who is signed in (an address to show),
     * and whether the relay's session is ready, being fetched, or failing and
     * why. Never a token. Answers at once; `"{}"` to a caller that is not the
     * app or a core with no broker.
     *
     * Added after `openSessionIntent`, at the end of the interface, so older
     * clients keep their transaction codes.
     */
    String llmAccount();

    /**
     * Sign in to the broker with a Google ID token (Credential Manager, in the
     * app) that the core trades for a Firebase sign-in, then asks the relay for
     * a session in the background. Blocks until Firebase has answered (bounded).
     * The token is a secret: never in a state answer, a log or an error. False
     * for a caller that is not the app, a core with no broker, or a refusal.
     */
    boolean llmSignIn(String googleIdToken);

    /** Sign out of the broker: the Firebase sign-in and the session in the core are dropped. False for a caller that is not the app or a core with no broker. */
    boolean llmSignOut();

    /**
     * The realtime model VIA uses, as JSON (`CoreContract.VOICE_MODEL_*`):
     * the chosen profile (`gpt` or `sr`) and whether this device holds the
     * SR connection fields. Never the fields themselves. Answers at once;
     * `"{}"` to a caller that is not the app.
     *
     * Added after `llmSignOut`, at the end of the interface, so older clients
     * keep their transaction codes.
     */
    String voiceModelState();

    /**
     * Choose the realtime model: `profile` is `CoreContract.VOICE_MODEL_GPT`
     * or `VOICE_MODEL_SR`. True when the choice was accepted and kept; the
     * core then restarts itself in the background (the process stays), so
     * every open tab sees the usual `core/state` booting and a new epoch.
     * False for a caller that is not the app, an unknown profile, or SR
     * without the connection fields on this device.
     */
    boolean setVoiceModel(String profile);

    /**
     * The signed-in person's current Firebase ID token, for the app's companion-apps download from
     * Firebase Storage (which only a signed-in person may read). Firebase Auth lives in this process
     * alone, so the app's main process asks here rather than reading the saved sign-in itself. A
     * secret: never in a state answer, a log or an error. Null for a caller that is not the app (the
     * browser never gets it), a core with no broker, nobody signed in, or a token that could not be
     * had in a short while. Blocking (bounded).
     *
     * Added after `setVoiceModel`, at the end of the interface, so older clients keep their
     * transaction codes; a client calling it on an older core gets an exception and sees null.
     */
    String companionDownloadToken();
}
