package org.aiupcycler.worker.pairing

import java.net.Inet4Address
import java.net.NetworkInterface

/**
 * Decides which addresses this worker declares at enrollment (spec D-B7).
 *
 * The declared list becomes the subject alternative names of the issued server
 * certificate, and the master's `load_secure` verifies the certificate against
 * the address it dialed. Two rules follow from that:
 *
 * - The loopback pair is always included: the master dials workers registered
 *   over USB at the ADB-forwarded `127.0.0.1`, and the server's own default
 *   address list is that same pair (`DEFAULT_ADDRESSES` in `api/pairing.rs`).
 * - LAN addresses are only declared while this worker is *still* on the same
 *   network the master reached it on. On a captive portal the router's address
 *   would otherwise be baked into a certificate as if it were this device -
 *   a server identity that is not ours.
 *
 * The master's `MAX_ADDRESSES` cap is mirrored here so an over-long list is
 * trimmed deliberately rather than rejected as a whole.
 */
object WorkerAddresses {

    /** Mirrors `MAX_ADDRESSES` in `core/src/api/pairing.rs`. */
    const val MAX_ADDRESSES = 8

    /** Mirrors `DEFAULT_ADDRESSES`: the ADB-tunnel loopback pair. */
    val LOOPBACK_ADDRESSES = listOf("127.0.0.1", "localhost")

    /**
     * Builds the declared list: LAN addresses (when [isOnLan] and
     * [isCaptivePortal] allow them), then the loopback pair, deduplicated and
     * capped at [MAX_ADDRESSES].
     */
    fun declared(
        lanAddresses: List<String>,
        isOnLan: Boolean,
        isCaptivePortal: Boolean,
    ): List<String> {
        val usableLan = if (isOnLan && !isCaptivePortal) lanAddresses else emptyList()
        return (usableLan + LOOPBACK_ADDRESSES)
            .filter { it.isNotBlank() }
            .distinct()
            .take(MAX_ADDRESSES)
    }

    /**
     * The device's own IPv4 addresses on non-loopback interfaces - what the
     * master dials over Wi-Fi. Android needs no permission to enumerate these.
     */
    fun localIpv4Addresses(
        interfaces: List<NetworkInterface> = NetworkInterface.getNetworkInterfaces()?.toList()
            ?: emptyList(),
    ): List<String> = interfaces
        .filter { interfaceIsUsable(it) }
        .flatMap { it.inetAddresses.toList() }
        .filterIsInstance<Inet4Address>()
        .filter { !it.isLoopbackAddress }
        .mapNotNull { it.hostAddress }

    private fun interfaceIsUsable(interface0: NetworkInterface): Boolean = try {
        interface0.isUp && !interface0.isLoopback && !interface0.isPointToPoint
    } catch (e: Exception) {
        // A torn-down interface throws on these getters; it has no address to
        // declare either way.
        false
    }
}
