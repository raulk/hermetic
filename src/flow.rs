//! Demo orchestration placeholder.
//!
//! M1 only proves the Tor-backed provider and sidecar boundary. The
//! shield/transfer/unshield orchestration lands after the Railgun SDK
//! feasibility checks close Q5 and Q6.

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DemoPhase {
    EventSync,
    Shield,
    Transfer,
    Unshield,
}
