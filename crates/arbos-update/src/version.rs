//! What "newer" means here.
//!
//! Two numbers, not one. `0.2.0` is the marketing version out of
//! `Cargo.toml`, and it does not move for weeks at a time; the build number is
//! the count of commits on the branch, and it moves with every merge. The dev
//! channel publishes a payload per green commit on `main`, so without the
//! second number every one of them would look like the one before it.
//!
//! It is the pair the bundle already carries — `CFBundleShortVersionString`
//! and `CFBundleVersion`, stamped by `desktop/Makefile` — so nothing new is
//! being invented for the feed to talk about.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt};

/// A release, ordered.
///
/// Written `0.2.0+1877` where it has to survive a round trip, and
/// `0.2.0 (1877)` where a person reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The commit count on the branch the payload was built from. `0` where
    /// the build had no repository to count — a tarball build, a vendored
    /// tree — which sorts below every build that did.
    pub build: u64,
    /// `rc1` in `0.2.0-rc1`. A pre-release sorts *below* the release of the
    /// same three numbers, which is the one place where "more text means
    /// less version" and so the one place this cannot derive its ordering.
    pub pre: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64, build: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            build,
            pre: None,
        }
    }

    /// `0.2.0`, `v0.2.0`, `0.2.0-rc1`, `0.2.0+1877`, `0.2.0-rc1+1877`.
    ///
    /// A leading `v` is taken because that is how the tags are written, and a
    /// tag is one of the two places a version is read from.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        if text.is_empty() {
            bail!("empty version");
        }
        let (text, build) = match text.split_once('+') {
            Some((head, build)) => (
                head,
                build
                    .parse::<u64>()
                    .with_context(|| format!("build in `{text}` is not a number"))?,
            ),
            None => (text, 0),
        };
        let (numbers, pre) = match text.split_once('-') {
            Some((head, pre)) if !pre.is_empty() => (head, Some(pre.to_owned())),
            Some(_) => bail!("empty pre-release in `{text}`"),
            None => (text, None),
        };
        let mut parts = numbers.split('.');
        let mut number = |what: &str| -> Result<u64> {
            parts
                .next()
                .with_context(|| format!("no {what} in `{text}`"))?
                .parse::<u64>()
                .with_context(|| format!("{what} in `{text}` is not a number"))
        };
        let major = number("major")?;
        let minor = number("minor")?;
        let patch = number("patch")?;
        if parts.next().is_some() {
            bail!("`{text}` has more than three numbers");
        }
        Ok(Self {
            major,
            minor,
            patch,
            build,
            pre,
        })
    }

    /// The version with a build number put on it. The app knows its own two
    /// halves from two different compile-time stamps, so it joins them here
    /// rather than formatting a string and parsing it back.
    pub fn with_build(mut self, build: u64) -> Self {
        self.build = build;
        self
    }

    /// How a person reads it: `0.2.0 (1877)`, or just `0.2.0` where there is
    /// no build number to tell two of them apart.
    pub fn human(&self) -> String {
        let release = match &self.pre {
            Some(pre) => format!("{}.{}.{}-{pre}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        };
        match self.build {
            0 => release,
            build => format!("{release} ({build})"),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        if self.build != 0 {
            write!(f, "+{}", self.build)?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                // A release beats its own pre-releases. `Option`'s own order
                // says the opposite, which is why this is written out.
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(ours), Some(theirs)) => ours.cmp(theirs),
            })
            .then_with(|| self.build.cmp(&other.build))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn parses_the_shapes_a_tag_and_a_bundle_are_written_in() {
        assert_eq!(Version::parse("0.2.0").unwrap(), Version::new(0, 2, 0, 0));
        assert_eq!(Version::parse("v0.2.0").unwrap(), Version::new(0, 2, 0, 0));
        assert_eq!(
            Version::parse(" 0.2.0+1877 ").unwrap(),
            Version::new(0, 2, 0, 1877)
        );
        assert_eq!(
            Version::parse("0.2.0-rc1").unwrap().pre.as_deref(),
            Some("rc1")
        );
        assert_eq!(Version::parse("0.2.0-rc1+9").unwrap().build, 9);
    }

    #[test]
    fn refuses_what_it_cannot_order() {
        for bad in ["", "0.2", "0.2.0.1", "0.two.0", "0.2.0+x", "0.2.0-"] {
            assert!(Version::parse(bad).is_err(), "`{bad}` should not parse");
        }
    }

    #[test]
    fn the_build_number_breaks_a_tie() {
        // The whole reason the dev channel works: `0.2.0` does not move for
        // weeks, so without this every commit on main looks like the last.
        assert!(Version::new(0, 2, 0, 1878) > Version::new(0, 2, 0, 1877));
        assert!(Version::new(0, 2, 0, 1877) == Version::new(0, 2, 0, 1877));
    }

    #[test]
    fn the_release_numbers_outrank_the_build_number() {
        // A tagged 0.3.0 cut from an older branch still beats a dev build of
        // 0.2.0 with a far higher commit count.
        assert!(Version::new(0, 3, 0, 5) > Version::new(0, 2, 0, 9999));
        assert!(Version::new(1, 0, 0, 0) > Version::new(0, 99, 99, 9999));
    }

    #[test]
    fn a_release_beats_its_own_pre_releases() {
        let rc = Version::parse("0.3.0-rc2+10").unwrap();
        let out = Version::parse("0.3.0+1").unwrap();
        assert!(out > rc);
        assert!(rc > Version::parse("0.3.0-rc1+99").unwrap());
        assert!(rc > Version::parse("0.2.9+9999").unwrap());
    }

    #[test]
    fn a_build_with_no_repository_sorts_under_one_that_had_one() {
        assert!(Version::new(0, 2, 0, 1) > Version::new(0, 2, 0, 0));
    }

    #[test]
    fn writes_itself_back_the_way_it_was_read() {
        for text in ["0.2.0", "0.2.0+1877", "0.3.0-rc1", "0.3.0-rc1+12"] {
            assert_eq!(Version::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(
            Version::parse("0.2.0+1877").unwrap().human(),
            "0.2.0 (1877)"
        );
        assert_eq!(Version::parse("0.2.0").unwrap().human(), "0.2.0");
    }
}
