package org.aiupcycler.workflow

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException

/**
 * B's Compose host owns navigation; E supplies this tab. It only reads runs:
 * no add/edit/delete controls exist here in this phase.
 */
@Composable
fun WorkflowProgressScreen(api: WorkflowApi, modifier: Modifier = Modifier) {
    var runs by remember { mutableStateOf<List<WorkflowRunSummary>>(emptyList()) }
    var detail by remember { mutableStateOf<WorkflowRunDetail?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var refreshKey by remember { mutableIntStateOf(0) }
    var detailKey by remember { mutableIntStateOf(0) }
    var selectedRunId by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(api, refreshKey) {
        try {
            val loaded = api.listRuns()
            runs = loaded
            // Refresh keeps a still-present selection; first run is only the default.
            selectedRunId = loaded.firstOrNull { it.runId == selectedRunId }?.runId
                ?: loaded.firstOrNull()?.runId
            detailKey += 1
            error = null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            error = failure.message ?: "Unable to load workflow progress"
        }
    }

    LaunchedEffect(api, selectedRunId, detailKey) {
        val runId = selectedRunId ?: return@LaunchedEffect
        try {
            detail = api.getRun(runId)
            error = null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            error = failure.message ?: "Unable to load workflow progress"
        }
    }

    Column(
        modifier = modifier.padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Task progress", style = MaterialTheme.typography.titleLarge)
        Button(
            onClick = { refreshKey += 1 },
            modifier = Modifier.heightIn(min = 48.dp),
        ) {
            Text("Refresh")
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            items(items = runs, key = { it.runId }) { run ->
                Card(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(role = Role.Button) {
                            selectedRunId = run.runId
                            detailKey += 1
                        },
                ) {
                    Column(modifier = Modifier.padding(12.dp)) {
                        Text("${run.model} - ${displayStatus(run.status)}")
                        Text(run.runId, style = MaterialTheme.typography.bodySmall)
                        if (run.runId == selectedRunId) {
                            Text("Selected", style = MaterialTheme.typography.labelSmall)
                        }
                    }
                }
            }
            detail?.let { selected ->
                items(items = selected.steps, key = { it.stepId }) { step ->
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Row(
                            modifier = Modifier.padding(12.dp),
                            verticalAlignment = Alignment.Top,
                        ) {
                            Text(displayStatus(step.status))
                            Spacer(Modifier.width(8.dp))
                            Column {
                                Text(step.stepId, style = MaterialTheme.typography.bodySmall)
                                step.source?.let { Text("source: $it") }
                                step.output?.let { Text(it, maxLines = 2) }
                                step.error?.let {
                                    Text("Error: $it", color = MaterialTheme.colorScheme.error)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
