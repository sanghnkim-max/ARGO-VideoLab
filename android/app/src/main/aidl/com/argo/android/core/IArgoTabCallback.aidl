package com.argo.android.core;

/**
 * Implemented by the client: where one tab's frames arrive (§2.5).
 */
oneway interface IArgoTabCallback {
    /**
     * One batch: `seq` is 1 for a tab's first batch and one more for each
     * after it, rising across a reset too. `framesJson` is the text of one
     * JSON array, each element a frame or `{"bulk":{"id":N,"len":L}}` standing
     * in for record N on the bulk socket. A retried or replayed batch carries
     * the same `seq` and text: deduplicate by `seq`, and call
     * `IArgoTab.resume` on a gap.
     */
    void onBatch(long seq, String framesJson);

    /**
     * Replay is impossible, or the core has just come up: fail every request
     * in flight and read state again from the batches that follow, which
     * begin with the hub's `ready` frame. `coreEpoch` changes when `:core`
     * restarted.
     */
    void onReset(String coreEpoch, String reason);

    /** The tab is closed; nothing more arrives on it. */
    void onClosed(String reason);
}
