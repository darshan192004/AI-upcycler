package org.aiupcycler.worker

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.util.Log
import org.aiupcycler.worker.identity.WorkerIdentity
import org.aiupcycler.worker.mdns.MdnsAdvertiser
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.aiupcycler.worker.pairing.CredentialStore
import org.aiupcycler.worker.pairing.EnrollmentClient
import org.aiupcycler.worker.pairing.EnrollmentRequest
import org.aiupcycler.worker.pairing.EnrollmentResult
import org.aiupcycler.worker.pairing.PairingState
import org.aiupcycler.worker.pairing.PairingUri
import org.aiupcycler.worker.pairing.WorkerAddresses
import org.aiupcycler.worker.pairing.WorkerCredentials
import org.aiupcycler.worker.rpc.WorkerProtocol
import org.aiupcycler.worker.rpc.WorkerRpcServer

/**
 * Owns the two long-lived worker services - the gRPC server and the mDNS
 * advertisement - so the UI and any future foreground service share one entry
 * point.
 */
class WorkerRuntime(private val context: Context) {
    private val identity = WorkerIdentity.from(context)
    private val metrics = DeviceMetrics(context)
    private val rpcServer = WorkerRpcServer(identity, metrics)
    private val advertiser =
        MdnsAdvertiser(context, metrics, "AI Upcycler Worker ${identity.deviceName}")
    private val credentials = CredentialStore(context.filesDir.resolve(PAIRING_DIR))

    val workerId: String get() = identity.workerId
    val deviceName: String get() = identity.deviceName
    val port: Int get() = WorkerProtocol.RPC_PORT

    /** The stored pairing, or `null` when this worker has never paired. */
    fun credentials(): WorkerCredentials? = credentials.load()

    /**
     * The pairing state the screen should show now, derived from what is on
     * disk (spec D-B4). A stored pairing whose fingerprint file is missing
     * still counts as paired - [CredentialStore.hasCredentials] decides that -
     * and the fingerprint is simply shown as unknown rather than the worker
     * pretending not to be paired.
     */
    fun pairingState(): PairingState = if (credentials.hasCredentials()) {
        PairingState.Paired(credentials.load()?.fingerprint.orEmpty())
    } else {
        PairingState.Unpaired
    }

    /**
     * [result] as the screen should show it: a successful enrollment is a
     * [PairingState.Paired] read back *from the store*, not from the response,
     * so what the screen shows is what the next start() will actually serve.
     */
    fun pairingState(result: EnrollmentResult): PairingState = when (result) {
        is EnrollmentResult.Enrolled -> pairingState()
        is EnrollmentResult.Rejected -> PairingState.Rejected(result.reason)
        is EnrollmentResult.Failed -> PairingState.Failed(result.reason)
    }

    fun start() {
        rpcServer.start(credentials.load())
        advertiser.start()
        Log.i(
            TAG,
            "worker $workerId serving gRPC on 0.0.0.0:$port" +
                if (rpcServer.isTls) " (mTLS)" else " (plaintext ADB path)",
        )
    }

    fun stop() {
        advertiser.stop()
        rpcServer.stop()
    }

    /**
     * Redeems [pin]/[fingerprint] against the master's pairing-only listener
     * and, on success, stores the issued credentials (spec D-B3/D-B4).
     *
     * [fingerprint] is the full value from a scanned/pasted payload, or the
     * leading characters the user typed; [fingerprintIsComplete] says which.
     *
     * Runs on the calling thread - the pairing screen calls it off the main
     * thread and posts the result back.
     */
    fun pair(
        pin: String,
        fingerprint: String,
        fingerprintIsComplete: Boolean,
    ): EnrollmentResult {
        val pairing = try {
            if (fingerprintIsComplete) {
                PairingUri.parse("$PAYLOAD_PREFIX$fingerprint&token=$pin")
            } else {
                PairingUri.parseManual(pin, fingerprint)
            }
        } catch (e: Exception) {
            return EnrollmentResult.Failed("Invalid pairing code: ${e.message}")
        }
        if (pairing is PairingUri.Result.Invalid) {
            return EnrollmentResult.Failed(pairing.reason)
        }
        val code = pairing as PairingUri.Result.Ok

        val request = if (fingerprintIsComplete) {
            EnrollmentRequest.scanned(
                enrollUrl = ENROLL_URL,
                pairing = code,
                deviceName = identity.deviceName,
                nodeId = nodeId(),
                addresses = declaredAddresses(),
            )
        } else {
            EnrollmentRequest.manual(
                enrollUrl = ENROLL_URL,
                pairing = code,
                deviceName = identity.deviceName,
                nodeId = nodeId(),
                addresses = declaredAddresses(),
            )
        }

        val result = EnrollmentClient().enroll(request)
        if (result is EnrollmentResult.Enrolled) {
            try {
                credentials.save(result.credentials)
                // Only now, with the credentials on disk, does the port stop
                // accepting plaintext (spec D-B5). A failure to restart leaves
                // the worker serving plaintext rather than half-configured, and
                // says so - the next start() picks TLS up from the store.
                restartServing()
                Log.i(TAG, "paired with the master; credentials stored, serving mTLS")
            } catch (e: Exception) {
                return EnrollmentResult.Failed("Could not store credentials: ${e.message}")
            }
        }
        return result
    }

    /** Re-reads the pairing and re-starts the server in the mode it implies. */
    private fun restartServing() {
        rpcServer.stop()
        rpcServer.start(credentials.load())
    }

    /** Forgets the pairing (spec D-B4); the worker returns to plaintext ADB. */
    fun forgetPairing() {
        credentials.clear()
        // The port must stop serving mTLS with credentials we no longer hold;
        // restarting is how the mode changes without a reconnect (D-B5).
        restartServing()
        Log.i(TAG, "pairing forgotten; serving plaintext on the ADB path again")
    }

    private fun nodeId(): String = advertiser.nodeId()

    /**
     * The addresses to declare at enrollment (spec D-B7). LAN addresses are
     * declared only while this worker is still on the same network the master
     * reached it on; on a captive portal only the loopback pair is declared, so
     * the router's address never ends up in a certificate as this device.
     */
    private fun declaredAddresses(): List<String> = WorkerAddresses.declared(
        lanAddresses = WorkerAddresses.localIpv4Addresses(),
        isOnLan = connectivityIsLan(),
        isCaptivePortal = connectivityIsCaptivePortal(),
    )

    private fun connectivityIsLan(): Boolean {
        val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE)
            as? ConnectivityManager ?: return false
        val network = manager.activeNetwork ?: return false
        val capabilities = manager.getNetworkCapabilities(network) ?: return false
        return capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
    }

    private fun connectivityIsCaptivePortal(): Boolean {
        val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE)
            as? ConnectivityManager ?: return false
        val network = manager.activeNetwork ?: return false
        val capabilities = manager.getNetworkCapabilities(network) ?: return false
        return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_CAPTIVE_PORTAL)
    }

    private companion object {
        const val TAG = "WorkerRuntime"

        /** The credential directory under the app's private files dir. */
        const val PAIRING_DIR = "pairing"

        /**
         * The master's pairing-only listener, reached as the worker's own
         * loopback through the ADB reverse tunnel (spec D-B2). Plain HTTP is
         * accepted only on loopback - see [EnrollmentClient].
         */
        const val ENROLL_URL = EnrollmentClient.DEFAULT_ENROLL_URL

        /** Payload prefix for a full fingerprint (`PairingUri.PREFIX`). */
        const val PAYLOAD_PREFIX = PairingUri.PREFIX
    }
}
