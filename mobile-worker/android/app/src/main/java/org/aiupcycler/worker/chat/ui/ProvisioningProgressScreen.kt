package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import org.aiupcycler.worker.chat.LanClient
import org.aiupcycler.worker.chat.LanException
import org.aiupcycler.worker.chat.ProvisionState
import org.aiupcycler.worker.chat.provisioningText
import org.aiupcycler.worker.chat.shouldPollProvisioning

/**
 * Model push status from `GET /api/devices/:node_id/provision`.
 *
 * Polling starts only when the host reports `running`, samples once on entry
 * and then once per second, and stops as soon as a terminal state is seen.
 * `LaunchedEffect` cancels the loop when this screen leaves composition.
 */
@Composable
fun ProvisioningProgressScreen(
    client: LanClient?,
    nodeId: String,
    provisioning: ProvisionState,
    onProvisioningChange: (ProvisionState) -> Unit,
    modifier: Modifier = Modifier,
) {
    val pollable = shouldPollProvisioning(provisioning)
    val updatedCallback = rememberUpdatedState(onProvisioningChange)

    LaunchedEffect(client, nodeId, pollable) {
        if (!pollable || client == null || nodeId.isBlank()) return@LaunchedEffect

        while (isActive) {
            val next = withContext(Dispatchers.IO) {
                try {
                    client.provisionState(nodeId)
                } catch (error: LanException) {
                    ProvisionState.Failed(error.error.message)
                } catch (error: IOException) {
                    ProvisionState.Failed(
                        error.message ?: "LAN host is offline; check the connection",
                    )
                }
            }
            updatedCallback.value(next)
            if (next !is ProvisionState.Running) break
            delay(1_000L)
        }
    }

    if (provisioning is ProvisionState.Idle) return

    Column(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .padding(horizontal = 8.dp, vertical = 8.dp),
    ) {
        when (val current = provisioning) {
            ProvisionState.Idle -> Unit
            is ProvisionState.Running -> {
                Text(
                    text = "Model push in progress",
                    style = MaterialTheme.typography.labelMedium.copy(fontFamily = FiraSansMedium),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(4.dp))
                val fraction = if (current.totalBytes > 0L) {
                    (current.bytesDone.toFloat() / current.totalBytes.toFloat()).coerceIn(0f, 1f)
                } else {
                    0f
                }
                LinearProgressIndicator(
                    progress = fraction,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(8.dp),
                    color = MaterialTheme.colorScheme.primary,
                    trackColor = MaterialTheme.colorScheme.surfaceVariant,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    text = provisioningText(current),
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FiraCode),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            is ProvisionState.Failed -> Text(
                text = provisioningText(current),
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FiraSans),
                color = MaterialTheme.colorScheme.error,
            )
            is ProvisionState.Done -> Text(
                text = provisioningText(current),
                style = MaterialTheme.typography.labelMedium.copy(fontFamily = FiraSansMedium),
                color = MaterialTheme.colorScheme.primary,
            )
        }
    }
}
