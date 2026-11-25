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

/// The project's notes for `tag` from its CHANGELOG (CHANGELOG.md, CHANGES.md,
/// HISTORY.md, any case), used as the GitHub release notes. A heading like
/// "## [0.2.0] - 2026-11-24" or "## v0.2.0" starts the section; it ends at the
/// next heading of the same or a higher level.
pub fn changelog_notes(dir: &std::path::Path, tag: &str) -> Option<String> {
    let version = tag.trim().trim_start_matches('v');
    if version.is_empty() {
        return None;
    }
    let file = std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        matches!(
            n.as_str(),
            "changelog.md" | "changes.md" | "history.md" | "changelog"
        )
        .then(|| e.path())
    })?;
    let meta = std::fs::metadata(&file).ok()?;
    if meta.len() > 2_000_000 {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let ver_re = regex::Regex::new(&format!(
        r"(^|[^0-9A-Za-z.])v?{}($|[^0-9A-Za-z.\-])",
        regex::escape(version)
    ))
    .ok()?;
    let mut level = 0usize;
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let is_heading = hashes > 0 && line[hashes..].starts_with(' ');
        if level == 0 {
            if is_heading && ver_re.is_match(&line[hashes..]) {
                level = hashes;
            }
            continue;
        }
        if is_heading && hashes <= level {
            break;
        }
        out.push(line);
    }
    let notes = out.join("\n").trim().to_string();
    if notes.is_empty() {
        return None;
    }
    let notes: String = notes.chars().take(20_000).collect();
    Some(notes)
}

#[cfg(test)]
mod changelog_tests {
    use super::*;

    #[test]
    fn finds_the_section_for_a_version() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n- wip\n\n## [0.1.0-beta.4] - 2026-11-24\n### Added\n- Push review step\n\n### Fixed\n- Updates work for private repos\n\n## [0.1.0] - 2026-10-01\n- First\n",
        )
        .unwrap();
        let n = changelog_notes(d.path(), "v0.1.0-beta.4").unwrap();
        assert!(n.starts_with("### Added"));
        assert!(n.contains("Updates work for private repos"));
        assert!(!n.contains("First"), "stops at the next version");
        // 0.1.0 must not match the 0.1.0-beta.4 heading.
        assert_eq!(
            changelog_notes(d.path(), "v0.1.0").as_deref(),
            Some("- First")
        );
        assert_eq!(changelog_notes(d.path(), "v9.9.9"), None);
    }

    #[test]
    fn no_changelog_means_no_notes() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(changelog_notes(d.path(), "v1.0.0"), None);
        std::fs::write(d.path().join("changes.md"), "## v1.0.0\n\n").unwrap();
        assert_eq!(
            changelog_notes(d.path(), "v1.0.0"),
            None,
            "an empty section"
        );
    }
}
