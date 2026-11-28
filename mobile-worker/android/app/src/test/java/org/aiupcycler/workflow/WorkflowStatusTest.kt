package org.aiupcycler.workflow

import org.junit.Assert.assertEquals
import org.junit.Test

class WorkflowStatusTest {
    @Test
    fun maps_known_workflow_statuses_to_readable_text() {
        assertEquals("Running", displayStatus("running"))
        assertEquals("Completed", displayStatus("completed"))
        assertEquals("Failed", displayStatus("failed"))
        assertEquals("Blocked", displayStatus("blocked"))
        assertEquals("Pending", displayStatus("pending"))
    }

    @Test
    fun maps_an_unknown_status_without_crashing() {
        assertEquals("Queued later", displayStatus("queued_later"))
    }
}
