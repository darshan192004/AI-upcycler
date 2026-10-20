package org.aiupcycler.worker.pairing

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * The enrollment client against the real contract (spec D-B3, `docs/api.md`
 * #3.4).
 *
 * Every exchange runs over a real loopback socket against
 * [LoopbackTestServer], because "plaintext is acceptable only on loopback" is a
 * transport rule this client enforces itself - a mocked connection would not
 * exercise it.
 *
 * The success body is the committed golden `enroll_response.json`, which the
 * Rust `pairing_fixtures` example serialized from the endpoint's own
 * `EnrollResponse` type, so this suite fails if the parser and the master's
 * real JSON encoding ever disagree. Error replies use the API's own envelope
 * shape (`api/error.rs`).
 */
class EnrollmentClientTest {

    @get:Rule
    val temporary = TemporaryFolder()

    private val golden: String by lazy {
        File(FIXTURE_DIR, "enroll_response.json").readText()
    }

    /** The fingerprint the golden response was issued with. */
    private val goldenFingerprint: String
        get() = Regex("\"fingerprint\"\\s*:\\s*\"([0-9a-f]{64})\"")
            .find(golden)!!.groupValues[1]

    private val capturedRequests = mutableListOf<LoopbackTestServer.Request>()

    /** Starts a loopback server answering the enrollment route. */
    private fun server(
        status: Int,
        body: String,
        errorEnvelope: Boolean = true,
    ): LoopbackTestServer {
        val reply = if (errorEnvelope && status >= 400) {
            """{"error":{"message":"$body","type":"forbidden","code":$status}}"""
        } else {
            body
        }
        return LoopbackTestServer { request ->
            capturedRequests += request
            LoopbackTestServer.Response(status = status, body = reply)
        }
    }

    private fun request(
        server: LoopbackTestServer,
        fingerprint: String,
        pin: String = "811896",
        complete: Boolean = true,
        addresses: List<String>? = null,
    ): EnrollmentRequest {
        val pairing = (if (complete) {
            PairingUri.parse("aiupcycler://pair?fp=$fingerprint&token=$pin")
        } else {
            PairingUri.parseManual(pin, fingerprint)
        }) as PairingUri.Result.Ok
        return if (complete) {
            EnrollmentRequest.scanned(
                enrollUrl = server.url(),
                pairing = pairing,
                deviceName = "Pixel 8",
                nodeId = "Pixel 8._aiupcycler-worker._tcp.local.",
                addresses = addresses,
            )
        } else {
            EnrollmentRequest.manual(
                enrollUrl = server.url(),
                pairing = pairing,
                deviceName = "Pixel 8",
                nodeId = "Pixel 8._aiupcycler-worker._tcp.local.",
                addresses = addresses,
            )
        }
    }

    @Test
    fun parsesTheMastersGoldenEnrollResponse() {
        val server = server(200, golden)
        server.use {
            val enrolled = EnrollmentClient().enroll(request(server, goldenFingerprint))
                as EnrollmentResult.Enrolled

            assertTrue(enrolled.credentials.caPem.contains("BEGIN CERTIFICATE"))
            assertTrue(enrolled.credentials.serverKeyPem.contains("BEGIN PRIVATE KEY"))
            assertTrue(enrolled.credentials.clientKeyPem.contains("BEGIN PRIVATE KEY"))
            assertEquals(goldenFingerprint, enrolled.credentials.fingerprint)
            // The status stays clean, so a rejection and a success can never be
            // confused by a caller that forgets to branch on the type.
            assertEquals(1, capturedRequests.size)
            assertEquals("POST", capturedRequests.single().method)
            assertEquals("/api/pairing/enroll", capturedRequests.single().path)
        }
    }

    @Test
    fun refusesAFingerprintThatDoesNotMatchTheScannedCode() {
        val server = server(200, golden)
        server.use {
            // Same response, different out-of-band fingerprint: the master on
            // the wire is not the one the user vouched for.
            val result = EnrollmentClient().enroll(request(server, "00".repeat(32)))

            assertTrue((result as EnrollmentResult.Failed).reason.contains("scanned code"))
        }
    }

    @Test
    fun acceptsAGoldenResponseThatMatchesTheTypedPrefix() {
        val server = server(200, golden)
        server.use {
            val result = EnrollmentClient()
                .enroll(request(server, goldenFingerprint.take(8), complete = false))
            assertTrue(result is EnrollmentResult.Enrolled)
        }
    }

    @Test
    fun rejectsAGoldenResponseThatDoesNotMatchTheTypedPrefix() {
        val server = server(200, golden)
        server.use {
            val wrong = if (goldenFingerprint.startsWith("ffff")) "00000000" else "ffffffff"
            val result = EnrollmentClient().enroll(request(server, wrong, complete = false))
            assertTrue((result as EnrollmentResult.Failed).reason.contains("you entered"))
        }
    }

    @Test
    fun mapsAForbiddenPinToARejection() {
        val server = server(403, "invalid pairing pin")
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertTrue((result as EnrollmentResult.Rejected).reason.contains("wrong or has expired"))
        }
    }

    @Test
    fun mapsAnAlreadyUsedPinToARejection() {
        val server = server(409, "pairing pin already redeemed")
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertTrue((result as EnrollmentResult.Rejected).reason.contains("already been used"))
        }
    }

    @Test
    fun surfacesTheMastersBadRequestMessage() {
        val server = server(400, "node_id must be 1-128 characters")
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertEquals(
                "node_id must be 1-128 characters",
                (result as EnrollmentResult.Failed).reason,
            )
        }
    }

    @Test
    fun reportsAnUnreachableMasterWithoutThrowing() {
        // Nothing is listening: the tunnel is down or the session never started.
        val result = EnrollmentClient().enroll(
            EnrollmentRequest.scanned(
                enrollUrl = "http://127.0.0.1:1/api/pairing/enroll",
                pairing = PairingUri.parse("aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896")
                    as PairingUri.Result.Ok,
                deviceName = "Pixel 8",
                nodeId = "Pixel 8._aiupcycler-worker._tcp.local.",
            ),
        )
        assertTrue((result as EnrollmentResult.Failed).reason.contains("Cannot reach the master"))
    }

    @Test
    fun refusesPlainHttpToALanAddress() {
        // The enroll response carries fresh private keys; over plaintext to a
        // LAN host they would cross the network in the clear.
        val result = EnrollmentClient().enroll(
            EnrollmentRequest.scanned(
                enrollUrl = "http://192.168.1.10:8081/api/pairing/enroll",
                pairing = PairingUri.parse("aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896")
                    as PairingUri.Result.Ok,
                deviceName = "Pixel 8",
                nodeId = "Pixel 8._aiupcycler-worker._tcp.local.",
            ),
        )
        assertTrue((result as EnrollmentResult.Failed).reason.contains("Plain HTTP is only allowed"))
    }

    @Test
    fun refusesANonHttpScheme() {
        val result = EnrollmentClient().enroll(
            EnrollmentRequest.scanned(
                enrollUrl = "ftp://127.0.0.1/api/pairing/enroll",
                pairing = PairingUri.parse("aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896")
                    as PairingUri.Result.Ok,
                deviceName = "Pixel 8",
                nodeId = "node",
            ),
        )
        assertTrue((result as EnrollmentResult.Failed).reason.contains("http://"))
    }

    @Test
    fun sendsTheDocumentedRequestShape() {
        val server = server(409, "stop here")
        server.use {
            EnrollmentClient().enroll(
                EnrollmentRequest.scanned(
                    enrollUrl = server.url(),
                    pairing = PairingUri.parse(
                        "aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896",
                    ) as PairingUri.Result.Ok,
                    deviceName = "Pixel 8",
                    nodeId = "Pixel 8._aiupcycler-worker._tcp.local.",
                    addresses = listOf("192.168.1.50", "127.0.0.1"),
                ),
            )

            assertEquals(
                """{"pin":"811896","device_name":"Pixel 8",""" +
                    """"node_id":"Pixel 8._aiupcycler-worker._tcp.local.",""" +
                    """"addresses":["192.168.1.50","127.0.0.1"]}""",
                capturedRequests.single().body,
            )
        }
    }

    @Test
    fun omitsTheAddressListWhenTheWorkerDeclaresNone() {
        // Omitted means "let the master apply its ADB-tunnel defaults"; an
        // explicitly empty list is a 400 by contract, so `null` must not
        // serialize to `[]`.
        val server = server(409, "stop here")
        server.use {
            EnrollmentClient().enroll(
                EnrollmentRequest.scanned(
                    enrollUrl = server.url(),
                    pairing = PairingUri.parse(
                        "aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896",
                    ) as PairingUri.Result.Ok,
                    deviceName = "Pixel 8",
                    nodeId = "node",
                ),
            )

            val sent = capturedRequests.single().body
            assertTrue("\"node_id\":\"node\"", sent.contains("\"node_id\":\"node\""))
            assertFalse("addresses must be omitted entirely", sent.contains("addresses"))
        }
    }

    @Test
    fun escapesQuotesAndBackslashesInDeclaredValues() {
        val server = server(409, "stop here")
        server.use {
            EnrollmentClient().enroll(
                EnrollmentRequest.scanned(
                    enrollUrl = server.url(),
                    pairing = PairingUri.parse(
                        "aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896",
                    ) as PairingUri.Result.Ok,
                    deviceName = "Pixel \"8\"\\Pro",
                    nodeId = "node",
                ),
            )

            assertTrue(
                capturedRequests.single().body
                    .contains("\"device_name\":\"Pixel \\\"8\\\"\\\\Pro\""),
            )
        }
    }

    @Test
    fun reportsAMalformedSuccessBodyInsteadOfEnrolling() {
        val server = server(200, "not json at all", errorEnvelope = false)
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertTrue((result as EnrollmentResult.Failed).reason.contains("not a JSON object"))
        }
    }

    @Test
    fun readsUnicodeEscapesWithoutSkippingTheFollowingCharacter() {
        // A `\uXXXX` escape must consume exactly the escape. A review flagged
        // that the shared `index++` skips one character too many; this test
        // proves the opposite by putting the escape immediately before a real
        // field, where a skipped character would swallow that field's opening
        // quote and break the parse.
        val body = golden.replaceFirst("{", "{\"note\":\"\\u0041\",")
        val server = server(200, body, errorEnvelope = false)
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertTrue(
                "a well-formed \\u escape must not corrupt the following field: $result",
                result is EnrollmentResult.Enrolled,
            )
        }
    }

    @Test
    fun reportsAMissingCertificateFieldInsteadOfEnrolling() {
        val truncated = golden.replace(
            Regex("\"client_key_pem\"\\s*:\\s*\"[^\"]*\""),
            "\"client_key_pem\":\"\"",
        )
        val server = server(200, truncated, errorEnvelope = false)
        server.use {
            val result = EnrollmentClient().enroll(request(server, goldenFingerprint))
            assertTrue((result as EnrollmentResult.Failed).reason.contains("missing client_key_pem"))
        }
    }

    @Test
    fun neverStoresCredentialsForAFailedAttempt() {
        // The store is only ever written on `Enrolled` (spec D-B3); a failed
        // attempt must leave it untouched, including after a *successful* HTTP
        // round trip whose fingerprint did not match the user's code.
        val store = CredentialStore(temporary.newFolder("pairing"))

        val mismatched = server(200, golden)
        mismatched.use {
            val result = EnrollmentClient().enroll(request(mismatched, "00".repeat(32)))
            assertTrue(result is EnrollmentResult.Failed)
            assertFalse(store.hasCredentials())
            assertNull(store.load())
        }

        val success = server(200, golden)
        success.use {
            val credentials =
                (EnrollmentClient().enroll(request(success, goldenFingerprint))
                    as EnrollmentResult.Enrolled).credentials
            store.save(credentials)
            assertNotNull(store.load())
            assertEquals(goldenFingerprint, store.load()!!.fingerprint)
        }
    }

    @Test
    fun theTunnelPortMirrorsTheRustPairingListener() {
        // `PAIRING_PORT` in core/src/api/mod.rs; `.github/workflows` starts a
        // session on the master side of the same reverse tunnel. A drift here
        // would make every enrollment fail on the device while CI stayed green.
        assertEquals(8081, EnrollmentClient.TUNNEL_PORT)
        assertTrue(
            EnrollmentRequest.scanned(
                enrollUrl = EnrollmentClient.DEFAULT_ENROLL_URL,
                pairing = PairingUri.parse("aiupcycler://pair?fp=${"ab".repeat(32)}&token=811896")
                    as PairingUri.Result.Ok,
                deviceName = "Pixel 8",
                nodeId = "node",
            ).enrollUrl.endsWith("/api/pairing/enroll"),
        )
    }

    private companion object {
        val FIXTURE_DIR = File("src/test/resources/pairing")
    }
}
