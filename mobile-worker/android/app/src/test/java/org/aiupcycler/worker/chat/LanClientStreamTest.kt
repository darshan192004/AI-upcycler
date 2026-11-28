package org.aiupcycler.worker.chat

import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.aiupcycler.worker.chat.secrets.MemorySecretStore
import org.aiupcycler.worker.chat.secrets.SecretStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Device-free SSE contract for the LAN chat stream. The server is local and
 * every id/content value is synthetic; no bearer token or real host is used.
 */
class LanClientStreamTest {

    private val request = ChatRequest(
        sessionId = "s-stream-test",
        model = "local-synthetic",
        history = listOf(ChatMessage(role = "user", content = "hello")),
    )

    private data class Terminal(
        val deltas: List<String>,
        val done: Source?,
        val error: LanError?,
        val path: String?,
    )

    private fun stream(body: String): Terminal {
        val server = MockWebServer()
        server.enqueue(
            MockResponse()
                .setHeader("Content-Type", "text/event-stream")
                .setBody(body),
        )
        server.start()
        try {
            val client = LanClient(
                endpoint = LanEndpoint(server.hostName, server.port),
                secrets = MemorySecretStore().apply {
                    write(SecretStore.LAN_TOKEN_KEY, "lan-test-token")
                },
                callFactory = OkHttpClient(),
            )
            val latch = CountDownLatch(1)
            val done = AtomicReference<Source?>(null)
            val error = AtomicReference<LanError?>(null)
            val deltas = Collections.synchronizedList(mutableListOf<String>())

            client.streamChat(
                request = request,
                onDelta = { deltas.add(it) },
                onDone = { source -> done.set(source); latch.countDown() },
                onError = { failure -> error.set(failure); latch.countDown() },
            )

            assertTrue("no terminal frame or error was reported", latch.await(5, TimeUnit.SECONDS))
            val recorded = server.takeRequest(5, TimeUnit.SECONDS)
            return Terminal(deltas.toList(), done.get(), error.get(), recorded?.path)
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun doneFrameEndsTurnExactlyOnceWithoutError() {
        val result = stream(
            "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n" +
                "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n" +
                "data: [DONE]\n\n",
        )

        assertEquals("/v1/chat/completions", result.path)
        assertEquals(listOf("He", "llo"), result.deltas)
        assertEquals(Source.Local, result.done)
        assertNull(result.error)
    }

    @Test
    fun eofWithoutTerminalFrameReportsIncompleteResponse() {
        val result = stream(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        )

        assertEquals(listOf("partial"), result.deltas)
        assertNull(result.done)
        assertEquals(LanError.MalformedResponse, result.error)
    }

    @Test
    fun defaultCallFactoryUsesFiniteRestDeadlines() {
        val factory = LanClient.defaultCallFactory() as OkHttpClient

        assertEquals(LanClient.REST_READ_TIMEOUT_MS, factory.readTimeoutMillis.toLong())
        assertEquals(LanClient.REST_CALL_TIMEOUT_MS, factory.callTimeoutMillis.toLong())
        assertTrue(factory.readTimeoutMillis > 0)
        assertTrue(factory.callTimeoutMillis > 0)
    }
}
