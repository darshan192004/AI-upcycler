package org.aiupcycler.worker

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import java.util.concurrent.Executors
import org.aiupcycler.worker.identity.WorkerIdentity
import org.aiupcycler.worker.pairing.PairingInput
import org.aiupcycler.worker.pairing.PairingState

/**
 * Minimal control surface: shows the worker's identity, starts/stops the
 * foreground service that owns the runtime, and drives the pairing flow
 * (spec D-B3/D-B4/D-B5).
 *
 * The service - not this activity - owns the gRPC server and mDNS advert, so
 * closing the UI does not take the worker out of the cluster. Pairing is
 * different: it is a user-driven, one-shot exchange, so it runs here against
 * the service's own runtime, reached through the service singleton.
 *
 * The screen has two shapes - unpaired (the form) and paired (the verified
 * fingerprint plus "forget") - because the spec's two entry paths differ in
 * what can be verified, and showing a form while already paired is how a user
 * ends up re-pairing by accident.
 */
class MainActivity : Activity() {

    private lateinit var runtime: WorkerRuntime
    private lateinit var content: LinearLayout
    private val uiThread = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestNotificationPermissionIfNeeded()

        runtime = WorkerForegroundService.runtime(applicationContext)

        content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 96, 48, 48)
        }
        setContentView(content)

        render(runtime.pairingState())
        WorkerForegroundService.start(applicationContext)
    }

    override fun onDestroy() {
        worker.shutdownNow()
        super.onDestroy()
    }

    // ---- rendering --------------------------------------------------------

    private fun render(state: PairingState) {
        content.removeAllViews()
        content.addView(identityView())
        content.addView(sectionLabel(getString(R.string.pairing_title)))
        when (state) {
            is PairingState.Paired -> renderPaired(state)
            is PairingState.Unpaired -> renderUnpaired(verdict = null, enabled = true)
            is PairingState.Working ->
                renderUnpaired(getString(R.string.pairing_working), enabled = false)

            is PairingState.Rejected -> renderUnpaired(
                "${getString(R.string.pairing_rejected_headline)}: ${state.reason}",
                enabled = true,
                verdictIsError = true,
            )

            is PairingState.Failed -> renderUnpaired(
                "${getString(R.string.pairing_failed_headline)}: ${state.reason}",
                enabled = true,
                verdictIsError = true,
            )
        }
        content.addView(stopButton())
    }

    private fun renderPaired(state: PairingState.Paired) {
        content.addView(headline(getString(R.string.pairing_paired_headline), error = false))
        content.addView(body(getString(R.string.pairing_paired_body)))
        content.addView(
            body(
                "${getString(R.string.pairing_fingerprint_label)}\n${state.fingerprint}",
            ).apply { typeface = android.graphics.Typeface.MONOSPACE },
        )
        content.addView(
            Button(this).apply {
                text = getString(R.string.pairing_forget)
                setOnClickListener { forget() }
            },
        )
    }

    /**
     * The form, plus an optional verdict line. [enabled] is false while an
     * attempt is in flight, so a double tap cannot spend a second PIN.
     */
    private fun renderUnpaired(verdict: String?, enabled: Boolean, verdictIsError: Boolean = false) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            // Android 9 and below cannot do TLS 1.3, so this device can never
            // serve the mTLS the cluster requires. Said plainly rather than
            // offering a form that would appear to work and then fail at the
            // handshake.
            content.addView(headline(getString(R.string.pairing_unpaired_headline), error = false))
            content.addView(body(getString(R.string.pairing_requires_modern_android)))
            return
        }

        content.addView(headline(getString(R.string.pairing_unpaired_headline), error = false))
        content.addView(body(getString(R.string.pairing_unpaired_body)))

        val code = field(R.string.pairing_code_label, R.string.pairing_code_hint, enabled)
        val pin = field(R.string.pairing_pin_label, R.string.pairing_pin_hint, enabled)
        content.addView(code)
        content.addView(pin)

        content.addView(
            Button(this).apply {
                text = getString(R.string.pairing_action_pair)
                isEnabled = enabled
                setOnClickListener {
                    val input = PairingInput(
                        code = code.input.text.toString(),
                        pin = pin.input.text.toString(),
                    )
                    val refusal = input.rejectionReason()
                    if (refusal != null) {
                        render(PairingState.Failed(refusal))
                        return@setOnClickListener
                    }
                    render(PairingState.Working)
                    worker.execute {
                        val result = runtime.pair(
                            pin = input.resolvedPin(),
                            fingerprint = input.resolvedFingerprint(),
                            fingerprintIsComplete = input.codeIsPayload,
                        )
                        uiThread.post { render(runtime.pairingState(result)) }
                    }
                }
            },
        )

        if (verdict != null) {
            content.addView(
                headline(verdict, error = verdictIsError).apply { setPadding(0, 24, 0, 0) },
            )
        }
    }

    private fun forget() {
        worker.execute {
            runtime.forgetPairing()
            uiThread.post { render(runtime.pairingState()) }
        }
    }

    // ---- small view helpers ----------------------------------------------

    /** A labelled text field; the label is what makes it a form and not two boxes. */
    private class LabelledField(val input: EditText) : LinearLayout(input.context)

    private fun field(labelRes: Int, hintRes: Int, enabled: Boolean): LabelledField {
        val input = EditText(this).apply {
            hint = getString(hintRes)
            isEnabled = enabled
            contentDescription = getString(labelRes)
        }
        return LabelledField(input).apply {
            orientation = LinearLayout.VERTICAL
            addView(
                TextView(this@MainActivity).apply {
                    text = getString(labelRes)
                    textSize = 12f
                },
            )
            addView(input)
        }
    }

    private fun identityView(): TextView {
        val identity = WorkerIdentity.from(applicationContext)
        return TextView(this).apply {
            text = getString(R.string.status_running, identity.workerId, identity.deviceName)
            textSize = 13f
        }
    }

    private fun sectionLabel(text: String): TextView = TextView(this).apply {
        this.text = text
        textSize = 18f
        setPadding(0, 32, 0, 8)
    }

    private fun headline(text: String, error: Boolean): TextView = TextView(this).apply {
        this.text = text
        textSize = 16f
        setTextColor(if (error) ERROR_COLOR else OK_COLOR)
    }

    private fun body(text: String): TextView = TextView(this).apply {
        this.text = text
        textSize = 13f
        setPadding(0, 4, 0, 12)
    }

    private fun stopButton(): Button = Button(this).apply {
        text = getString(R.string.action_stop)
        setOnClickListener {
            WorkerForegroundService.stop(applicationContext)
            finish()
        }
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
        const val ERROR_COLOR = 0xFFB3261E.toInt()
        const val OK_COLOR = 0xFF1B5E20.toInt()
    }
}
