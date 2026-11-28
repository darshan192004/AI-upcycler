package org.aiupcycler.worker.chat

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Device-free contract for the LAN URL boundary. A host-aware builder must
 * bracket raw IPv6 literals while leaving IPv4 and DNS names untouched.
 * Every address below is documentation/synthetic; no real host is contacted.
 */
class LanEndpointTest {

    @Test
    fun baseUrlBracketsRawIpv6Host() {
        assertEquals(
            "http://[2001:db8::1]:8080",
            LanEndpoint(host = "2001:db8::1", port = 8080).baseUrl(),
        )
    }

    @Test
    fun baseUrlPreservesIpv4AndDnsHosts() {
        assertEquals(
            "http://192.168.1.20:8080",
            LanEndpoint(host = "192.168.1.20", port = 8080).baseUrl(),
        )
        assertEquals(
            "http://master.local:8080",
            LanEndpoint(host = "master.local", port = 8080).baseUrl(),
        )
    }

    @Test
    fun parsedIpv6EndpointStillProducesBracketedUrl() {
        val endpoint = LanEndpoint.parse("2001:db8::1:8080")

        assertEquals("2001:db8::1", endpoint.host)
        assertEquals(8080, endpoint.port)
        assertEquals("http://[2001:db8::1]:8080", endpoint.baseUrl())
    }

    @Test
    fun invalidHostsAreRejectedBeforeAUrlIsBuilt() {
        for (host in listOf("bad host", "bad_host")) {
            val error = runCatching { LanEndpoint(host, 8080) }.exceptionOrNull()
            assertEquals("LAN endpoint host is invalid", error?.message)
        }
    }
}
