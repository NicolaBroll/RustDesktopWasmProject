//! Tags (`tag.create`): checks shared by the backends (`impl-docs/git/branch.md`, "Tags").
//!
//! The names passed to the commands are **short** (`v1.0`, not `refs/tags/v1.0`); the full name
//! is built by [`tag_ref`].

use super::valid_ref_name;
use crate::EngineError;

/// Prefix of tags.
pub const TAGS_PREFIX: &str = "refs/tags/";

/// The full name of the tag `name` (`refs/tags/<name>`).
pub fn tag_ref(name: &str) -> String {
    format!("{TAGS_PREFIX}{name}")
}

/// The name of a **new** tag: not empty, short (without `refs/`), not starting with `-` and such
/// that `refs/tags/<name>` is a valid ref name ([`valid_ref_name`]). Otherwise
/// [`EngineError::InvalidParams`].
pub fn check_new_tag_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    let reason = if name.is_empty() {
        Some(msg!("engine-tag-name-empty"))
    } else if name.starts_with("refs/") {
        Some(msg!("engine-tag-name-full"))
    } else if name.starts_with('-') {
        Some(msg!("engine-tag-name-dash"))
    } else if !valid_ref_name(&tag_ref(name)) {
        Some(msg!("engine-tag-name-invalid"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: msg!(
                "engine-with-name",
                reason = reason,
                name = name.escape_debug().to_string()
            ),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_names() {
        for good in ["v1.0", "release/2026-10", "x"] {
            assert!(check_new_tag_name("tag.create", good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "refs/tags/v1",
            "-v1",
            "a b",
            "a..b",
            "v1.lock",
            "a~1",
            "a:b",
        ] {
            assert!(check_new_tag_name("tag.create", bad).is_err(), "{bad}");
        }
    }
}
