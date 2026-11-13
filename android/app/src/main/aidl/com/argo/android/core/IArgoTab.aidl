package com.argo.android.core;

/**
 * One hub tab, held by the client that opened it (§2.5). Frames are the JSON
 * text a WebSocket tab carries, plus the core's own `core/*` namespace.
 *
 * Every method is oneway, and every call is checked against the uid that
 * opened the tab: a handle that reaches another process does nothing there.
 */
interface IArgoTab {
    /**
     * One request frame, at most `CoreContract.INLINE_FRAME_BYTES` of UTF-8.
     * The reply comes back as a frame in a batch.
     */
    oneway void send(String frameJson);

    /**
     * One request frame too large for `send`, already written to the bulk
     * socket as record `bulkId` (`BulkRecords`). Ordered with `send` as the
     * calls arrive.
     */
    oneway void sendBulk(long bulkId);

    /** Every batch up to and including `seq` has been handled. Cumulative. */
    oneway void ack(long seq);

    /** Every batch up to `lastSeq` was handled and the next one is missing. */
    oneway void resume(long lastSeq);

    /** Detach this tab. Parked asks stay parked; `:core` keeps running. */
    oneway void close();
}
