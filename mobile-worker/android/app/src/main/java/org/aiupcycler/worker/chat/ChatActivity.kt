package org.aiupcycler.worker.chat

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.aiupcycler.worker.MainActivity
import org.aiupcycler.worker.R
import org.aiupcycler.worker.WorkerForegroundService
import org.aiupcycler.worker.chat.ui.ChatScreen
import org.aiupcycler.worker.chat.ui.ChatTheme
import org.aiupcycler.worker.pairing.AutoEnrollExtras

/**
 * The B launcher. It starts the one foreground service, hosts the chat tree,
 * and keeps the existing control/pairing screen one overflow tap away.
 *
 * No pairing work happens here: `onNewIntent` deliberately ignores enrollment
 * extras. Only a LAN discovery callback extra is consumed, so the desktop
 * app's explicit `am start .../.MainActivity` path remains untouched.
 */
@OptIn(ExperimentalMaterial3Api::class)
class ChatActivity : ComponentActivity() {
    private val viewModel: ChatViewModel by lazy {
        ViewModelProvider(this, ChatViewModel.factory(applicationContext))[ChatViewModel::class.java]
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestNotificationPermissionIfNeeded()
        WorkerForegroundService.start(applicationContext)

        setContent {
            ChatRoot()
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.hasExtra(AutoEnrollExtras.AUTO_ENROLL) ||
            intent.hasExtra(AutoEnrollExtras.PIN) ||
            intent.hasExtra(AutoEnrollExtras.CA_FINGERPRINT)
        ) {
            return
        }
        val endpoint = intent.getStringExtra(EXTRA_LAN_ENDPOINT) ?: return
        runCatching { LanEndpoint.parse(endpoint) }
            .getOrNull()
            ?.let(viewModel::onDiscoveryEndpoint)
    }

    @Composable
    private fun ChatRoot() {
        val state by viewModel.uiState.collectAsStateWithLifecycle()
        val banner by viewModel.banner.collectAsStateWithLifecycle()
        val client by viewModel.lanClient.collectAsStateWithLifecycle()
        var aboutVisible by remember { mutableStateOf(false) }

        LaunchedEffect(client) {
            if (client != null) viewModel.openProvisioning()
        }

        Column(modifier = Modifier.fillMaxSize()) {
            ChatTheme {
                TopAppBar(
                    title = { Text(text = stringResource(R.string.app_name)) },
                    actions = {
                        OverflowMenu(
                            onControl = {
                                startActivity(Intent(this@ChatActivity, MainActivity::class.java))
                            },
                            onAbout = { aboutVisible = true },
                        )
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.surface,
                        titleContentColor = MaterialTheme.colorScheme.onSurface,
                        actionIconContentColor = MaterialTheme.colorScheme.onSurface,
                    ),
                )
                banner?.let { current ->
                    Surface(
                        color = MaterialTheme.colorScheme.errorContainer,
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag(
                                if (current is Banner.PairingRequired) PAIRING_BANNER_TAG
                                else CHAT_BANNER_TAG,
                            ),
                    ) {
                        Text(
                            text = current.message,
                            modifier = Modifier.padding(16.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onErrorContainer,
                        )
                    }
                }
            }

            // ChatScreen installs ChatTheme itself; it is not wrapped again here.
            ChatScreen(
                state = state,
                onEvent = viewModel::onEvent,
                onNewSession = viewModel::newSession,
                onSelectSession = viewModel::selectSession,
                onDraftChange = viewModel::onDraftChange,
                onSend = viewModel::send,
                onStop = viewModel::stop,
                onAttachImage = { fileName, mimeType, bytes ->
                    viewModel.attachImage(fileName, mimeType, bytes)
                },
                onRemoveAttachment = viewModel::removeAttachment,
                onProviderSelected = viewModel::selectProvider,
                onModelSelected = viewModel::selectModel,
                provisioningClient = client,
                nodeId = viewModel.nodeId,
                modifier = Modifier.weight(1f),
            )
        }

        if (aboutVisible) {
            ChatTheme {
                AlertDialog(
                    onDismissRequest = { aboutVisible = false },
                    confirmButton = {
                        Button(onClick = { aboutVisible = false }) { Text(text = "OK") }
                    },
                    title = { Text(text = "About") },
                    text = {
                        Text(
                            text = "${stringResource(R.string.app_name)} 0.1.0 — private LAN chat.",
                        )
                    },
                )
            }
        }
    }

    @Composable
    private fun OverflowMenu(
        onControl: () -> Unit,
        onAbout: () -> Unit,
    ) {
        var expanded by remember { mutableStateOf(false) }
        Button(
            onClick = { expanded = true },
            modifier = Modifier
                .heightIn(min = 48.dp)
                .testTag(OVERFLOW_BUTTON_TAG)
                .semantics { contentDescription = "More options" },
        ) {
            Text(text = "More")
        }
        DropdownMenu(
            expanded = expanded,
            onDismissRequest = { expanded = false },
            modifier = Modifier.testTag(OVERFLOW_MENU_TAG),
        ) {
            DropdownMenuItem(
                text = { Text(text = "Control & pairing") },
                onClick = {
                    expanded = false
                    onControl()
                },
                modifier = Modifier
                    .heightIn(min = 48.dp)
                    .testTag(OVERFLOW_ITEM_TAG),
            )
            DropdownMenuItem(
                text = { Text(text = "About") },
                onClick = {
                    expanded = false
                    onAbout()
                },
                modifier = Modifier
                    .heightIn(min = 48.dp)
                    .testTag(OVERFLOW_ITEM_TAG),
            )
        }
    }

    private fun requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), NOTIFICATION_REQUEST)
        }
    }

    companion object {
        /** LAN discovery callback: a validated `host:port`, never a token. */
        const val EXTRA_LAN_ENDPOINT = "org.aiupcycler.worker.chat.LAN_ENDPOINT"

        const val OVERFLOW_ITEM_TAG = "chat-overflow-item"
        const val PAIRING_BANNER_TAG = "chat-pairing-banner"

        private const val NOTIFICATION_REQUEST = 1
        private const val OVERFLOW_BUTTON_TAG = "chat-overflow-button"
        private const val OVERFLOW_MENU_TAG = "chat-overflow-menu"
        private const val CHAT_BANNER_TAG = "chat-banner"
    }
}
