package org.aiupcycler.workflow

/** Read-only surface. This phase has no phone-side workflow editing. */
interface WorkflowApi {
    suspend fun listRuns(): List<WorkflowRunSummary>
    suspend fun getRun(runId: String): WorkflowRunDetail
}

data class WorkflowRunSummary(
    val runId: String,
    val model: String,
    val status: String,
    val createdAt: String?,
)

data class WorkflowStepState(
    val stepId: String,
    val status: String,
    val output: String?,
    val source: String?,
    val error: String?,
)

data class WorkflowRunDetail(
    val runId: String,
    val model: String,
    val status: String,
    val steps: List<WorkflowStepState>,
)

/** Text is the primary status indicator; colour is only a secondary cue. */
fun displayStatus(status: String): String = when (status) {
    "running" -> "Running"
    "completed" -> "Completed"
    "failed" -> "Failed"
    "blocked" -> "Blocked"
    "pending" -> "Pending"
    else -> status.replace('_', ' ').replaceFirstChar { it.uppercase() }
}
