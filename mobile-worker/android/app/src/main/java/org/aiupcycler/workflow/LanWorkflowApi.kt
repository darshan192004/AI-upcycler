package org.aiupcycler.workflow

import java.net.URLEncoder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.aiupcycler.worker.chat.LanClient

/**
 * Read-only workflow API over B's injected LAN client. The client already owns
 * the endpoint, the bearer token from [org.aiupcycler.worker.chat.secrets.SecretStore],
 * and the call factory; this adapter stores neither a token nor a base URL.
 */
class LanWorkflowApi(
    private val client: LanClient,
) : WorkflowApi {

    override suspend fun listRuns(): List<WorkflowRunSummary> = withContext(Dispatchers.IO) {
        val array = client.getJson("/api/workflows").getJSONArray("runs")
        (0 until array.length()).map { index ->
            val row = array.getJSONObject(index)
            WorkflowRunSummary(
                runId = row.getString("run_id"),
                model = row.getString("model"),
                status = row.getString("status"),
                createdAt = if (row.isNull("created_at")) null else row.optString("created_at"),
            )
        }
    }

    override suspend fun getRun(runId: String): WorkflowRunDetail = withContext(Dispatchers.IO) {
        val body = client.getJson("/api/workflows/${encode(runId)}")
        val run = body.getJSONObject("run")
        val array = body.getJSONArray("steps")
        val steps = (0 until array.length()).map { index ->
            val row = array.getJSONObject(index)
            WorkflowStepState(
                stepId = row.getString("step_id"),
                status = row.getString("status"),
                output = if (row.isNull("output")) null else row.optString("output"),
                source = if (row.isNull("source")) null else row.optString("source"),
                error = if (row.isNull("error")) null else row.optString("error"),
            )
        }
        WorkflowRunDetail(
            runId = run.getString("run_id"),
            model = run.getString("model"),
            status = run.getString("status"),
            steps = steps,
        )
    }
}

private fun encode(value: String): String = URLEncoder.encode(value, "UTF-8")
