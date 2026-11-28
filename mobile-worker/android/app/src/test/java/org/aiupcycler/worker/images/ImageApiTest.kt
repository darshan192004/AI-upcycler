package org.aiupcycler.worker.images

import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ImageApiTest {
    @Test
    fun listImages_addsBearerToken_andParsesMetadata() {
        val server = MockWebServer()
        server.enqueue(
            MockResponse().setResponseCode(200).setBody(
                """{"images":[{"image_id":"img-1","provider":"openai","model":"gpt-image-2.6-flare","prompt":"a red bicycle","media_sha256":"abc","media_path":"images/ab/abc.png","mime_type":"image/png","byte_len":68,"size":"1024x1024","created_at_epoch_ms":1732704000000}]}"""
            )
        )
        server.start()
        val api = ImageApi(server.url("/").toString().trimEnd('/'), OkHttpClient()) { "lan-test-token" }

        val images = api.list(limit = 10, offset = 0)

        val request = server.takeRequest()
        assertEquals("/api/images?limit=10&offset=0", request.path)
        assertEquals("Bearer lan-test-token", request.getHeader("Authorization"))
        assertEquals(1, images.size)
        assertEquals("img-1", images[0].imageId)
        server.shutdown()
    }

    @Test
    fun imageBytes_requireImageContentType() {
        val server = MockWebServer()
        server.enqueue(MockResponse().setResponseCode(200).setBody("not an image"))
        server.start()
        val api = ImageApi(server.url("/").toString().trimEnd('/'), OkHttpClient()) { "lan-test-token" }

        val failed = runCatching { api.imageBytes("img-1") }
        assertTrue(failed.exceptionOrNull() is ImageApiException)

        val request = server.takeRequest()
        assertEquals("/api/images/img-1", request.path)
        assertEquals("Bearer lan-test-token", request.getHeader("Authorization"))
        server.shutdown()
    }

    @Test
    fun missingToken_failsBeforeRequest() {
        val api = ImageApi("http://127.0.0.1:8080", OkHttpClient()) { null }
        val failed = runCatching { api.list() }
        assertTrue(failed.exceptionOrNull() is IllegalStateException)
        assertEquals("LAN token is missing", failed.exceptionOrNull()?.message)
    }
}
