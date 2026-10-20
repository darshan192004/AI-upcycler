package org.aiupcycler.worker.mdns

/**
 * The DNS-SD instance fullname Android's NSD registers, i.e. the `node_id` the
 * master's browser reports for this worker.
 *
 * This is a cross-language contract, not a formatting detail: the master keeps
 * `info.get_fullname()` (`core::discovery::mdns`) as the node id and
 * `EnrolledWorkerSource` resolves a worker's credentials by it, so a worker
 * that enrolls with a different string is simply never dialed. The Rust side's
 * `worker_node_id` helper is the mirror of this function and both are pinned to
 * the same golden by test (spec 2026-10-20, D-B6).
 */
object NodeIds {

    /** Service type as NSD spells it (no trailing `.local.`). */
    const val SERVICE_TYPE = "_aiupcycler-worker._tcp"

    /** The `local.` domain suffix DNS-SD appends to a service type. */
    private const val DOMAIN = "local."

    /**
     * Composes `<instance>._aiupcycler-worker._tcp.local.` - the same string
     * `ServiceInfo::get_fullname()` produces on the master, trailing dot
     * included.
     */
    fun fullname(instanceName: String): String = "$instanceName.$SERVICE_TYPE.$DOMAIN"
}
