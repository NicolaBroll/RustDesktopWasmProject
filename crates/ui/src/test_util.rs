//! Helpers shared by the UI's unit tests.

use project_domain::Oid;
use project_engine::MemoryBackend;

/// The fake id of seed `n` ([`MemoryBackend::fake_oid`]).
pub(crate) fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}
