package org.aiupcycler.worker.pairing

/**
 * Parses the pairing payload the master renders as a QR code
 * (`docs/api.md` section 3.4): `aiupcycler://pair?fp=<ca-fingerprint>&token=<pin>`.
 *
 * The payload is the out-of-band channel (spec D-B3): the fingerprint it
 * carries is what the enrollment response is checked against, so a man in the
 * middle on the wire cannot substitute its own CA. The 6-digit PIN may also be
 * typed by hand, with the fingerprint's first characters checked by eye - both
 * paths land here, and both produce the same [Result].
 *
 * Deliberately locale-independent: the payload is machine text, so both
 * comparisons use [String.equals] rather than the locale-sensitive
 * `equals(ignoreCase = true)`.
 */
object PairingUri {

    /** Scheme + authority every pairing payload starts with. */
    const val PREFIX = "aiupcycler://pair?"

    /** A CA fingerprint is a SHA-256 digest: 64 lowercase hex characters. */
    private const val FINGERPRINT_LENGTH = 64

    private val HEX = Regex("^[0-9a-f]+$")

    sealed interface Result {
        data class Ok(val fingerprint: String, val pin: String) : Result

        /** A human-readable reason, shown verbatim on the pairing screen. */
        data class Invalid(val reason: String) : Result
    }

    /**
     * Parses a scanned or pasted payload. Returns [Result.Invalid] with a
     * message that names the actual problem rather than a generic "bad code".
     */
    fun parse(payload: String): Result {
        val trimmed = payload.trim()
        if (!trimmed.startsWith(PREFIX, ignoreCase = true)) {
            return invalid("Pairing code must start with $PREFIX")
        }

        val params = mutableMapOf<String, String>()
        for (pair in trimmed.substring(PREFIX.length).split('&')) {
            if (pair.isEmpty()) continue
            val separator = pair.indexOf('=')
            if (separator <= 0) return invalid("Malformed pairing code field: $pair")
            val name = pair.substring(0, separator)
            val value = pair.substring(separator + 1)
            if (params.put(name, value) != null) {
                return invalid("Repeated field in pairing code: $name")
            }
        }

        val fingerprint = params["fp"]
            ?: return invalid("Pairing code is missing the master fingerprint")
        if (fingerprint.length != FINGERPRINT_LENGTH ||
            !HEX.containsMatchIn(fingerprint.lowercase())
        ) {
            return invalid("Master fingerprint must be $FINGERPRINT_LENGTH hexadecimal characters")
        }

        val pin = params["token"]
            ?: return invalid("Pairing code is missing the pairing token")

        return validatePin(pin, fingerprint.lowercase())
    }

    /**
     * The typed fallback (spec D-B3): a 6-digit PIN plus the leading characters
     * of the fingerprint shown on the master's pairing panel. The prefix is
     * discarded after this check - the enroll response's own full fingerprint
     * is what gets compared against the master's displayed value.
     */
    fun parseManual(pin: String, fingerprintPrefix: String): Result {
        val prefix = fingerprintPrefix.trim().lowercase()
        if (prefix.isEmpty()) {
            return invalid("Enter the first characters of the fingerprint shown on the master")
        }
        if (!HEX.containsMatchIn(prefix)) {
            return invalid("Fingerprint characters must be 0-9 or a-f")
        }
        if (prefix.length > FINGERPRINT_LENGTH) {
            return invalid("Fingerprint is at most $FINGERPRINT_LENGTH characters")
        }
        return validatePin(pin.trim(), prefix)
    }

    private fun validatePin(pin: String, fingerprint: String): Result {
        if (pin.length != PIN_LENGTH || !pin.all { it in '0'..'9' }) {
            return invalid("Pairing token must be $PIN_LENGTH digits")
        }
        return Result.Ok(fingerprint = fingerprint, pin = pin)
    }

    private fun invalid(reason: String) = Result.Invalid(reason)

    /** The PIN is always six digits (`PairingManager::create_session`). */
    const val PIN_LENGTH = 6
}
