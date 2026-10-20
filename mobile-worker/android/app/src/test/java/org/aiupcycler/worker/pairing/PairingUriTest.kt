package org.aiupcycler.worker.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pairing payload parser (spec D-B3). The payload is the out-of-band
 * channel that makes enrollment tamper-evident, so the interesting cases are
 * the malformed ones: anything that does not describe a 64-hex fingerprint and
 * a 6-digit PIN must be refused with a reason, never parsed permissively.
 */
class PairingUriTest {

    private val fingerprint =
        "d87240d8adc360808043b42fd7e91fbe5cbbec0d21805b1b3d14b3d6ba17b4b8"

    private fun ok(payload: String): PairingUri.Result.Ok =
        PairingUri.parse(payload) as PairingUri.Result.Ok

    private fun invalid(payload: String): String =
        (PairingUri.parse(payload) as PairingUri.Result.Invalid).reason

    @Test
    fun parsesTheDocumentedPayloadShape() {
        val parsed = ok("aiupcycler://pair?fp=$fingerprint&token=811896")
        assertEquals(fingerprint, parsed.fingerprint)
        assertEquals("811896", parsed.pin)
    }

    @Test
    fun acceptsFieldOrderAndSurroundingWhitespace() {
        // A scanned payload may carry a trailing newline from the QR text.
        val parsed = ok("  aiupcycler://pair?token=811896&fp=$fingerprint  ")
        assertEquals("811896", parsed.pin)
        assertEquals(fingerprint, parsed.fingerprint)
    }

    @Test
    fun normalisesAnUppercaseFingerprint() {
        // Fingerprints are compared case-insensitively everywhere else; the
        // stored value is always lowercase so display and comparison agree.
        val parsed = ok("aiupcycler://pair?fp=${fingerprint.uppercase()}&token=811896")
        assertEquals(fingerprint, parsed.fingerprint)
    }

    @Test
    fun rejectsAnotherScheme() {
        assertTrue(invalid("https://pair?fp=$fingerprint&token=811896").startsWith("Pairing code must start"))
    }

    @Test
    fun rejectsAMissingFingerprint() {
        assertEquals(
            "Pairing code is missing the master fingerprint",
            invalid("aiupcycler://pair?token=811896"),
        )
    }

    @Test
    fun rejectsAMissingToken() {
        assertEquals(
            "Pairing code is missing the pairing token",
            invalid("aiupcycler://pair?fp=$fingerprint"),
        )
    }

    @Test
    fun rejectsATruncatedFingerprint() {
        // The truncated-fingerprint case is the one that matters: 63 hex
        // characters look right at a glance and are not a SHA-256 digest.
        assertTrue(invalid("aiupcycler://pair?fp=${fingerprint.dropLast(1)}&token=811896")
            .contains("64 hexadecimal"))
    }

    @Test
    fun rejectsANonHexFingerprint() {
        val notHex = "z".repeat(64)
        assertTrue(invalid("aiupcycler://pair?fp=$notHex&token=811896").contains("hexadecimal"))
    }

    @Test
    fun rejectsATokenThatIsNotSixDigits() {
        assertTrue(invalid("aiupcycler://pair?fp=$fingerprint&token=8118").contains("6 digits"))
        assertTrue(invalid("aiupcycler://pair?fp=$fingerprint&token=81189a").contains("6 digits"))
    }

    @Test
    fun rejectsRepeatedFields() {
        // A duplicate could be used to make the trusted-looking value differ
        // between two readers of the same payload.
        assertTrue(invalid("aiupcycler://pair?fp=$fingerprint&fp=$fingerprint&token=811896")
            .startsWith("Repeated field"))
    }

    @Test
    fun rejectsAFieldWithoutAValue() {
        assertTrue(invalid("aiupcycler://pair?fp&token=811896").startsWith("Malformed"))
    }

    @Test
    fun typedFallbackAcceptsAPinAndPrefix() {
        val parsed = PairingUri.parseManual("811896", "d87240") as PairingUri.Result.Ok
        assertEquals("811896", parsed.pin)
        assertEquals("d87240", parsed.fingerprint)

        // Uppercase is normalised here too.
        val upper = PairingUri.parseManual("811896", "D87240") as PairingUri.Result.Ok
        assertEquals("d87240", upper.fingerprint)
    }

    @Test
    fun typedFallbackRejectsAnEmptyOrNonHexPrefix() {
        assertTrue((PairingUri.parseManual("811896", "  ") as PairingUri.Result.Invalid)
            .reason.contains("first characters"))
        assertTrue((PairingUri.parseManual("811896", "nothex") as PairingUri.Result.Invalid)
            .reason.contains("0-9 or a-f"))
    }

    @Test
    fun typedFallbackRejectsAnOverlongPrefix() {
        assertTrue((PairingUri.parseManual("811896", "a".repeat(65)) as PairingUri.Result.Invalid)
            .reason.contains("at most 64"))
    }

    @Test
    fun typedFallbackRejectsANonNumericPin() {
        assertTrue((PairingUri.parseManual("81a896", "d87240") as PairingUri.Result.Invalid)
            .reason.contains("6 digits"))
    }
}
