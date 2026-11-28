package org.aiupcycler.worker.images

import com.google.gson.annotations.SerializedName

data class ImageMetadata(
    @SerializedName("image_id") val imageId: String,
    @SerializedName("provider") val provider: String,
    @SerializedName("model") val model: String,
    @SerializedName("prompt") val prompt: String,
    @SerializedName("media_sha256") val mediaSha256: String,
    @SerializedName("media_path") val mediaPath: String,
    @SerializedName("mime_type") val mimeType: String,
    @SerializedName("byte_len") val byteLen: Long,
    @SerializedName("size") val size: String,
    @SerializedName("created_at_epoch_ms") val createdAtEpochMs: Long,
)
