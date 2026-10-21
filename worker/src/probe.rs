//! On-demand hardware self-measurement (spec 2026-10-21, D-C2).
//!
//! Pure, synchronous, bounded work: a sequential-memory pass over a fixed
//! buffer and a single-threaded f32 fused multiply-add loop. No I/O, no
//! thread spawning, no timing loops that run "until confident" — the sizes
//! and pass counts are constants, so total wall time is roughly half a second
//! by construction and the worker stays responsive for serving throughout.

/// Elements in the bandwidth probe buffer (32 MiB of `f32`, D-C2: conserva-
/// tive for a 6–8 GB phone; the number feeds relative comparison, not marketing).
pub const BANDWIDTH_BUFFER_ELEMENTS: usize = 8 * 1024 * 1024;

/// Sequential read+write passes over the buffer (one full write + one full
/// read each counts as 2 × buffer bytes of traffic).
pub const BANDWIDTH_PASSES: u32 = 2;

/// Fused multiply-add iterations in the compute probe.
pub const FLOPS_ITERATIONS: u64 = 64 * 1024 * 1024;

/// Sequential read+write throughput of `buffer`, in MB/s (megabytes/second).
///
/// The reduction is deliberately a dependent chain (each element adds the
/// previous result) so the compiler cannot vectorize it away into nothing;
/// `black_box` keeps the accumulation observable.
pub fn probe_bandwidth_mbs(buffer: &mut [f32], passes: u32) -> f64 {
    let bytes_per_pass = 2.0 * std::mem::size_of_val(buffer) as f64;
    let started = std::time::Instant::now();
    let mut sink = 0.0f32;
    for _ in 0..passes {
        for value in buffer.iter_mut() {
            *value = std::hint::black_box(*value) + 1.0;
            sink += *value;
        }
        for value in buffer.iter() {
            sink += std::hint::black_box(*value);
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    std::hint::black_box(sink);
    if elapsed <= 0.0 {
        return 0.0;
    }
    (bytes_per_pass * passes as f64) / elapsed / 1_000_000.0
}

/// Single-threaded f32 fused multiply-add rate, in FLOP/s (2 FLOPs per FMA).
///
/// The chain is data-dependent (`b` feeds the next multiply) so it cannot be
/// constant-folded; `black_box` on the inputs and output keeps it observable.
pub fn probe_throughput_flops(iterations: u64) -> f64 {
    let started = std::time::Instant::now();
    let mut a = 1.000_001f32;
    let mut b = 0.999_999f32;
    for i in 0..iterations {
        let scale = 1.0 + ((i % 7) as f32) * 1e-6;
        a = std::hint::black_box(a).mul_add(std::hint::black_box(b), scale);
        b = std::hint::black_box(b) * 0.999_999_5 + 1e-9;
    }
    std::hint::black_box((a, b));
    let elapsed = started.elapsed().as_secs_f64();
    if elapsed <= 0.0 {
        return 0.0;
    }
    (2.0 * iterations as f64) / elapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    // The values themselves are hardware-dependent (and thus only asserted on
    // physical devices, D-C9-style); the device-free contract is shape: the
    // sizes are the spec's bounded constants and every measurement is a
    // positive, finite f64.

    #[test]
    fn bandwidth_probe_uses_the_bounded_spec_buffer() {
        assert_eq!(BANDWIDTH_BUFFER_ELEMENTS * 4, 32 * 1024 * 1024);
        const { assert!(BANDWIDTH_PASSES >= 1) };
    }

    #[test]
    fn bandwidth_probe_returns_a_positive_finite_measurement() {
        let mut buffer = vec![1.0f32; BANDWIDTH_BUFFER_ELEMENTS];
        let mbps = probe_bandwidth_mbs(&mut buffer, BANDWIDTH_PASSES);
        assert!(mbps.is_finite() && mbps > 0.0, "bandwidth {mbps}");
        assert!(buffer.iter().all(|v| *v > 1.0), "buffer was written");
    }

    /// The engine dispatches the probe through `tokio::task::spawn_blocking`
    /// (`WorkerEngine::benchmark`) so the CPU-bound work never occupies an
    /// async worker thread. This pins the property that motivates it: an async
    /// task keeps making progress on the same runtime while a probe runs on
    /// the blocking pool. A single-thread runtime is the strict case — if the
    /// probe blocked the runtime, the ticker could not advance at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_probe_on_the_blocking_pool_does_not_starve_the_runtime() {
        let probe = tokio::task::spawn_blocking(|| {
            let mut buffer = vec![1.0f32; BANDWIDTH_BUFFER_ELEMENTS];
            probe_bandwidth_mbs(&mut buffer, BANDWIDTH_PASSES)
        });

        // The ticker shares the runtime with the probe dispatch.
        let mut ticks = 0u32;
        while !probe.is_finished() {
            tokio::task::yield_now().await;
            ticks += 1;
            if ticks > 1_000_000 {
                break; // the probe is slower than 1e6 yields: still "did not starve"
            }
        }

        let mbps = probe.await.expect("probe task panicked");
        assert!(mbps.is_finite() && mbps > 0.0, "bandwidth {mbps}");
        assert!(ticks > 0, "runtime made no progress while the probe ran");
    }

    #[test]
    fn bandwidth_probe_reports_zero_for_a_degenerate_clock() {
        // Zero passes => zero elapsed guard must not divide by zero.
        let mut buffer = vec![1.0f32; 1024];
        assert_eq!(probe_bandwidth_mbs(&mut buffer, 0), 0.0);
    }

    #[test]
    fn flops_probe_returns_a_positive_finite_measurement() {
        let flops = probe_throughput_flops(FLOPS_ITERATIONS);
        assert!(flops.is_finite() && flops > 0.0, "flops {flops}");
    }

    #[test]
    fn flops_probe_reports_zero_iterations_as_zero() {
        assert_eq!(probe_throughput_flops(0), 0.0);
    }
}
