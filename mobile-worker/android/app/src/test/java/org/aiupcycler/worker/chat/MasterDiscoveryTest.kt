package org.aiupcycler.worker.chat

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Device-free validation of the master advert: no NsdManager, no device, no
 * network. Only public TXT metadata and the credential fingerprint are used.
 */
class MasterDiscoveryTest {

    @Test
    fun ignoresServicesWhoseTxtVersionIsNotOne() {
        val recorder = Recorder()
        val discovery = discovery(recorder)

        discovery.onResolved(
            host = "192.168.1.20",
            port = 8090,
            txt = txt(version = "2", fingerprint = FINGERPRINT, apiPort = "8090"),
        )

        assertTrue(recorder.events.isEmpty())
    }

    @Test
    fun rejectsFingerprintMismatchWithExactReason() {
        val recorder = Recorder()
        val discovery = discovery(recorder)

        discovery.onResolved(
            host = "192.168.1.20",
            port = 8090,
            txt = txt(version = "1", fingerprint = "fp-other-test", apiPort = "8090"),
        )

        assertEquals(listOf(DiscoveryFailure("fingerprint mismatch")), recorder.events)
    }

    @Test
    fun acceptsMatchingFingerprintAndUsesValidTxtApiPort() {
        val recorder = Recorder()
        val discovery = discovery(recorder)

        discovery.onResolved(
            // The SRV port is deliberately different: TXT api_port is authoritative.
            host = "192.168.1.20",
            port = 9999,
            txt = txt(version = "1", fingerprint = FINGERPRINT, apiPort = "8091"),
        )

        assertEquals(listOf(MasterEndpoint("192.168.1.20", 8091)), recorder.events)
    }

    @Test
    fun rejectsMissingAndInvalidApiPortWithExactReasons() {
        val missing = Recorder()
        discovery(missing).onResolved(
            host = "192.168.1.20",
            port = 8090,
            txt = mapOf("version" to "1", "fingerprint" to FINGERPRINT),
        )
        assertEquals(listOf(DiscoveryFailure("missing api_port")), missing.events)

        val invalid = Recorder()
        discovery(invalid).onResolved(
            host = "192.168.1.20",
            port = 8090,
            txt = txt(version = "1", fingerprint = FINGERPRINT, apiPort = "70000"),
        )
        assertEquals(listOf(DiscoveryFailure("invalid api_port")), invalid.events)
    }

    @Test
    fun manualHostPortBypassesNsdButStillUsesEndpointValidation() {
        val browser = FakeBrowser()
        val discovery = MasterDiscovery(
            browser = browser,
            expectedFingerprint = { FINGERPRINT },
            onFound = {},
            onFailure = {},
        )

        assertEquals(
            MasterEndpoint("192.168.1.20", 8090),
            MasterDiscovery.manual("192.168.1.20:8090"),
        )
        assertFalse(browser.started)

        val error = runCatching { MasterDiscovery.manual("user:pass@192.168.1.20:8090") }
            .exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals("LAN endpoint must not contain userinfo", error?.message)
    }

    @Test
    fun startsTheExactServiceTypeAndReportsBrowserErrorsAsFailures() {
        val browser = FakeBrowser()
        val recorder = Recorder()
        val discovery = MasterDiscovery(browser, { FINGERPRINT }, recorder::found, recorder::failed)

        discovery.start()

        assertEquals("_aiupcycler-master._tcp.", browser.serviceType)
        assertTrue(browser.started)

        discovery.onError("discovery failed")
        assertEquals(listOf(DiscoveryFailure("discovery failed")), recorder.events)

        discovery.stop()
        assertTrue(browser.stopped)
    }

    private fun discovery(recorder: Recorder): MasterDiscovery = MasterDiscovery(
        browser = FakeBrowser(),
        expectedFingerprint = { FINGERPRINT },
        onFound = recorder::found,
        onFailure = recorder::failed,
    )

    private fun txt(version: String, fingerprint: String, apiPort: String?): Map<String, String> =
        buildMap {
            put("version", version)
            put("fingerprint", fingerprint)
            if (apiPort != null) put("api_port", apiPort)
        }

    private class Recorder {
        val events = mutableListOf<Any>()

        fun found(endpoint: MasterEndpoint) {
            events += endpoint
        }

        fun failed(failure: DiscoveryFailure) {
            events += failure
        }
    }

    private class FakeBrowser : MasterDiscoveryBrowser {
        var started = false
        var stopped = false
        var serviceType: String? = null
        var listener: MasterDiscoveryBrowser.Listener? = null

        override fun start(serviceType: String, listener: MasterDiscoveryBrowser.Listener) {
            this.serviceType = serviceType
            this.listener = listener
            started = true
        }

        override fun stop() {
            stopped = true
        }
    }

    private companion object {
        const val FINGERPRINT = "fp-test-a1b2c3"
    }
}
