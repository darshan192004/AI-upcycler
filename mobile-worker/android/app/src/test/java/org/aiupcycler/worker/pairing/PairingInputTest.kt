package org.aiupcycler.worker.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pairing screen's input rules (spec D-B3).
 *
 * The rule under test is the one that is easy to get silently wrong: a pasted
 * payload is checked against the master's **full** fingerprint, and a typed
 * fingerprint only against the characters the user gave. Collapsing those two
 * into one path would downgrade every scanned pairing to a prefix check, and
 * nothing would look broken.
 *
 * `PairingInput` carries the derivation; `WorkerRuntime.pair` consumes the flag
 * it produces. This test pins the derivation, and the rejections that must
 * happen before any network call.
 */
class PairingInputTest {

    private val fullFingerprint = "8a06e436cc1b2e771dc91c60172185ba457a9e406c40709d0f585e20e7571e0f"

    private fun payload(fp: String = fullFingerprint, token: String = "811896") =
        "aiupcycler://pair?fp=$fp&token=$token"

    // ---- the derivation that matters --------------------------------------

    @Test
    fun aPayloadCarriesItsFingerprintAndPin() {
        val input = PairingInput(code = payload())

        assertEquals(fullFingerprint, input.resolvedFingerprint())
        assertEquals("811896", input.resolvedPin())
        assertTrue(
            "a scanned payload must be verified against the whole fingerprint",
            input.codeIsPayload,
        )
    }

    @Test
    fun aTypedPrefixIsCheckedOnlyAgainstWhatWasTyped() {
        val input = PairingInput(code = "8a06e436", pin = "811896")

        assertEquals("8a06e436", input.resolvedFingerprint())
        assertEquals("811896", input.resolvedPin())
        assertFalse(
            "a typed prefix must not be treated as a full fingerprint",
            input.codeIsPayload,
        )
    }

    @Test
    fun aBareFingerprintIsNotAPayload() {
        assertFalse(PairingInput(code = fullFingerprint).codeIsPayload)
        assertTrue(PairingInput(code = payload(fp = "ab", token = "1")).codeIsPayload)
    }

    @Test
    fun aBareFingerprintLongerThanSixtyFourCharactersIsStillNotAPayload() {
        // A prefix of any length is a prefix, not a payload - the scheme is what
        // separates the two paths, and guessing from length would let a
        // 65-character paste be treated as an authoritative full fingerprint.
        assertFalse(PairingInput(code = "a".repeat(65)).codeIsPayload)
    }

    // ---- what must be refused before any network call ---------------------

    @Test
    fun anEmptyCodeIsRefused() {
        val reason = PairingInput(code = "  ", pin = "811896").rejectionReason()
        assertTrue("an empty code must be refused", reason != null)
        assertTrue(reason!!.contains("code"))
    }

    @Test
    fun aMissingPinIsRefused() {
        val reason = PairingInput(code = fullFingerprint).rejectionReason()
        assertTrue("a missing PIN must be refused", reason != null)
        assertTrue(reason!!.contains("PIN"))
    }

    @Test
    fun aCompletePayloadHasNothingToRefuse() {
        assertNull(PairingInput(code = payload()).rejectionReason())
    }

    @Test
    fun aTypedPrefixWithAPinHasNothingToRefuse() {
        assertNull(PairingInput(code = "8a06e436", pin = "811896").rejectionReason())
    }

    @Test
    fun aTruncatedPayloadIsReportedAsIncompleteRatherThanDowngradedToAPrefix() {
        // The failure this guards: a paste that lost its tail still starts with
        // the scheme. Treating it as a bare prefix would make the worker trust
        // the response's own fingerprint - i.e. verify almost nothing - while
        // the UI implied the scanned code had been checked. It must be refused
        // before any network call instead, and the message must name the paste
        // problem rather than send the user looking for a PIN that is already
        // inside the code they tried to paste.
        val truncated = payload(fp = "8a06e436")

        val reason = PairingInput(code = truncated).rejectionReason()

        assertTrue("a malformed payload must be refused", reason != null)
        assertTrue("the message must name the paste problem, got $reason", reason!!.contains("incomplete"))
    }

    // ---- field precedence -------------------------------------------------

    @Test
    fun theExplicitPinWinsOverTheOneInsideThePayload() {
        val input = PairingInput(code = payload(token = "111111"), pin = "222222")
        assertEquals("222222", input.resolvedPin())
    }

    @Test
    fun thePayloadPinIsUsedWhenNoPinWasTyped() {
        assertEquals("111111", PairingInput(code = payload(token = "111111")).resolvedPin())
    }

    @Test
    fun aBlankPinFieldFallsBackToThePayload() {
        val input = PairingInput(code = payload(token = "811896"), pin = "   ")
        assertEquals("811896", input.resolvedPin())
    }
}
