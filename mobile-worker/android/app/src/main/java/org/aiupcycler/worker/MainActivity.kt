package org.aiupcycler.worker

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import org.aiupcycler.worker.identity.WorkerIdentity

/**
 * Minimal control surface: shows the worker's identity and starts/stops the
 * foreground service that actually owns the runtime.
 *
 * The service — not this activity — owns the gRPC server and mDNS advert, so
 * closing the UI no longer takes the worker out of the cluster.
 */
class MainActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestNotificationPermissionIfNeeded()

        val identity = WorkerIdentity.from(applicationContext)
        val status = TextView(this).apply {
            text = getString(
                R.string.status_running,
                identity.workerId,
                identity.deviceName,
            )
            textSize = 16f
        }
        val stop = Button(this).apply {
            text = getString(R.string.action_stop)
            setOnClickListener {
                WorkerForegroundService.stop(applicationContext)
                finish()
            }
        }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(48, 96, 48, 48)
                addView(status)
                addView(stop)
            },
        )

        WorkerForegroundService.start(applicationContext)
    }

    /** Notification is informational; the service still runs if this is denied. */
    private fun requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), NOTIFICATION_REQUEST)
        }
    }

    private companion object {
        const val NOTIFICATION_REQUEST = 1
    }
}
