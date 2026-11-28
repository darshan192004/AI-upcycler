package org.aiupcycler.worker.images

import com.google.gson.Gson
import com.google.gson.JsonSyntaxException
import okhttp3.Call
import okhttp3.Request
import okhttp3.Response
import java.io.IOException

class ImageApiException(val status: Int, message: String) : IOException(message)

/**
 * Image list/bytes boundary over B's already-injected [Call.Factory].
 * Reuses the single HTTP stack: no second client, no provider key, no token logging.
 */
class ImageApi(
    private val baseUrl: String,
    private val callFactory: Call.Factory,
    private val tokenProvider: () -> String?,
) {
    private val gson = Gson()

    fun list(limit: Int = 60, offset: Int = 0): List<ImageMetadata> {
        require(limit in 1..200) { "limit must be 1..200" }
        require(offset >= 0) { "offset must be >= 0" }
        val body = executeString("/api/images?limit=$limit&offset=$offset")
        return try {
            val envelope = gson.fromJson(body, ImageListEnvelope::class.java)
                ?: throw IOException("invalid image list response")
            envelope.images?.toList() ?: throw IOException("invalid image list response")
        } catch (error: JsonSyntaxException) {
            throw IOException("invalid image list response", error)
        }
    }

    fun imageBytes(imageId: String): ByteArray {
        val response = execute("/api/images/$imageId")
        val contentType = response.header("Content-Type").orEmpty()
        if (!contentType.startsWith("image/")) {
            val code = response.code
            response.close()
            throw ImageApiException(code, "image response content type was $contentType")
        }
        return response.body?.bytes() ?: throw IOException("image response has no body")
    }

    private fun executeString(path: String): String {
        val response = execute(path)
        return response.body?.string() ?: throw IOException("response has no body")
    }

    private fun execute(path: String): Response {
        val token = tokenProvider() ?: throw IllegalStateException("LAN token is missing")
        val request = Request.Builder()
            .url(baseUrl.trimEnd('/') + path)
            .header("Authorization", "Bearer $token")
            .get()
            .build()
        val response = callFactory.newCall(request).execute()
        if (!response.isSuccessful) {
            val code = response.code
            response.close()
            throw ImageApiException(code, "image API returned HTTP $code")
        }
        return response
    }

    private data class ImageListEnvelope(val images: List<ImageMetadata>? = emptyList())
}
