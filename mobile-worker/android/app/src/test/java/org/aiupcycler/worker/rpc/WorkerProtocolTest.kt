package org.aiupcycler.worker.rpc

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Guards the worker against silently drifting from the master's wire contract
 * (`core/src/rpc/client.rs`, `core/src/discovery/mdns.rs`).
 */
class WorkerProtocolTest {

    @Test
    fun rpcPortMatchesMasterAdbForwardPort() {
        assertEquals(50052, WorkerProtocol.RPC_PORT)
    }

    @Test
    fun protocolVersionMatchesMaster() {
        assertEquals("1.0", WorkerProtocol.PROTOCOL_VERSION)
    }

    @Test
    fun txtRecordsUseTheKeysTheMasterParses() {
        val records = WorkerProtocol.txtRecords(
            totalRamBytes = 8_589_934_592,
            freeRamBytes = 4_294_967_296,
        )

        assertEquals("1.0", records["protocol_version"])
        assertEquals("8589934592", records["total_ram_bytes"])
        assertEquals("4294967296", records["free_ram_bytes"])
    }

    @Test
    fun txtRecordsClampUnavailableRamToZero() {
        val records = WorkerProtocol.txtRecords(totalRamBytes = -1, freeRamBytes = -5)

        assertEquals("0", records["total_ram_bytes"])
        assertEquals("0", records["free_ram_bytes"])
    }
}
