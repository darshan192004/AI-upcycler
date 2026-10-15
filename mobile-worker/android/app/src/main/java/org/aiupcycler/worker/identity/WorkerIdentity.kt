package org.aiupcycler.worker.identity

import android.content.Context
import android.os.Build
import java.util.UUID
import org.aiupcycler.worker.rpc.WorkerIdentitySource

/**
 * Stable identity for this device as a cluster worker.
 *
 * The worker id is generated once and persisted so the master's topology view
 * keeps the same node across restarts; the device name is derived from the
 * hardware model.
 */
class WorkerIdentity private constructor(
    override val workerId: String,
    override val deviceName: String,
    override val hasNpuAcceleration: Boolean,
) : WorkerIdentitySource {
    companion object {
        private const val PREFS = "ai-upcycler-worker"
        private const val KEY_WORKER_ID = "worker_id"

        /** NNAPI system feature string (no `PackageManager` constant exists). */
        private const val FEATURE_NNAPI = "android.hardware.neuralnetworks"

        fun from(context: Context): WorkerIdentity {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val existing = prefs.getString(KEY_WORKER_ID, null)
            val workerId = existing ?: UUID.randomUUID().toString().also {
                prefs.edit().putString(KEY_WORKER_ID, it).apply()
            }
            return WorkerIdentity(
                workerId = workerId,
                deviceName = Build.MODEL,
                hasNpuAcceleration = context.packageManager
                    .hasSystemFeature(FEATURE_NNAPI),
            )
        }
    }
}
