package org.aiupcycler.worker.pairing

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.StandardCharsets

/** What the master's CA fingerprint is checked with (spec D-B3). */
sealed interface FingerprintAuthority {
    /** The scanned or pasted payload carried the full 64-character fingerprint. */
    data class Full(val fingerprint: String) : FingerprintAuthority

    /**
     * The fingerprint was typed by hand, so only its leading characters could
     * be compared. The enrollment response's own fingerprint then becomes the
     * value the worker trusts: the remaining characters of a fingerprint the
     * user only partly checked cannot be attributed to the master.
     */
    data class Prefix(val prefix: String) : FingerprintAuthority

    companion object {
        /** A CA fingerprint is a SHA-256 digest: 64 lowercase hex characters. */
        const val FULL_LENGTH = 64
    }
}

/** Everything an enrollment attempt needs (spec D-B7). */
data class EnrollmentRequest(
    /** `http://127.0.0.1:8081`, reached through the ADB reverse tunnel. */
    val enrollUrl: String,
    /** The pairing code: a scanned/pasted pin, or a typed one (see [manual]). */
    val pairing: PairingUri.Result.Ok,
    /** How the CA fingerprint is verified: the full value, or a typed prefix. */
    val authority: FingerprintAuthority,
    /** `WorkerIdentitySource.deviceName`, what the master lists the worker as. */
    val deviceName: String,
    /** `DiscoveredNode::node_id` for this worker: the NSD-registered full name. */
    val nodeId: String,
    /**
     * The addresses the master will dial this worker at; they become the issued
     * server certificate's SANs. Omitted entirely (`null`) the master applies
     * its own ADB-tunnel loopback defaults - so this is `null` on the tunnel
     * path, unlike an explicit empty list, which the contract rejects.
     */
    val addresses: List<String>? = null,
) {
    companion object {
        /** A request from a scanned or pasted payload (full fingerprint). */
        fun scanned(
            enrollUrl: String,
            pairing: PairingUri.Result.Ok,
            deviceName: String,
            nodeId: String,
            addresses: List<String>? = null,
        ) = EnrollmentRequest(
            enrollUrl = enrollUrl,
            pairing = pairing,
            authority = FingerprintAuthority.Full(pairing.fingerprint),
            deviceName = deviceName,
            nodeId = nodeId,
            addresses = addresses,
        )

        /** A request from hand-typed PIN plus fingerprint prefix. */
        fun manual(
            enrollUrl: String,
            pairing: PairingUri.Result.Ok,
            deviceName: String,
            nodeId: String,
            addresses: List<String>? = null,
        ) = EnrollmentRequest(
            enrollUrl = enrollUrl,
            pairing = pairing,
            authority = FingerprintAuthority.Prefix(pairing.fingerprint),
            deviceName = deviceName,
            nodeId = nodeId,
            addresses = addresses,
        )
    }
}

/** The outcome of an enrollment attempt, as the pairing screen shows it. */
sealed interface EnrollmentResult {
    data class Enrolled(val credentials: WorkerCredentials) : EnrollmentResult

    /** The attempt never reached a verdict; retrying the same code may work. */
    data class Failed(val reason: String) : EnrollmentResult

    /**
     * The master answered, settling the code's fate (spec D-B3): wrong or
     * expired PIN, or one already redeemed. Retrying the same code cannot
     * work - the user needs a fresh session.
     */
    data class Rejected(val reason: String) : EnrollmentResult
}

/**
 * Redeems a pairing code against the master's pairing-only listener
 * (`POST /api/pairing/enroll`, `docs/api.md` section 3.4; spec D-B2).
 *
 * The transport is the ADB reverse tunnel: plaintext is acceptable there and
 * only there (`docs/security.md` section 3). That is why this class refuses any
 * plain-HTTP address except loopback - a LAN host would be handed the
 * worker's fresh private keys in the clear, the exact failure pairing exists
 * to prevent. Pinning TLS to the QR fingerprint arrives with the wireless
 * pairing follow-up; on the tunnel the CA is verified against the out-of-band
 * fingerprint instead.
 *
 * [connect] is injected so the request can be exercised without a socket.
 */
