package org.aiupcycler.worker.rpc

/**
 * Wire constants shared with the master orchestrator.
 *
 * Mirrors the master's own definitions so the two ends cannot silently drift:
 * `core/src/rpc/client.rs` (`MASTER_ID`, `PROTOCOL_VERSION`, forward port) and
 * `core/src/discovery/mdns.rs` (TXT record keys + service type).
 */
object WorkerProtocol {
    /**
     * gRPC port the worker serves `aiupcycler.rpc.WorkerService` on. Matches the
     * master's `ADB_FORWARD_PORT` so `adb forward tcp:50052 tcp:50052` reaches it.
     */
    const val RPC_PORT: Int = 50052

    /** Version this build speaks during `Handshake`. */
    const val PROTOCOL_VERSION: String = "1.0"

    /** Identity the master presents on handshake (logged for traceability). */
    const val MASTER_ID: String = "ai-upcycler-master"

    /**
     * mDNS service type as consumed by Android's `NsdManager`; the trailing
     * `.local.` the master uses (`_aiupcycler-worker._tcp.local.`) is implied.
     */
    const val MDNS_SERVICE_TYPE: String = "_aiupcycler-worker._tcp."

    const val TXT_PROTOCOL_VERSION: String = "protocol_version"
    const val TXT_TOTAL_RAM_BYTES: String = "total_ram_bytes"
    const val TXT_FREE_RAM_BYTES: String = "free_ram_bytes"

    /**
     * Builds the TXT records advertised alongside the mDNS service.
     *
     * The master reads these keys in `DiscoveredNode::from_service_info`; values
     * are decimal strings because mDNS TXT records are byte strings. Negative
     * readings (an unavailable metric) clamp to `0`.
     */
    fun txtRecords(
        totalRamBytes: Long,
        freeRamBytes: Long,
        protocolVersion: String = PROTOCOL_VERSION,
    ): Map<String, String> = mapOf(
        TXT_PROTOCOL_VERSION to protocolVersion,
        TXT_TOTAL_RAM_BYTES to totalRamBytes.coerceAtLeast(0L).toString(),
        TXT_FREE_RAM_BYTES to freeRamBytes.coerceAtLeast(0L).toString(),
    )
}
