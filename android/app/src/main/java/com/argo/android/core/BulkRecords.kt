package com.argo.android.core

import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * The bulk socket's record format, both directions (§2.5 "Large frames").
 *
 * A frame too large for a batch (from `:core`) or for an inline `send` (from
 * a client) travels on the tab's socket pair instead, as one record:
 *
 * ```
 * id      8 bytes, big-endian: the id the placeholder or `sendBulk` names
 * length  4 bytes, big-endian: the frame's UTF-8 byte count
 * frame   that many bytes of UTF-8 JSON text
 * ```
 *
 * No framing beyond that: the socket is a stream between two ends of one
 * app, and a record that does not parse is a broken peer, which closes the
 * tab. Ids rise within a tab in each direction, so a reader discards any
 * record below the id it is waiting for.
 */
object BulkRecords {
    /**
     * The largest frame either end accepts: a 10 MiB `web/attach` is about
     * 14 MB of base64, and a transcript can be large too. Past this a record
     * is a broken or hostile peer rather than a frame, and reading it would
     * only exhaust the heap.
     */
    const val MAX_FRAME_BYTES = 64 * 1024 * 1024

    /** Write one record and flush it. Blocks while the peer is not reading. */
    fun write(out: OutputStream, id: Long, frame: String) {
        val bytes = frame.toByteArray(Charsets.UTF_8)
        require(bytes.size <= MAX_FRAME_BYTES) { "bulk frame $id is ${bytes.size} bytes, over $MAX_FRAME_BYTES" }
        val data = DataOutputStream(out)
        data.writeLong(id)
        data.writeInt(bytes.size)
        data.write(bytes)
        data.flush()
    }

    /** One record read back. */
    data class Record(val id: Long, val frame: String)

    /**
     * Read the next record, or `null` at a clean end of stream (the peer
     * closed between records).
     *
     * @throws IOException a record cut short, or one claiming a length over
     *   [MAX_FRAME_BYTES] or under zero.
     */
    fun read(input: DataInputStream): Record? {
        val id = try {
            input.readLong()
        } catch (_: EOFException) {
            return null
        }
        val length = input.readInt()
        if (length < 0 || length > MAX_FRAME_BYTES) throw IOException("bulk record $id claims $length bytes")
        val bytes = ByteArray(length)
        input.readFully(bytes)
        return Record(id, String(bytes, Charsets.UTF_8))
    }
}

/**
 * The records one end has read, waited on by id.
 *
 * A reader thread ([run]) reads the stream into here as records arrive; the
 * code that needs one — the tab's inbound queue for a `sendBulk`, the
 * client's batch assembler for a placeholder — [take]s it by id. The writer
 * on the far side writes a record before it names it, but nothing orders the
 * stream against the Binder call that names it, so a taker may well wait.
 */
class BulkInbox(private val input: InputStream) {
    private val lock = ReentrantLock()
    private val arrived = lock.newCondition()
    private val records = HashMap<Long, String>()

    /** Set once the stream ended, cleanly or not: nothing more will arrive. */
    private var ended: Throwable? = null
    private var endedCleanly = false

    /**
     * Read records until the stream ends. Run on a thread of its own; it
     * returns when the far end closes or this end's stream is closed under it.
     */
    fun run() {
        val data = DataInputStream(input)
        try {
            while (true) {
                val record = BulkRecords.read(data) ?: break
                lock.withLock {
                    records[record.id] = record.frame
                    arrived.signalAll()
                }
            }
            lock.withLock {
                endedCleanly = true
                arrived.signalAll()
            }
        } catch (error: IOException) {
            lock.withLock {
                ended = error
                arrived.signalAll()
            }
        }
    }

    /**
     * Record [id], waiting up to [timeoutMs] for it. Records with a lower id
     * still held are dropped: the ids rise, so whatever named them is past.
     *
     * @throws IOException the stream ended without it, or it did not arrive
     *   in time.
     */
    fun take(id: Long, timeoutMs: Long): String = lock.withLock {
        var left = TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        var frame = records.remove(id)
        while (frame == null) {
            ended?.let { throw IOException("the bulk socket failed before record $id", it) }
            if (endedCleanly) throw IOException("the bulk socket closed before record $id")
            if (left <= 0) throw IOException("bulk record $id did not arrive within ${timeoutMs}ms")
            left = arrived.awaitNanos(left)
            frame = records.remove(id)
        }
        records.keys.removeAll { it < id }
        frame
    }

    /** How many records are held, for a test. */
    internal fun held(): Int = lock.withLock { records.size }
}
