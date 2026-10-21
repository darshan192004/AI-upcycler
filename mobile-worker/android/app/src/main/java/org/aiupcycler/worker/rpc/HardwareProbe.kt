package org.aiupcycler.worker.rpc

/**
 * On-demand hardware self-measurement (spec 2026-10-21, D-C2/D-C9).
 *
 * The Kotlin twin of the Rust worker's `worker/src/probe.rs`: same buffer
 * size, same pass count, same fused-multiply-add shape, so a master compares
 * two workers on the same scale rather than on two different measurements.
 *
 * Pure and synchronous: no I/O, no threads, fixed sizes. The wall time is
 * bounded to roughly half a second by construction, which keeps the probe
 * from competing with serving.
 */
object HardwareProbe {

    /** Elements in the bandwidth probe buffer (32 MiB of `f32`). */
    const val BANDWIDTH_BUFFER_ELEMENTS: Int = 8 * 1024 * 1024

    /** Sequential read+write passes over the buffer. */
    const val BANDWIDTH_PASSES: Int = 2

    /** Fused multiply-add iterations in the compute probe. */
    const val FLOPS_ITERATIONS: Long = 64L * 1024L * 1024L

    /**
     * The measured numbers from one probe run.
     *
     * @param memoryBandwidthMbs sequential read+write throughput, MB/s.
     * @param throughputFlops single-threaded f32 FMA rate, FLOP/s.
     * @param probeDurationMs wall time of the probe loops, in milliseconds.
     */
    data class Result(
        val memoryBandwidthMbs: Double,
        val throughputFlops: Double,
        val probeDurationMs: Double,
    )

    /**
     * Runs both probes and returns their measurements.
     *
     * The buffer is allocated before the timer starts: the reported duration
     * exists so a master can sanity-check a degenerate run, so it must cover
     * the probe loops only and not the allocation.
     */
    @JvmStatic
    fun measure(): Result {
        val buffer = FloatArray(BANDWIDTH_BUFFER_ELEMENTS) { 1.0f }
        val started = System.nanoTime()
        val bandwidth = probeBandwidthMbs(buffer, BANDWIDTH_PASSES)
        val flops = probeThroughputFlops(FLOPS_ITERATIONS)
        val elapsedMs = (System.nanoTime() - started) / 1_000_000.0
        return Result(bandwidth, flops, elapsedMs)
    }

    /**
     * Sequential read+write throughput of [buffer], in MB/s.
     *
     * The accumulation is a dependent chain, so the compiler cannot fold it
     * away, and the result is consumed at the end so the work stays
     * observable.
     */
    @JvmStatic
    fun probeBandwidthMbs(buffer: FloatArray, passes: Int): Double {
        if (passes <= 0) return 0.0
        val bytesPerPass = 2.0 * buffer.size * 4.0
        val started = System.nanoTime()
        var sink = 0.0f
        for (pass in 0 until passes) {
            var index = 0
            while (index < buffer.size) {
                val value = buffer[index] + 1.0f
                buffer[index] = value
                sink += value
                index++
            }
            index = 0
            while (index < buffer.size) {
                sink += buffer[index]
                index++
            }
        }
        // Consume the sink so the reads cannot be optimized away.
        if (sink == Float.POSITIVE_INFINITY || sink == Float.NEGATIVE_INFINITY) {
            return 0.0
        }
        val elapsed = (System.nanoTime() - started) / 1_000_000_000.0
        if (elapsed <= 0.0) return 0.0
        return (bytesPerPass * passes) / elapsed / 1_000_000.0
    }

    /**
     * Single-threaded f32 fused multiply-add rate, in FLOP/s (2 FLOPs per FMA).
     *
     * The chain is data-dependent, so it cannot be constant-folded.
     */
    @JvmStatic
    fun probeThroughputFlops(iterations: Long): Double {
        if (iterations <= 0) return 0.0
        val started = System.nanoTime()
        var a = 1.000001f
        var b = 0.999999f
        var index = 0L
        while (index < iterations) {
            val scale = 1.0f + ((index % 7L).toFloat()) * 1e-6f
            a = a * b + scale
            b = b * 0.9999995f + 1e-9f
            index++
        }
        // Consume the result so the chain cannot be optimized away.
        if (a == Float.POSITIVE_INFINITY || b == Float.NEGATIVE_INFINITY) {
            return 0.0
        }
        val elapsed = (System.nanoTime() - started) / 1_000_000_000.0
        if (elapsed <= 0.0) return 0.0
        return (2.0 * iterations) / elapsed
    }
}
