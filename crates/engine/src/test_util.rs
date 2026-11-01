//! Helpers shared by the engine's unit tests.

use project_domain::Oid;
use serde_json::Value;

use crate::MemoryBackend;

/// The fake id of seed `n` ([`MemoryBackend::fake_oid`]).
pub(crate) fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

/// Reads `out` as `T` and checks that `T` gives the same JSON again: the UI deserializes the
/// command results into these types.
pub(crate) fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) -> T {
    let typed: T = serde_json::from_value(out.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), out);
    typed
}
