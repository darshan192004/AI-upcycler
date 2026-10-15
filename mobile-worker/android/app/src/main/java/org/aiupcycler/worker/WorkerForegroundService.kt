package org.aiupcycler.worker

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import android.util.Log
import java.util.concurrent.Executors

/**
 * Keeps the worker serving while the UI is backgrounded.
 *
 * Without a foreground service Android reclaims the process as soon as the
 * activity is not visible, which drops the gRPC socket and the mDNS advert out
 * of the cluster. The ongoing notification doubles as the status surface:
 * it shows the worker id and the port the master reaches.
 *
 * `foregroundServiceType="dataSync"` is declared in the manifest because
 * `targetSdk 34` requires a type; the worker is a long-lived network data
 * source, which is what the `dataSync` type models.
 */
class WorkerForegroundService : Service() {
    private lateinit var runtime: WorkerRuntime
    private val executor = Executors.newSingleThreadExecutor()

    override fun onCreate() {
        super.onCreate()
        runtime = WorkerRuntime(applicationContext)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // `startForeground` must happen quickly on every start path, before the
        // (possibly slow) server bind and mDNS registration.
        createChannel()
        startForeground(NOTIFICATION_ID, buildNotification())
        executor.execute {
            runCatching { runtime.start() }
                .onFailure { Log.e(TAG, "worker failed to start", it) }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        executor.execute { runtime.stop() }
        executor.shutdown()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createChannel() {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    getString(R.string.notification_channel_name),
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
    }

    private fun buildNotification(): Notification =
        Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(getString(R.string.notification_text, runtime.port, runtime.workerId))
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setOngoing(true)
            .build()

    companion object {
        private const val TAG = "WorkerService"
        private const val CHANNEL_ID = "ai-upcycler-worker"
        private const val NOTIFICATION_ID = 1

        fun start(context: Context) {
            context.startForegroundService(Intent(context, WorkerForegroundService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, WorkerForegroundService::class.java))
        }
    }
}
