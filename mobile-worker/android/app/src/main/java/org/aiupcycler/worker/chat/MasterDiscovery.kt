package org.aiupcycler.worker.chat

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo

/** The endpoint advertised by the host's opt-in LAN listener. */
data class MasterEndpoint(val host: String, val port: Int)

/** A rejected advert or browser error, with the exact reason shown to the UI. */
data class DiscoveryFailure(val reason: String)

/** Browser boundary; NsdManager lives only in [NsdMasterDiscoveryBrowser]. */
interface MasterDiscoveryBrowser {
    fun start(serviceType: String, listener: Listener)
    fun stop()

    interface Listener {
        fun onResolved(host: String, port: Int, txt: Map<String, String?>)
        fun onLost()
        fun onError(reason: String)
    }
}

sealed interface DiscoveryOutcome {
    data class Found(val endpoint: MasterEndpoint) : DiscoveryOutcome
    data class Failed(val failure: DiscoveryFailure) : DiscoveryOutcome
    object Ignored : DiscoveryOutcome
}

/**
 * Validates the public TXT records of `_aiupcycler-master._tcp.` against the
 * fingerprint already verified at pairing. The token is never advertised and
 * never accepted from an advert. Browser access is behind an interface, so all
 * validation is exercised on the JVM.
 */
class MasterDiscovery(
    private val browser: MasterDiscoveryBrowser,
    private val expectedFingerprint: () -> String?,
    private val onFound: (MasterEndpoint) -> Unit,
    private val onFailure: (DiscoveryFailure) -> Unit,
) {
    fun start() {
        browser.start(SERVICE_TYPE, object : MasterDiscoveryBrowser.Listener {
            override fun onResolved(host: String, port: Int, txt: Map<String, String?>) {
                this@MasterDiscovery.onResolved(host, port, txt)
            }

            override fun onLost() {
                // A lost advert is not a failure: the UI keeps its last endpoint.
            }

            override fun onError(reason: String) {
                this@MasterDiscovery.onError(reason)
            }
        })
    }

    fun stop() {
        browser.stop()
    }

    /** The SRV port is ignored: the TXT `api_port` is authoritative. */
    @Suppress("UNUSED_PARAMETER")
    fun onResolved(host: String, port: Int, txt: Map<String, String?>) {
        when (val outcome = resolve(host, txt, expectedFingerprint())) {
            is DiscoveryOutcome.Found -> onFound(outcome.endpoint)
            is DiscoveryOutcome.Failed -> onFailure(outcome.failure)
            DiscoveryOutcome.Ignored -> Unit
        }
    }

    fun onError(reason: String) {
        onFailure(DiscoveryFailure(reason))
    }

    companion object {
        const val SERVICE_TYPE = "_aiupcycler-master._tcp."
        const val TXT_VERSION = "version"
        const val TXT_FINGERPRINT = "fingerprint"
        const val TXT_API_PORT = "api_port"

        const val REASON_FINGERPRINT_MISMATCH = "fingerprint mismatch"
        const val REASON_MISSING_FINGERPRINT = "missing fingerprint"
        const val REASON_MISSING_STORED_FINGERPRINT = "missing stored fingerprint"
        const val REASON_MISSING_API_PORT = "missing api_port"
        const val REASON_INVALID_API_PORT = "invalid api_port"
        const val REASON_INVALID_HOST = "invalid host"

        /** Manual entry skips NSD entirely but uses exactly the same host:port parser. */
        fun manual(input: String): MasterEndpoint {
            val endpoint = LanEndpoint.parse(input)
            return MasterEndpoint(endpoint.host, endpoint.port)
        }

        fun resolve(
            host: String,
            txt: Map<String, String?>,
            expectedFingerprint: String?,
        ): DiscoveryOutcome {
            if (txt[TXT_VERSION] != "1") return DiscoveryOutcome.Ignored

            if (expectedFingerprint.isNullOrBlank()) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_MISSING_STORED_FINGERPRINT))
            }
            val advertisedFingerprint = txt[TXT_FINGERPRINT]?.trim()
            if (advertisedFingerprint.isNullOrBlank()) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_MISSING_FINGERPRINT))
            }
            if (advertisedFingerprint != expectedFingerprint) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_FINGERPRINT_MISMATCH))
            }

            val apiPortText = txt[TXT_API_PORT]?.trim()
            if (apiPortText.isNullOrBlank()) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_MISSING_API_PORT))
            }
            val apiPort = apiPortText.toIntOrNull()
                ?: return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_INVALID_API_PORT))
            if (apiPort !in 1..65535) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_INVALID_API_PORT))
            }

            val endpoint = try {
                LanEndpoint.parse("$host:$apiPort")
            } catch (_: IllegalArgumentException) {
                return DiscoveryOutcome.Failed(DiscoveryFailure(REASON_INVALID_HOST))
            }
            return DiscoveryOutcome.Found(MasterEndpoint(endpoint.host, endpoint.port))
        }
    }
}

/** The only Android-facing piece: NsdManager is fully behind the interface. */
class NsdMasterDiscoveryBrowser(private val context: Context) : MasterDiscoveryBrowser {
    private var manager: NsdManager? = null
    private var discoveryListener: NsdManager.DiscoveryListener? = null

    override fun start(serviceType: String, listener: MasterDiscoveryBrowser.Listener) {
        val nsd = context.applicationContext.getSystemService(Context.NSD_SERVICE) as NsdManager
        manager = nsd
        val platformListener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(regType: String?) = Unit

            override fun onStartDiscoveryFailed(serviceType: String?, errorCode: Int) {
                listener.onError("start discovery failed: $errorCode")
            }

            override fun onDiscoveryStopped(serviceType: String?) = Unit

            override fun onStopDiscoveryFailed(serviceType: String?, errorCode: Int) {
                listener.onError("stop discovery failed: $errorCode")
            }

            override fun onServiceFound(serviceInfo: NsdServiceInfo?) {
                if (serviceInfo == null) return
                nsd.resolveService(serviceInfo, object : NsdManager.ResolveListener {
                    override fun onResolveFailed(info: NsdServiceInfo?, errorCode: Int) {
                        listener.onError("resolve failed: $errorCode")
                    }

                    override fun onServiceResolved(info: NsdServiceInfo?) {
                        if (info == null) return
                        val host = info.host?.hostAddress
                        if (host.isNullOrBlank()) {
                            listener.onError(MasterDiscovery.REASON_INVALID_HOST)
                            return
                        }
                        val txt = info.attributes.mapValues { (_, value) ->
                            value?.toString(Charsets.UTF_8)
                        }
                        listener.onResolved(host, info.port, txt)
                    }
                })
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo?) = Unit
        }
        discoveryListener = platformListener
        nsd.discoverServices(serviceType, NsdManager.PROTOCOL_DNS_SD, platformListener)
    }

    override fun stop() {
        val nsd = manager ?: return
        val listener = discoveryListener ?: return
        try {
            nsd.stopServiceDiscovery(listener)
        } catch (_: IllegalArgumentException) {
            // Discovery was already stopped; no user-visible failure.
        }
        discoveryListener = null
    }
}
