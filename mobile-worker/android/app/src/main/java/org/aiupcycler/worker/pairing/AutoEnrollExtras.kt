package org.aiupcycler.worker.pairing

/**
 * How this activity was launched.
 *
 * The desktop app can start the worker with enrollment extras already attached
 * (`adb shell am start -n org.aiupcycler.worker/.MainActivity ...`), which is
 * how a phone joins the cluster without anyone typing a PIN. Every other launch
 * - the launcher icon, a notification - is [Interactive] and shows the form.
 */
sealed class Launch {
    /** The user opened the app themselves; show the pairing form. */
    object Interactive : Launch()

    /** The desktop app asked this device to enroll itself. */
    data class AutoEnroll(val pin: String, val fingerprint: String) : Launch()

    /** The desktop app asked, but the extras are unusable. */
    data class Malformed(val reason: String) : Launch()
}

/**
 * Extras the desktop app sets on the launch intent.
 *
 * Parsing is kept separate from [android.content.Intent] so it can be tested
 * as plain JVM code: the extras arrive as three primitives and the validation
 * is delegated to [PairingUri], the same code the manual path uses. That way a
 * fingerprint the desktop app gets wrong fails here with the same message a
 * mistyped one would.
 */
object AutoEnrollExtras {

    /** `"true"` when this launch is a desktop-driven enrollment. */
    const val AUTO_ENROLL = "aiupcycler.auto_enroll"

    /** The single-use pairing token the master issued for this device. */
    const val PIN = "aiupcycler.pin"

    /** The master's CA fingerprint: 64 lowercase hex characters. */
    const val CA_FINGERPRINT = "aiupcycler.ca_fingerprint"

    /**
     * Decides what this launch means.
     *
     * [autoEnroll] being false is the normal case and is never an error, so
     * missing extras on an ordinary launch cannot produce a spurious failure.
     */
    fun parse(autoEnroll: Boolean, pin: String?, fingerprint: String?): Launch {
        if (!autoEnroll) return Launch.Interactive

        val payload = buildString {
            append(PairingUri.PREFIX)
            append("fp=")
            append(fingerprint?.trim().orEmpty())
            append("&token=")
            append(pin?.trim().orEmpty())
        }

        return when (val parsed = PairingUri.parse(payload)) {
            is PairingUri.Result.Ok ->
                Launch.AutoEnroll(pin = parsed.pin, fingerprint = parsed.fingerprint)

            is PairingUri.Result.Invalid -> Launch.Malformed(parsed.reason)
        }
    }
}
