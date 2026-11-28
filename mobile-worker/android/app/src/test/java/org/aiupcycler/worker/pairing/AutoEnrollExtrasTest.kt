package org.aiupcycler.worker.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AutoEnrollExtrasTest {

    private val fingerprint = "ab".repeat(32)

    @Test
    fun an_ordinary_launch_is_interactive() {
        assertEquals(
            Launch.Interactive,
            AutoEnrollExtras.parse(autoEnroll = false, pin = null, fingerprint = null),
        )
    }

    @Test
    fun an_ordinary_launch_ignores_extras_that_happen_to_be_present() {
        // A stale intent must not enroll the device by accident.
        assertEquals(
            Launch.Interactive,
            AutoEnrollExtras.parse(autoEnroll = false, pin = "123456", fingerprint = fingerprint),
        )
    }

    @Test
    fun valid_extras_request_an_enrollment() {
        val launch = AutoEnrollExtras.parse(
            autoEnroll = true,
            pin = " 123456 ",
            fingerprint = fingerprint.uppercase(),
        )
        assertTrue("expected auto-enroll, got $launch", launch is Launch.AutoEnroll)
        val request = launch as Launch.AutoEnroll
        assertEquals("123456", request.pin)
        assertEquals("fingerprint is normalised to lowercase", fingerprint, request.fingerprint)
    }

    @Test
    fun a_short_fingerprint_is_reported_rather_than_ignored() {
        val launch = AutoEnrollExtras.parse(
            autoEnroll = true,
            pin = "123456",
            fingerprint = "abc",
        )
        assertTrue("expected malformed, got $launch", launch is Launch.Malformed)
        val malformed = launch as Launch.Malformed
        assertTrue(malformed.reason, malformed.reason.contains("fingerprint"))
    }

    @Test
    fun a_missing_pin_is_reported() {
        val launch = AutoEnrollExtras.parse(
            autoEnroll = true,
            pin = null,
            fingerprint = fingerprint,
        )
        assertTrue("expected malformed, got $launch", launch is Launch.Malformed)
    }

    @Test
    fun a_non_hex_fingerprint_is_rejected() {
        val launch = AutoEnrollExtras.parse(
            autoEnroll = true,
            pin = "123456",
            fingerprint = "z".repeat(64),
        )
        assertTrue("expected malformed, got $launch", launch is Launch.Malformed)
    }
}
