// SPDX-License-Identifier: GPL-3.0-or-later
//! Secret scanning before push, and the patterns used to redact secrets from
//! logs and reports. A full secret value is never shown anywhere.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const PATTERNS: [(&str, &str); 7] = [
    ("GitHub token", r"ghp_[A-Za-z0-9]{36}"),
    ("GitHub fine-grained token", r"github_pat_[A-Za-z0-9_]{50,}"),
    ("API key (sk-)", r"sk-[A-Za-z0-9_-]{20,}"),
    ("AWS access key", r"AKIA[0-9A-Z]{16}"),
    ("Google API key", r"AIza[0-9A-Za-z_-]{35}"),
    ("Slack token", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    ("Private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
];

fn compiled() -> &'static Vec<(&'static str, Regex)> {
    static R: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    R.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|(n, p)| (*n, Regex::new(p).expect("valid regex")))
            .collect()
    })
}

fn extra_redactions() -> &'static Vec<Regex> {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        vec![
            Regex::new(r"(?i)bearer\s+\S+").unwrap(),
            Regex::new(r"(?i)token=\S+").unwrap(),
            Regex::new(r"(?i)(gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}").unwrap(),
        ]
    })
}

/// Replace every secret-looking value with `[REDACTED]`.
pub fn redact_secrets(text: &str) -> String {
    let mut s = text.to_string();
    for (_, re) in compiled() {
        s = re.replace_all(&s, "<secret>").into_owned();
    }
    for re in extra_redactions() {
        s = re.replace_all(&s, "<secret>").into_owned();
    }
    s
}

/// First four characters and an ellipsis.
pub fn mask(value: &str) -> String {
    let head: String = value.chars().take(4).collect();
    format!("{head}…")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Project-relative path.
    pub file: String,
    pub line: usize,
    pub kind: String,
    /// Masked value, safe to show.
    pub masked: String,
    /// Stable id: file + line + hash of the line (for "Not a secret").
    pub id: String,
}

fn line_hash(line: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(line.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Scan one file's text.
pub fn scan_text(rel: &str, text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for (kind, re) in compiled() {
            for m in re.find_iter(line) {
                hits.push(Hit {
                    file: rel.to_string(),
                    line: i + 1,
                    kind: kind.to_string(),
                    masked: mask(m.as_str()),
                    id: format!("{rel}:{}:{}", i + 1, line_hash(line)),
                });
            }
        }
    }
    hits
}

fn is_example_env(rel: &str) -> bool {
    let name = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    name == ".env.example" || name.ends_with(".example") || name.ends_with(".sample")
}

/// Scan the given files (project-relative paths, usually the files git would
/// commit). Skips binaries, files over 2 MB and example env files.
pub fn scan_files(root: &Path, files: &[PathBuf]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for rel in files {
        let rel_s = rel.to_string_lossy().to_string();
        if rel_s.starts_with(".git/") || is_example_env(&rel_s) {
            continue;
        }
        let p = root.join(rel);
        let meta = match std::fs::metadata(&p) {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        if meta.len() > 2 * 1024 * 1024 {
            continue;
        }
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes);
        hits.extend(scan_text(&rel_s, &text));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_ghp() -> String {
        format!(
            "ghp_{}",
            "A1b2C3d4E5".repeat(4).chars().take(36).collect::<String>()
        )
    }

    #[test]
    fn finds_and_masks() {
        let tok = fake_ghp();
        let hits = scan_text("src/config.js", &format!("const t = \"{tok}\";\n"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].masked, "ghp_…");
        assert_eq!(hits[0].line, 1);
        assert!(!format!("{:?}", hits[0]).contains(&tok[4..]));
    }

    #[test]
    fn all_patterns_match_samples() {
        let samples = [
            fake_ghp(),
            format!("github_pat_{}", "x".repeat(60)),
            format!("sk-{}", "a".repeat(30)),
            // Built at run time so StowLock's own source never trips its scanner.
            format!("AKIA{}", "ABCDEFGHIJKLMNOP"),
            format!("AIza{}", "b".repeat(35)),
            format!("xox{}-1234567890-abc", "b"),
            format!("-----BEGIN RSA {}-----", "PRIVATE KEY"),
        ];
        for s in samples {
            assert_eq!(scan_text("f", &s).len(), 1, "no match for {}", mask(&s));
            assert!(!redact_secrets(&s).contains(&s));
        }
    }

    #[test]
    fn skips_examples_and_binaries() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".env.example"), format!("KEY={}", fake_ghp())).unwrap();
        let mut bin = vec![0u8, 1, 2];
        bin.extend(fake_ghp().as_bytes());
        std::fs::write(d.path().join("blob.bin"), bin).unwrap();
        let hits = scan_files(d.path(), &[".env.example".into(), "blob.bin".into()]);
        assert!(hits.is_empty());
    }

    #[test]
    fn redacts_bearer_and_token_params() {
        let s = redact_secrets("Authorization: Bearer abc123 url?token=zzz");
        assert!(!s.contains("abc123") && !s.contains("zzz"));
    }
}

#[cfg(test)]
mod own_source {
    /// StowLock's own files must pass its own secret scan (a test once held
    /// a token-shaped string and blocked the push of a release).
    #[test]
    fn stowlocks_own_source_has_no_secrets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = vec![];
        for e in walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            n != "target" && n != ".git"
        }) {
            let e = e.unwrap();
            if e.file_type().is_file() {
                files.push(e.path().strip_prefix(root).unwrap().to_path_buf());
            }
        }
        let hits = super::scan_files(root, &files);
        assert!(
            hits.is_empty(),
            "token-shaped text in StowLock's own files: {hits:?}"
        );
    }
}
