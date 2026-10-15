package org.aiupcycler.worker

import android.app.Activity
import android.os.Bundle
import android.widget.TextView
import java.util.concurrent.Executors

/**
 * Minimal control surface: shows the worker's identity and starts the runtime.
 *
 * A foreground service (so the worker survives the UI being backgrounded) is a
 * follow-up slice; today the runtime lives for the lifetime of this activity.
 */
class MainActivity : Activity() {
    private lateinit var runtime: WorkerRuntime
    private val executor = Executors.newSingleThreadExecutor()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        runtime = WorkerRuntime(applicationContext)

        val status = TextView(this).apply {
            text = "Starting worker…"
            textSize = 16f
            setPadding(48, 96, 48, 48)
        }
        setContentView(status)

        executor.execute {
            val result = runCatching { runtime.start() }
            runOnUiThread {
                status.text = result.fold(
                    onSuccess = {
                        "AI Upcycler worker running\n\n" +
                            "id:    ${runtime.workerId}\n" +
                            "name:  ${runtime.deviceName}\n" +
                            "gRPC:  0.0.0.0:${runtime.port}\n" +
                            "mDNS:  _aiupcycler-worker._tcp"
                    },
                    onFailure = { "Worker failed to start: ${it.message}" },
                )
            }
        }
    }

    override fun onDestroy() {
        executor.execute { runtime.stop() }
        executor.shutdown()
        super.onDestroy()
    }
}
