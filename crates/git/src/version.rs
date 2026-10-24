//! Git version and minimum-version check.

use std::fmt;

/// Git version, read from `git --version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// Minimum supported version: needed, for example, by `rev-parse --end-of-options` (2.30).
    pub const MIN: Self = Self::new(2, 30, 0);

    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses the output of `git --version`, e.g. `git version 2.43.0`, also with suffixes like
    /// `2.45.1.windows.1`, `2.39.3 (Apple Git-146)` o `2.50.0.rc1`. The patch, if missing, is 0.
    pub fn parse(output: &str) -> Option<Self> {
        let version = output.trim().strip_prefix("git version ")?;
        let version = version.split_whitespace().next()?;
        let mut parts = version.split('.').map(leading_number);
        let major = parts.next()??;
        let minor = parts.next()??;
        let patch = parts.next().flatten().unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }

    /// True if the version is at least [`GitVersion::MIN`].
    pub fn is_supported(self) -> bool {
        self >= Self::MIN
    }
}

/// The leading digits of `part` (`"0-rc1"` → 0); `None` if it does not start with a digit.
fn leading_number(part: &str) -> Option<u32> {
    let end = part
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(part.len());
    part[..end].parse().ok()
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_outputs() {
        let cases = [
            ("git version 2.43.0\n", GitVersion::new(2, 43, 0)),
            ("git version 2.45.1.windows.1", GitVersion::new(2, 45, 1)),
            (
                "git version 2.39.3 (Apple Git-146)",
                GitVersion::new(2, 39, 3),
            ),
            ("git version 2.50.0.rc1", GitVersion::new(2, 50, 0)),
            ("git version 2.30.0-rc2", GitVersion::new(2, 30, 0)),
            ("git version 3.0", GitVersion::new(3, 0, 0)),
        ];
        for (output, expected) in cases {
            assert_eq!(GitVersion::parse(output), Some(expected), "{output:?}");
        }
    }

    #[test]
    fn rejects_unknown_outputs() {
        for output in [
            "",
            "git version",
            "git version x.y",
            "hg version 2.1",
            "git version 2",
        ] {
            assert_eq!(GitVersion::parse(output), None, "{output:?}");
        }
    }

    #[test]
    fn minimum_is_2_30() {
        assert!(GitVersion::new(2, 30, 0).is_supported());
        assert!(GitVersion::new(2, 43, 0).is_supported());
        assert!(GitVersion::new(3, 0, 0).is_supported());
        assert!(!GitVersion::new(2, 29, 9).is_supported());
        assert!(!GitVersion::new(1, 99, 0).is_supported());
        assert_eq!(GitVersion::MIN.to_string(), "2.30.0");
    }
}
