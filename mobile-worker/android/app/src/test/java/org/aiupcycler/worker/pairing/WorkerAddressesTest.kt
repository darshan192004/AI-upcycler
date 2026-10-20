package org.aiupcycler.worker.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The declared-address rule (spec D-B7). Declared addresses become the SANs of
 * the issued server certificate, so declaring the wrong ones is not a cosmetic
 * bug: the master's `load_secure` verifies the certificate against the address
 * it dialed, and a SAN that is not ours would assert an identity this device
 * does not hold.
 */
class WorkerAddressesTest {

    @Test
    fun declaresLanAddressesThenTheLoopbackPair() {
        assertEquals(
            listOf("192.168.1.50", "127.0.0.1", "localhost"),
            WorkerAddresses.declared(
                lanAddresses = listOf("192.168.1.50"),
                isOnLan = true,
                isCaptivePortal = false,
            ),
        )
    }

    @Test
    fun omitsLanAddressesOnACaptivePortal() {
        // A captive portal's address belongs to the router that is holding the
        // connection, not to this device; baking it into a certificate would
        // declare an identity we do not hold.
        assertEquals(
            WorkerAddresses.LOOPBACK_ADDRESSES,
            WorkerAddresses.declared(
                lanAddresses = listOf("10.0.0.1"),
                isOnLan = true,
                isCaptivePortal = true,
            ),
        )
    }

    @Test
    fun omitsLanAddressesWhenNotOnALan() {
        assertEquals(
            WorkerAddresses.LOOPBACK_ADDRESSES,
            WorkerAddresses.declared(
                lanAddresses = listOf("192.168.1.50"),
                isOnLan = false,
                isCaptivePortal = false,
            ),
        )
    }

    @Test
    fun alwaysKeepsTheLoopbackPairForTheAdbTunnel() {
        // The tunnel path dials 127.0.0.1, so the loopback pair can never be
        // dropped - an empty declared list is a contract error anyway.
        val declared = WorkerAddresses.declared(emptyList(), isOnLan = false, isCaptivePortal = false)
        assertEquals(listOf("127.0.0.1", "localhost"), declared)
        assertTrue(declared.isNotEmpty())
    }

    @Test
    fun deduplicatesAndCapsAtTheMastersLimit() {
        val many = (0 until 12).map { "192.168.1.$it" } + "127.0.0.1"
        val declared = WorkerAddresses.declared(many, isOnLan = true, isCaptivePortal = false)
        assertEquals(WorkerAddresses.MAX_ADDRESSES, declared.size)
        assertEquals(declared.distinct(), declared)
    }

    @Test
    fun capMirrorsTheMastersConstant() {
        // `MAX_ADDRESSES` in core/src/api/pairing.rs: one enrollment must not
        // mint an unbounded SAN list.
        assertEquals(8, WorkerAddresses.MAX_ADDRESSES)
    }
}
