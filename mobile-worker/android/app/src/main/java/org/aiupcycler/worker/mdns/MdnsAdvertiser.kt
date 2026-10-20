package org.aiupcycler.worker.mdns

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.util.Log
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.aiupcycler.worker.rpc.WorkerProtocol

/**
 * Advertises this node as `_aiupcycler-worker._tcp` over DNS-SD so the master's
 * mDNS browser discovers it without any manual pairing.
 */
class MdnsAdvertiser(
    private val context: Context,
    private val metrics: DeviceMetrics,
    private val instanceName: String,
) {
    private var listener: NsdManager.RegistrationListener? = null
    private var registeredName: String? = null

    /**
     * The DNS-SD instance fullname the master's browser reports as this node's
     * `node_id` - the exact string `core::discovery::mdns::DiscoveredNode` keeps
     * and `EnrolledWorkerSource` keys credentials by.
     *
     * Before NSD confirms registration this is the *requested* name; Android may
     * rename the instance on a collision, so after [start] the
     * [onServiceRegistered][NsdManager.RegistrationListener.onServiceRegistered]
     * callback's actual name wins. Enrollment has to declare this value, which
     * is why pairing must not run before registration resolves (spec D-B6).
     */
    fun nodeId(): String = NodeIds.fullname(registeredName ?: instanceName)

    /** Whether NSD has confirmed the advertised instance name. */
    fun isRegistered(): Boolean = registeredName != null

    fun start() {
        if (listener != null) return
        val snapshot = metrics.snapshot()
        val serviceInfo = NsdServiceInfo().apply {
            serviceName = instanceName
            serviceType = NodeIds.SERVICE_TYPE
            port = WorkerProtocol.RPC_PORT
            WorkerProtocol.txtRecords(snapshot.totalRamBytes, snapshot.freeRamBytes)
                .forEach { (key, value) -> setAttribute(key, value) }
        }

        val nsdManager = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        val registrationListener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) {
                // May differ from the requested name: NSD renames on collision.
                registeredName = info.serviceName
                Log.i(TAG, "mDNS: registered ${info.serviceName} (node_id ${nodeId()})")
            }

            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "mDNS: registration failed (error $errorCode)")
            }

            override fun onServiceUnregistered(info: NsdServiceInfo) {
                registeredName = null
                Log.i(TAG, "mDNS: unregistered ${info.serviceName}")
            }

            override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "mDNS: unregistration failed (error $errorCode)")
            }
        }
        nsdManager.registerService(serviceInfo, NsdManager.PROTOCOL_DNS_SD, registrationListener)
        listener = registrationListener
    }

    fun stop() {
        val registered = listener ?: return
        listener = null
        registeredName = null
        val nsdManager = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        // `unregisterService` throws IllegalArgumentException when the
        // registration never completed (or already failed); swallow teardown
        // races so destroying the activity can't crash the process.
        runCatching { nsdManager.unregisterService(registered) }
            .onFailure { Log.w(TAG, "mDNS: unregister failed: ${it.message}") }
    }

    private companion object {
        const val TAG = "MdnsAdvertiser"
    }
}
