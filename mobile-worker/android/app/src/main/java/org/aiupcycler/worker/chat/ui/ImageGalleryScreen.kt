package org.aiupcycler.worker.chat.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.aiupcycler.worker.images.ImageApi
import org.aiupcycler.worker.images.ImageMetadata

private val GalleryBackground = Color(0xFF0F172A)
private val GalleryCard = Color(0xFF1B2336)
private val GalleryPressed = Color(0xFF334155)
private val GalleryText = Color(0xFFE2E8F0)
private val GalleryMuted = Color(0xFF94A3B8)
private val GalleryError = Color(0xFFFCA5A5)

private data class GalleryState(
    val images: List<ImageMetadata> = emptyList(),
    val loading: Boolean = true,
    val error: String? = null,
)

/** Per-image state, kept separate from gallery metadata so one failed byte
 * request does not leave its card on the loading placeholder forever. */
private data class GalleryImageState(
    val loading: Boolean = true,
    val error: String? = null,
    val bitmap: ImageBitmap? = null,
)

/**
 * Generated-image gallery hosted as one slot inside the existing `ChatScreen`.
 * It reuses B's injected `ImageApi` and inherits the host `ChatScreen` theme;
 * no second host, theme, navigation graph, or HTTP client is introduced. The bearer token is supplied
 * by the caller's token provider and is never rendered or logged here.
 */
@Composable
fun imageGalleryScreen(api: ImageApi, onClose: () -> Unit) {
    var state by remember { mutableStateOf(GalleryState()) }
    var refreshKey by remember { mutableIntStateOf(0) }

    fun load() {
        refreshKey += 1
    }

    LaunchedEffect(api, refreshKey) {
        state = state.copy(loading = true, error = null)
        val loaded = withContext(Dispatchers.IO) {
            runCatching { api.list() }
        }
        state = loaded.fold(
            onSuccess = { GalleryState(images = it, loading = false, error = null) },
            onFailure = {
                GalleryState(
                    loading = false,
                    error = it.message ?: "unknown error",
                )
            },
        )
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(GalleryBackground)
            .padding(16.dp)
            .testTag("image-gallery-screen"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = "Images",
                style = MaterialTheme.typography.titleLarge,
                color = GalleryText,
                modifier = Modifier
                    .weight(1f)
                    .semantics { contentDescription = "Images" },
            )
            val closeInteraction = remember { MutableInteractionSource() }
            val closePressed by closeInteraction.collectIsPressedAsState()
            IconButton(
                onClick = onClose,
                modifier = Modifier
                    .size(48.dp)
                    .background(
                        color = if (closePressed) GalleryPressed else GalleryBackground,
                        shape = CircleShape,
                    )
                    .semantics { contentDescription = "Close images" }
                    .testTag("image-gallery-close"),
                interactionSource = closeInteraction,
            ) {
                Icon(
                    imageVector = Icons.Default.Close,
                    contentDescription = null,
                    tint = GalleryText,
                )
            }
        }
        state.error?.let { message ->
            Text(
                text = "Could not load images: $message",
                style = MaterialTheme.typography.bodyMedium,
                color = GalleryError,
                modifier = Modifier.testTag("image-gallery-error"),
            )
        }
        if (state.error != null) {
            val retryInteraction = remember { MutableInteractionSource() }
            val retryPressed by retryInteraction.collectIsPressedAsState()
            Button(
                onClick = { load() },
                modifier = Modifier
                    .heightIn(min = 48.dp)
                    .testTag("image-gallery-retry"),
                interactionSource = retryInteraction,
                colors = ButtonDefaults.buttonColors(
                    containerColor = if (retryPressed) GalleryPressed else GalleryCard,
                    contentColor = GalleryText,
                ),
            ) {
                Text(text = "Retry")
            }
        }
        LazyColumn(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (state.loading) {
                item(key = "image-gallery-loading") {
                    GalleryLoadingPlaceholder()
                }
            }
            items(items = state.images, key = { it.imageId }) { item ->
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(GalleryCard, RoundedCornerShape(8.dp))
                        .padding(8.dp)
                        .testTag("image-gallery-item-${item.imageId}"),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    GalleryImage(api = api, item = item)
                    Text(
                        text = item.prompt,
                        style = MaterialTheme.typography.bodyMedium,
                        color = GalleryText,
                    )
                    Text(
                        text = "${item.provider} / ${item.model}",
                        style = MaterialTheme.typography.bodySmall,
                        color = GalleryMuted,
                    )
                }
            }
        }
    }
}

@Composable
private fun GalleryLoadingPlaceholder(tag: String = "image-gallery-loading") {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .testTag(tag)
            .height(160.dp)
            .background(GalleryCard),
    )
}

@Composable
private fun GalleryImage(api: ImageApi, item: ImageMetadata) {
    var state by remember(item.imageId) { mutableStateOf(GalleryImageState()) }
    var retryKey by remember(item.imageId) { mutableIntStateOf(0) }

    LaunchedEffect(api, item.imageId, retryKey) {
        state = GalleryImageState(loading = true)
        val result = withContext(Dispatchers.IO) {
            runCatching {
                val bytes = api.imageBytes(item.imageId)
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
            }
        }
        state = result.fold(
            onSuccess = { bitmap ->
                if (bitmap != null) {
                    GalleryImageState(loading = false, bitmap = bitmap)
                } else {
                    GalleryImageState(loading = false, error = "Image could not be decoded")
                }
            },
            onFailure = { failure ->
                if (failure is CancellationException) throw failure
                GalleryImageState(loading = false, error = failure.message ?: "unknown error")
            },
        )
    }

    val image = state.bitmap
    when {
        image != null -> Image(
            bitmap = image,
            contentDescription = item.prompt,
            modifier = Modifier
                .fillMaxWidth()
                .height(160.dp),
            contentScale = ContentScale.Crop,
        )

        state.error != null -> Column(
            modifier = Modifier
                .fillMaxWidth()
                .height(160.dp)
                .background(GalleryCard, RoundedCornerShape(8.dp))
                .padding(8.dp)
                .testTag("image-gallery-item-error-${item.imageId}"),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = "Could not load image: ${state.error}",
                style = MaterialTheme.typography.bodySmall,
                color = GalleryError,
            )
            Button(
                onClick = { retryKey += 1 },
                modifier = Modifier
                    .heightIn(min = 48.dp)
                    .semantics { contentDescription = "Retry image ${item.imageId}" }
                    .testTag("image-gallery-item-retry-${item.imageId}"),
                colors = ButtonDefaults.buttonColors(
                    containerColor = GalleryPressed,
                    contentColor = GalleryText,
                ),
            ) {
                Text(text = "Retry image")
            }
        }

        else -> GalleryLoadingPlaceholder(tag = "image-gallery-item-loading-${item.imageId}")
    }
}