class EnrollmentClient(
    private val connect: (URL) -> HttpURLConnection = { url ->
        @Suppress("DEPRECATION") // Android's HttpURLConnection is URL-string based.
        url.openConnection() as HttpURLConnection
    },
) {
    fun enroll(request: EnrollmentRequest): EnrollmentResult {
        val url = try {
            URL(request.enrollUrl)
        } catch (e: Exception) {
            return EnrollmentResult.Failed("Invalid master address: ${e.message}")
        }

        transportIssue(url)?.let { return EnrollmentResult.Failed(it) }

        val body = requestBody(request)
        val connection = connect(url)
        return try {
            connection.requestMethod = "POST"
            connection.doOutput = true
            connection.connectTimeout = CONNECT_TIMEOUT_MS
            connection.readTimeout = READ_TIMEOUT_MS
            connection.setRequestProperty("Content-Type", "application/json")
            connection.outputStream.use { it.write(body.toByteArray(StandardCharsets.UTF_8)) }

            val status = connection.responseCode
            val response = readBody(connection, status)
            when (status) {
                in 200..299 -> parseEnrollment(response, request)
                HTTP_FORBIDDEN ->
                    EnrollmentResult.Rejected("The pairing code is wrong or has expired")
                HTTP_CONFLICT ->
                    EnrollmentResult.Rejected("That pairing code has already been used")
                HTTP_BAD_REQUEST ->
                    EnrollmentResult.Failed(errorMessage(response) ?: "The master rejected pairing")
                else -> EnrollmentResult.Failed(
                    "Master answered HTTP $status${errorMessage(response)?.let { ": $it" } ?: ""}",
                )
            }
        } catch (e: IOException) {
            EnrollmentResult.Failed("Cannot reach the master at ${request.enrollUrl}: ${e.message}")
        } finally {
            connection.disconnect()
        }
    }

    private fun requestBody(request: EnrollmentRequest): String {
        // Built by hand rather than through a JSON library: the fields are
        // strings, so escaping is the only thing that matters (device names
        // and node ids carry spaces, quotes and backslashes).
        val fields = mutableListOf(
            "\"pin\":${quote(request.pairing.pin)}",
            "\"device_name\":${quote(request.deviceName)}",
            "\"node_id\":${quote(request.nodeId)}",
        )
        val addresses = request.addresses
        if (addresses != null) {
            fields += "\"addresses\":[${addresses.joinToString(",") { quote(it) }}]"
        }
        return "{${fields.joinToString(",")}}"
    }

    /**
     * Rejects anything that is not a tunnel-reachable loopback address: a LAN
     * host would receive the worker's private keys over plaintext.
     */
    private fun transportIssue(url: URL): String? {
        val scheme = url.protocol.lowercase()
        val host = url.host.lowercase()
        return when {
            scheme != "http" && scheme != "https" ->
                "Master address must be an http:// or https:// URL"
            scheme == "http" && host != "127.0.0.1" && host != "localhost" ->
                "Plain HTTP is only allowed over the ADB loopback tunnel; use the master's https address"
            else -> null
        }
    }

    private fun parseEnrollment(response: String, request: EnrollmentRequest): EnrollmentResult {
        val fields = parseFlatObject(response)
            ?: return EnrollmentResult.Failed("Master returned a body that is not a JSON object")

        val caPem = fields["ca_pem"]
            ?: return EnrollmentResult.Failed("Master response is missing ca_pem")
        val fingerprint = fields["fingerprint"]?.lowercase()
            ?: return EnrollmentResult.Failed("Master response is missing fingerprint")
        val required = listOf(
            "server_cert_pem",
            "server_key_pem",
            "client_cert_pem",
            "client_key_pem",
        )
        for (field in required) {
            if (fields[field].isNullOrEmpty()) {
                return EnrollmentResult.Failed("Master response is missing $field")
            }
        }

        when (val authority = request.authority) {
            is FingerprintAuthority.Full ->
                // Deliberately not echoing either value: the user compares
                // fingerprints by eye, and a near-match must not read as a
                // success.
                if (!fingerprint.equals(authority.fingerprint, ignoreCase = true)) {
                    return EnrollmentResult.Failed(
                        "The master's certificate fingerprint does not match the scanned code",
                    )
                }

            is FingerprintAuthority.Prefix ->
                if (!fingerprint.startsWith(authority.prefix, ignoreCase = true)) {
                    return EnrollmentResult.Failed(
                        "The master's certificate fingerprint does not match the characters you entered",
                    )
                }
        }

        return EnrollmentResult.Enrolled(
            WorkerCredentials(
                caPem = caPem,
                serverCertPem = fields.getValue("server_cert_pem"),
                serverKeyPem = fields.getValue("server_key_pem"),
                clientCertPem = fields.getValue("client_cert_pem"),
                clientKeyPem = fields.getValue("client_key_pem"),
                fingerprint = fingerprint,
            ),
        )
    }

    private fun readBody(connection: HttpURLConnection, status: Int): String {
        val stream = if (status in 200..299) connection.inputStream else connection.errorStream
        return stream?.use { it.readBytes().toString(StandardCharsets.UTF_8) } ?: ""
    }

    /** The API's error envelope is `{"error":{"message":"..."}}` (`api/error.rs`). */
    private fun errorMessage(response: String): String? {
        val match = MESSAGE.find(response) ?: return null
        return match.groupValues[1].replace("\\\"", "\"").replace("\\\\", "\\")
    }

    private fun quote(value: String): String {
        val out = StringBuilder("\"")
        for (character in value) {
            when (character) {
                '"' -> out.append("\\\"")
                '\\' -> out.append("\\\\")
                '\n' -> out.append("\\n")
                '\r' -> out.append("\\r")
                '\t' -> out.append("\\t")
                else -> if (character < ' ') {
                    out.append("\\u%04x".format(character.code))
                } else {
                    out.append(character)
                }
            }
        }
        return out.append('"').toString()
    }

    /**
     * Parses a flat JSON object of string values - exactly the enroll response
     * and error envelope shapes. Deliberately not a general JSON parser: the
     * values are PEM blocks and hex digests, and a general parser would be
     * more code to trust for no added coverage.
     */
    private fun parseFlatObject(json: String): Map<String, String>? {
        val fields = mutableMapOf<String, String>()
        var index = 0

        fun skipWhitespace() {
            while (index < json.length && json[index].isWhitespace()) index++
        }

        fun readString(): String? {
            if (index >= json.length || json[index] != '"') return null
            index++
            val out = StringBuilder()
            while (index < json.length) {
                when (val character = json[index]) {
                    '"' -> {
                        index++
                        return out.toString()
                    }

                    '\\' -> {
                        index++
                        if (index >= json.length) return null
                        when (json[index]) {
                            '"' -> out.append('"')
                            '\\' -> out.append('\\')
                            '/' -> out.append('/')
                            'n' -> out.append('\n')
                            'r' -> out.append('\r')
                            't' -> out.append('\t')
                            'b' -> out.append('\b')
                            'f' -> out.append('\u000c')
                            'u' -> {
                                if (index + 4 >= json.length) return null
                                val code = json.substring(index + 1, index + 5).toIntOrNull(16)
                                    ?: return null
                                out.append(code.toChar())
                                index += 4
                            }

                            else -> return null
                        }
                        index++
                    }

                    else -> {
                        out.append(character)
                        index++
                    }
                }
            }
            return null
        }

        skipWhitespace()
        if (index >= json.length || json[index] != '{') return null
        index++
        skipWhitespace()
        if (index < json.length && json[index] == '}') return fields

        while (index < json.length) {
            skipWhitespace()
            val key = readString() ?: return null
            skipWhitespace()
            if (index >= json.length || json[index] != ':') return null
            index++
            skipWhitespace()
            if (index < json.length && json[index] == '"') {
                val value = readString() ?: return null
                fields[key] = value
            } else {
                // Only string fields are consumed; anything else is skipped by
                // scanning to the next delimiter.
                while (index < json.length && json[index] != ',' && json[index] != '}') index++
            }
            skipWhitespace()
            when {
                index >= json.length -> return null
                json[index] == ',' -> index++
                json[index] == '}' -> return fields
                else -> return null
            }
        }
        return fields
    }

    companion object {
        /**
         * The master's pairing-only listener port, mirrored from `PAIRING_PORT`
         * in `core/src/api/mod.rs`. The ADB reverse tunnel maps this port on the
         * worker's loopback to the master's `8081`, so the two must agree
         * (spec D-B2).
         */
        const val TUNNEL_PORT = 8081

        /** The enroll route on the tunnel, from `docs/api.md` section 3.4. */
        const val DEFAULT_ENROLL_URL = "http://127.0.0.1:$TUNNEL_PORT/api/pairing/enroll"

        private const val CONNECT_TIMEOUT_MS = 10_000
        private const val READ_TIMEOUT_MS = 15_000

        private const val HTTP_BAD_REQUEST = 400
        private const val HTTP_FORBIDDEN = 403
        private const val HTTP_CONFLICT = 409

        private val MESSAGE = Regex("\"message\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"")
    }
}
