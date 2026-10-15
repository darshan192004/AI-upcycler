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

    fun start() {
        if (listener != null) return
        val snapshot = metrics.snapshot()
        val serviceInfo = NsdServiceInfo().apply {
            serviceName = instanceName
            serviceType = WorkerProtocol.MDNS_SERVICE_TYPE
            port = WorkerProtocol.RPC_PORT
            WorkerProtocol.txtRecords(snapshot.totalRamBytes, snapshot.freeRamBytes)
                .forEach { (key, value) -> setAttribute(key, value) }
        }

        val nsdManager = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        val registrationListener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) {
                Log.i(TAG, "mDNS: registered ${info.serviceName}")
            }

            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "mDNS: registration failed (error $errorCode)")
            }

            override fun onServiceUnregistered(info: NsdServiceInfo) {
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
