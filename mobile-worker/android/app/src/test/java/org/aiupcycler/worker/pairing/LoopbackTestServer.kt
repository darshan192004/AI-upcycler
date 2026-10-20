package org.aiupcycler.worker.pairing

import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.charset.StandardCharsets
import kotlin.concurrent.thread

/**
 * A single-request loopback HTTP server for the enrollment-client tests.
 *
 * `com.sun.net.httpserver` is not on the Android unit-test compile classpath,
 * and a full HTTP library would be a new test dependency for one endpoint, so
 * this speaks just enough HTTP/1.1 to answer `POST /api/pairing/enroll`: read
 * the request, invoke the configured handler, write the response.
 *
 * Binds `127.0.0.1` on an ephemeral port. The enrollment client refuses
 * plaintext to anything but loopback, so this is exactly the transport the tests
 * are allowed to use.
 */
class LoopbackTestServer(
    private val handler: (Request) -> Response,
) : AutoCloseable {

    /** A parsed request: method, path, and body. */
    data class Request(val method: String, val path: String, val body: String)

    /** A canned reply. */
    data class Response(val status: Int, val body: String, val contentType: String = "application/json")

    private val socket: ServerSocket = ServerSocket()
    private val worker: Thread

    init {
        socket.bind(InetSocketAddress("127.0.0.1", 0))
        worker = thread(isDaemon = true, name = "loopback-test-server") {
            serve()
        }
    }

    val port: Int get() = socket.localPort

    fun url(path: String = "/api/pairing/enroll"): String = "http://127.0.0.1:$port$path"

    private fun serve() {
        while (!socket.isClosed) {
            val connection = try {
                socket.accept()
            } catch (e: Exception) {
                return
            }
            try {
                connection.use { handle(it) }
            } catch (e: Exception) {
                // A failed exchange only fails the test that made the call.
            }
        }
    }

    private fun handle(connection: Socket) {
        val input = connection.getInputStream()
        val request = readRequest(input)
        val response = handler(request)
        val bytes = response.body.toByteArray(StandardCharsets.UTF_8)
        val header = buildString {
            append("HTTP/1.1 ${response.status} ${statusText(response.status)}\r\n")
            append("Content-Type: ${response.contentType}\r\n")
            append("Content-Length: ${bytes.size}\r\n")
            append("Connection: close\r\n")
            append("\r\n")
        }.toByteArray(StandardCharsets.US_ASCII)
        connection.getOutputStream().use {
            it.write(header)
            it.write(bytes)
            it.flush()
        }
    }

    private fun readRequest(input: InputStream): Request {
        val headerBytes = ByteArrayOutputStream()
        var state = 0
        while (true) {
            val read = input.read()
            if (read < 0) break
            headerBytes.write(read)
            state = when {
                state == 0 && read == '\r'.code -> 1
                state == 1 && read == '\n'.code -> 2
                state == 2 && read == '\r'.code -> 3
                state == 3 && read == '\n'.code -> 4
                else -> 0
            }
            if (state == 4) break
        }
        val headerText = headerBytes.toString(StandardCharsets.US_ASCII)
        val lines = headerText.split("\r\n")
        val requestLine = lines.firstOrNull().orEmpty().split(" ")
        val contentLength = lines
            .firstOrNull { it.startsWith("Content-Length:", ignoreCase = true) }
            ?.substringAfter(':')?.trim()?.toIntOrNull() ?: 0
        val body = ByteArray(contentLength)
        var offset = 0
        while (offset < contentLength) {
            val read = input.read(body, offset, contentLength - offset)
            if (read < 0) break
            offset += read
        }
        return Request(
            method = requestLine.getOrElse(0) { "" },
            path = requestLine.getOrElse(1) { "" },
            body = String(body, 0, offset, StandardCharsets.UTF_8),
        )
    }

    private fun statusText(status: Int): String = when (status) {
        200 -> "OK"
        400 -> "Bad Request"
        403 -> "Forbidden"
        409 -> "Conflict"
        else -> "Status"
    }

    override fun close() {
        socket.close()
    }
}
