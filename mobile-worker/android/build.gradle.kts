// Root build script for the AI Upcycler Android worker agent.
//
// The worker is the on-device half of the pooled cluster: it advertises itself
// over mDNS and serves the `aiupcycler.rpc.WorkerService` contract defined in
// the repository's canonical `proto/cluster_rpc.proto`.
plugins {
    id("com.android.application") version "8.5.2" apply false
    id("org.jetbrains.kotlin.android") version "1.9.24" apply false
    id("com.google.protobuf") version "0.9.4" apply false
}

// The worker needs a Java-friendly proto source dir, so `proto/cluster_rpc.proto`
// is vendored into `app/src/main/proto/`. This guard fails the build if the two
// copies ever diverge, keeping the canonical contract the single source of truth.
val canonicalProto = rootDir.resolve("../../proto/cluster_rpc.proto").normalize()
val vendoredProto = rootDir.resolve("app/src/main/proto/cluster_rpc.proto").normalize()

tasks.register("checkProtoInSync") {
    description = "Fails if the vendored worker proto drifts from the canonical proto/."
    group = "verification"
    doLast {
        if (!canonicalProto.exists()) {
            throw GradleException("Canonical proto not found: $canonicalProto")
        }
        if (canonicalProto.readText() != vendoredProto.readText()) {
            throw GradleException(
                "app/src/main/proto/cluster_rpc.proto is out of sync with proto/cluster_rpc.proto. " +
                    "Copy the canonical file over the vendored copy.",
            )
        }
    }
}
