//! ADB USB tunnel manager.
//!
//! Bridges Android worker devices into the master node's localhost namespace via
//! `adb forward tcp:50052 tcp:50052` so the RPC client can talk to them as if
//! they were loopback endpoints.