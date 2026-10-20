package org.aiupcycler.worker.mdns

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The `node_id` composition is a cross-language contract (spec D-B6): the
 * master keeps `info.get_fullname()` as the node id and resolves a worker's
 * credentials by it, so the string this app composes must match exactly -
 * including the trailing dot.
 *
 * The Rust mirror is `core::discovery::mdns::worker_node_id`, whose test pins
 * the same golden against the real mdns-sd fullname. Changing either side
 * without the other breaks enrollment-to-dial silently, which is why this
 * golden is asserted literally.
 */
class NodeIdsTest {

    @Test
    fun composesTheMastersInstanceFullname() {
        assertEquals(
            "AI Upcycler Worker Pixel 8._aiupcycler-worker._tcp.local.",
            NodeIds.fullname("AI Upcycler Worker Pixel 8"),
        )
    }

    @Test
    fun matchesTheRustGoldenUsedByTheMasterBrowser() {
        // Same golden as `worker_node_id_golden...` on the Rust side.
        assertEquals(
            "worker-1._aiupcycler-worker._tcp.local.",
            NodeIds.fullname("worker-1"),
        )
    }

    @Test
    fun keepsDotsInsideTheInstanceName() {
        // NSD allows dots in a service name; the fullname is still a plain
        // concatenation, so a dot is not an extra label boundary to strip.
        assertEquals(
            "worker.1._aiupcycler-worker._tcp.local.",
            NodeIds.fullname("worker.1"),
        )
    }

    @Test
    fun serviceTypeHasNoTrailingLocalSuffix() {
        // `NsdManager` appends `.local.` itself; carrying it here would produce
        // a doubled domain in the registered fullname.
        assertEquals("_aiupcycler-worker._tcp", NodeIds.SERVICE_TYPE)
    }
}
