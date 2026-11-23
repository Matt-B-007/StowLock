// SPDX-License-Identifier: GPL-3.0-or-later
//! Version tags: stable/beta suggestions and validation (semver).

use semver::{Prerelease, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn label(&self) -> &'static str {
        match self {
            Channel::Stable => "STABLE",
            Channel::Beta => "BETA",
        }
    }
    pub fn key(&self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }
}

pub fn parse_tag(tag: &str, prefix: &str) -> Option<Version> {
    let t = tag.trim();
    let t = t.strip_prefix(prefix).unwrap_or(t);
    let t = t.strip_prefix('v').unwrap_or(t);
    Version::parse(t).ok()
}

fn beta_number(v: &Version) -> Option<u64> {
    let p = v.pre.as_str();
    let rest = p.strip_prefix("beta")?;
    let rest = rest.trim_start_matches(['.', '-']);
    if rest.is_empty() {
        Some(0)
    } else {
        rest.parse().ok()
    }
}

/// Suggest the next tag from existing tags.
pub fn suggest(tags: &[String], channel: Channel, prefix: &str) -> String {
    let versions: Vec<Version> = tags.iter().filter_map(|t| parse_tag(t, prefix)).collect();
    let latest_stable = versions.iter().filter(|v| v.pre.is_empty()).max().cloned();
    let latest_any = versions.iter().max().cloned();
    let next_minor = |s: &Option<Version>| match s {
        Some(v) => Version::new(v.major, v.minor + 1, 0),
        None => Version::new(0, 1, 0),
    };
    // A beta newer than the last stable release, if any.
    let open_beta = latest_any
        .filter(|v| !v.pre.is_empty() && latest_stable.as_ref().map(|s| v > s).unwrap_or(true));
    let v = match channel {
        Channel::Stable => match &open_beta {
            Some(b) => Version::new(b.major, b.minor, b.patch),
            None => next_minor(&latest_stable),
        },
        Channel::Beta => match &open_beta {
            Some(b) if beta_number(b).is_some() => {
                let mut nv = Version::new(b.major, b.minor, b.patch);
                nv.pre = Prerelease::new(&format!("beta.{}", beta_number(b).unwrap() + 1)).unwrap();
                nv
            }
            Some(b) => {
                let mut nv = Version::new(b.major, b.minor, b.patch);
                nv.pre = Prerelease::new("beta.1").unwrap();
                nv
            }
            None => {
                let mut nv = next_minor(&latest_stable);
                nv.pre = Prerelease::new("beta.1").unwrap();
                nv
            }
        },
    };
    format!("{prefix}{v}")
}

/// Check a tag: valid semver after the prefix, matching the channel, unused.
pub fn validate(
    tag: &str,
    channel: Channel,
    prefix: &str,
    existing: &[String],
) -> Result<(), String> {
    let t = tag.trim();
    if t.is_empty() {
        return Ok(()); // no tag is fine
    }
    if t.contains(char::is_whitespace) {
        return Err("Tags can't contain spaces.".into());
    }
    if !prefix.is_empty() && !t.starts_with(prefix) {
        return Err(format!(
            "Tags should start with \"{prefix}\", e.g. {prefix}0.1.0."
        ));
    }
    let v = parse_tag(t, prefix)
        .ok_or_else(|| format!("\"{t}\" isn't a version number like {prefix}1.2.0."))?;
    match channel {
        Channel::Beta if v.pre.is_empty() => {
            return Err(format!(
                "A beta tag needs a beta label, e.g. {prefix}{v}-beta.1."
            ))
        }
        Channel::Stable if !v.pre.is_empty() => {
            return Err(
                "Stable tags can't have a label like -beta. Switch to Beta or remove it.".into(),
            )
        }
        _ => {}
    }
    if existing.iter().any(|e| e == t) {
        return Err(format!("The tag {t} already exists."));
    }
    Ok(())
}

pub fn default_commit_message(is_new: bool, tag: &str) -> String {
    if is_new {
        "Initial commit".into()
    } else if tag.trim().is_empty() {
        format!("Update {}", crate::util::today())
    } else {
        format!("Update to {}", tag.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest(&[], Channel::Stable, "v"), "v0.1.0");
        assert_eq!(suggest(&[], Channel::Beta, "v"), "v0.1.0-beta.1");
        assert_eq!(suggest(&t(&["v0.1.0"]), Channel::Stable, "v"), "v0.2.0");
        assert_eq!(
            suggest(&t(&["v0.1.0"]), Channel::Beta, "v"),
            "v0.2.0-beta.1"
        );
        assert_eq!(
            suggest(&t(&["v0.1.0", "v0.2.0-beta.1"]), Channel::Beta, "v"),
            "v0.2.0-beta.2"
        );
        assert_eq!(
            suggest(&t(&["v0.1.0", "v0.2.0-beta.3"]), Channel::Stable, "v"),
            "v0.2.0"
        );
        assert_eq!(
            suggest(&t(&["v0.2.0", "v0.2.0-beta.3"]), Channel::Beta, "v"),
            "v0.3.0-beta.1"
        );
        assert_eq!(
            suggest(&t(&["junk", "v1.4.2"]), Channel::Stable, "v"),
            "v1.5.0"
        );
    }

    #[test]
    fn validation() {
        let existing = t(&["v0.1.0"]);
        assert!(validate("", Channel::Stable, "v", &existing).is_ok());
        assert!(validate("v0.2.0", Channel::Stable, "v", &existing).is_ok());
        assert!(validate("v0.1.0", Channel::Stable, "v", &existing).is_err());
        assert!(validate("v0.2.0", Channel::Beta, "v", &existing).is_err());
        assert!(validate("v0.2.0-beta.1", Channel::Stable, "v", &existing).is_err());
        assert!(validate("0.2.0", Channel::Stable, "v", &existing).is_err());
        assert!(validate("v 1", Channel::Stable, "v", &existing).is_err());
        assert!(validate("v0.2.0-beta.1", Channel::Beta, "v", &existing).is_ok());
    }
}
