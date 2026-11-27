// SPDX-License-Identifier: GPL-3.0-or-later
//! Repo-name slugs, stack detection, .gitignore templates, default run
//! commands and the starter README for new repos.

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stack {
    Rust,
    Node,
    Python,
    Go,
    Other,
}

impl Stack {
    pub fn name(&self) -> &'static str {
        match self {
            Stack::Rust => "Rust",
            Stack::Node => "Node",
            Stack::Python => "Python",
            Stack::Go => "Go",
            Stack::Other => "Other",
        }
    }
}

fn suffix_patterns() -> &'static Vec<Regex> {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        [
            r"(?i)[\s_-]*\((v?\d+(\.\d+)*)\)$",
            r"(?i)[\s_-]+v?\d+(\.\d+)+([\s._-]?(beta|alpha|rc)([.\-]?\d+)?)?$",
            r"(?i)[\s_-]+v\d+$",
            r"(?i)[\s_-]+(main|master|beta|final|backup|latest|source|src|code|project|full|complete|files)$",
            r"(?i)[\s_-]+\d{4}-\d{2}-\d{2}([\s_-]\d{4,6})?$",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

/// Lowercase letters, digits and single hyphens; max 100 characters.
pub fn kebab(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let t = out.trim_matches('-').to_string();
    t.chars()
        .take(100)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Suggested repo slug from a zip file name, with version/snapshot suffixes
/// removed: `sluice-v2.zip` → `sluice`, `My App (1).zip` → `my-app`.
pub fn slug_from_filename(file_name: &str) -> String {
    let mut stem = file_name.trim().to_string();
    if stem.to_ascii_lowercase().ends_with(".zip") {
        stem.truncate(stem.len() - 4);
    }
    loop {
        let before = stem.clone();
        for re in suffix_patterns() {
            stem = re.replace(&stem, "").to_string();
        }
        if stem == before {
            break;
        }
    }
    let s = kebab(&stem);
    if s.is_empty() {
        kebab(file_name.trim_end_matches(".zip"))
    } else {
        s
    }
}

/// The existing project a zip belongs to, when its name starts with one
/// ("gimbot-fixed-fonts" or "Gimbot_house_standard" → "Gimbot"). The longest
/// match wins, and the existing name keeps its exact spelling.
pub fn existing_match<'a>(slug: &str, known: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let s = slug.to_ascii_lowercase();
    known
        .into_iter()
        .filter(|k| !k.is_empty())
        .filter(|k| {
            let k = k.to_ascii_lowercase();
            s == k || s.strip_prefix(&k).is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|k| k.len())
        .map(str::to_string)
}

/// Valid as a GitHub repository name: 1–100 of letters, digits, `.`, `_`
/// and `-`, and not `.` or `..`. Capitals are allowed (existing repos keep
/// their exact names); new names are formatted with `format_typing`/`kebab`.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Why a name can't be used, in plain words (None = fine).
pub fn name_problem(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        Some("Enter a name for the repo.".into())
    } else if s.len() > 100 {
        Some(format!(
            "The name is {} characters; GitHub allows at most 100.",
            s.len()
        ))
    } else if !valid_slug(s) {
        Some("GitHub names can only use letters, numbers, hyphens, dots and underscores.".into())
    } else {
        None
    }
}

/// Format a new repo name while it's being typed: lowercase, spaces and
/// underscores become hyphens, other characters are dropped, repeated
/// hyphens collapse. A trailing hyphen is kept so typing can continue.
pub fn format_typing(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c == ' ' || c == '_' {
            '-'
        } else {
            c.to_ascii_lowercase()
        };
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c);
        } else if c == '-' && !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.chars().take(100).collect()
}

/// The finished form of a typed name (no leading/trailing hyphens or dots).
pub fn finish_name(s: &str) -> String {
    format_typing(s)
        .trim_matches(|c| c == '-' || c == '.')
        .to_string()
}

/// Find a folder in `parent` whose name matches `name` ignoring case.
pub fn find_dir_ci(parent: &Path, name: &str) -> Option<String> {
    if parent.join(name).is_dir() {
        return Some(name.to_string());
    }
    std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.eq_ignore_ascii_case(name))
}

/// The project folder for a repo name: an existing folder with the same name
/// in any capitalisation, else `<workspace>/<name>`.
pub fn project_dir(workspace: &Path, name: &str) -> std::path::PathBuf {
    match find_dir_ci(workspace, name) {
        Some(n) => workspace.join(n),
        None => workspace.join(name),
    }
}

/// A version found for a zip, and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundVersion {
    pub tag: String,
    pub source: String,
}

fn full_version_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[\s_-])v?(\d+\.\d+\.\d+(?:-[0-9a-z]+(?:\.[0-9a-z]+)*)?)$").unwrap()
    })
}
fn short_version_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)[\s_-](v\d+(?:\.\d+)?)$").unwrap())
}
fn py_version_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?m)^\s*(?:__version__|version)\s*=\s*['"]([^'"]+)['"]"#).unwrap()
    })
}

fn zip_stem(file_name: &str) -> String {
    let mut stem = file_name.trim().to_string();
    if stem.to_ascii_lowercase().ends_with(".zip") {
        stem.truncate(stem.len() - 4);
    }
    // "name (1)" download duplicates
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"\s*\(\d+\)$").unwrap());
    re.replace(&stem, "").trim().to_string()
}

fn is_semverish(v: &str) -> bool {
    semver::Version::parse(v.trim().trim_start_matches('v')).is_ok()
}

/// Read a project's version from the files inside a zip, without extracting
/// it. Only small text files at the top level or inside one wrapper folder.
pub fn version_in_zip(zip: &Path) -> Option<(String, String)> {
    let file = std::fs::File::open(zip).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut candidates: Vec<(usize, String, usize)> = Vec::new(); // (priority, name, index)
    for i in 0..archive.len().min(50_000) {
        let Ok(e) = archive.by_index_raw(i) else {
            continue;
        };
        if e.is_dir() || e.size() > 1024 * 1024 {
            continue;
        }
        let Some(path) = e.enclosed_name() else {
            continue;
        };
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if parts.first().map(|p| p == "__MACOSX").unwrap_or(false) {
            continue;
        }
        let depth = parts.len();
        let base = parts.last().cloned().unwrap_or_default();
        let prio = match base.as_str() {
            "Cargo.toml" => 0,
            "package.json" => 1,
            "pyproject.toml" => 2,
            "setup.py" => 3,
            "__init__.py" => 4,
            "VERSION" | "VERSION.txt" => 5,
            _ => continue,
        };
        let allowed = depth == 1 || depth == 2 || (base == "__init__.py" && depth <= 3);
        if allowed {
            candidates.push((prio * 10 + depth, base, i));
        }
    }
    candidates.sort();
    for (_, base, i) in candidates {
        let mut text = String::new();
        {
            use std::io::Read;
            let Ok(mut e) = archive.by_index(i) else {
                continue;
            };
            if e.by_ref()
                .take(1024 * 1024)
                .read_to_string(&mut text)
                .is_err()
            {
                continue;
            }
        }
        let found = match base.as_str() {
            "Cargo.toml" => toml::from_str::<toml::Value>(&text).ok().and_then(|v| {
                v.get("package")?
                    .get("version")?
                    .as_str()
                    .map(|s| s.to_string())
            }),
            "package.json" => serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("version")?.as_str().map(|s| s.to_string())),
            "pyproject.toml" => toml::from_str::<toml::Value>(&text).ok().and_then(|v| {
                v.get("project")
                    .and_then(|p| p.get("version"))
                    .or_else(|| v.get("tool")?.get("poetry")?.get("version"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            }),
            "setup.py" | "__init__.py" => py_version_re().captures(&text).map(|c| c[1].to_string()),
            _ => text.lines().next().map(|l| l.trim().to_string()),
        };
        if let Some(v) = found {
            if is_semverish(&v) {
                return Some((v.trim().trim_start_matches('v').to_string(), base));
            }
        }
    }
    None
}

/// Work out a zip's version: full version in the file name, then the
/// project's own version file, then a short version in the name.
pub fn detect_version(zip: &Path, prefix: &str) -> Option<FoundVersion> {
    let name = zip
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = zip_stem(&name);
    if let Some(c) = full_version_re().captures(&stem) {
        return Some(FoundVersion {
            tag: format!("{prefix}{}", &c[1]),
            source: "the file name".into(),
        });
    }
    if let Some((v, file)) = version_in_zip(zip) {
        return Some(FoundVersion {
            tag: format!("{prefix}{v}"),
            source: file,
        });
    }
    if let Some(c) = short_version_re().captures(&stem) {
        return Some(FoundVersion {
            tag: c[1].to_ascii_lowercase(),
            source: "the file name".into(),
        });
    }
    None
}

pub fn detect_stack(dir: &Path) -> Stack {
    if dir.join("Cargo.toml").is_file() {
        return Stack::Rust;
    }
    if dir.join("package.json").is_file() {
        return Stack::Node;
    }
    if dir.join("go.mod").is_file() {
        return Stack::Go;
    }
    if dir.join("pyproject.toml").is_file()
        || dir.join("requirements.txt").is_file()
        || dir.join("setup.py").is_file()
    {
        return Stack::Python;
    }
    let has_py = walkdir::WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .flatten()
        .any(|e| {
            e.path().extension().map(|x| x == "py").unwrap_or(false)
                && !e.path().components().any(|c| c.as_os_str() == ".git")
        });
    if has_py {
        Stack::Python
    } else {
        Stack::Other
    }
}

const ENV_LINES: [&str; 3] = [".env", ".env.*", "!.env.example"];

pub fn gitignore_template(stack: Stack) -> String {
    let body = match stack {
        Stack::Rust => "/target/\n**/*.rs.bk\n*.pdb\n",
        Stack::Node => "node_modules/\ndist/\nbuild/\n.next/\ncoverage/\nnpm-debug.log*\nyarn-error.log*\n.pnpm-debug.log*\n",
        Stack::Python => "__pycache__/\n*.py[cod]\n.venv/\nvenv/\nenv/\n*.egg-info/\ndist/\nbuild/\n.pytest_cache/\n.mypy_cache/\n",
        Stack::Go => "/bin/\n*.exe\n*.test\n*.out\nvendor/\n",
        Stack::Other => "",
    };
    format!(
        "# Generated by StowLock\n{body}\n# OS junk\n.DS_Store\nThumbs.db\n__MACOSX/\n\n# Secrets: never commit these\n{}\n",
        ENV_LINES.join("\n")
    )
}

/// Append any missing `.env` rules to existing .gitignore text. Returns the new
/// text and whether anything changed.
pub fn add_env_rules(existing: &str) -> (String, bool) {
    let have: Vec<&str> = existing.lines().map(|l| l.trim()).collect();
    let missing: Vec<&str> = ENV_LINES
        .iter()
        .copied()
        .filter(|l| !have.contains(l))
        .collect();
    if missing.is_empty() {
        return (existing.to_string(), false);
    }
    let mut s = existing.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("\n# Secrets: never commit these (added by StowLock)\n");
    for m in missing {
        s.push_str(m);
        s.push('\n');
    }
    (s, true)
}

/// Default command for the Run button.
pub fn default_run_command(dir: &Path, stack: Stack) -> String {
    match stack {
        Stack::Rust => "cargo run".into(),
        Stack::Node => {
            let pkg = std::fs::read_to_string(dir.join("package.json")).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&pkg).unwrap_or_default();
            let scripts = v.get("scripts");
            if scripts.and_then(|s| s.get("start")).is_some() {
                "npm start".into()
            } else if scripts.and_then(|s| s.get("dev")).is_some() {
                "npm run dev".into()
            } else {
                "npm start".into()
            }
        }
        Stack::Python => {
            if dir.join("main.py").is_file() {
                "python3 main.py".into()
            } else if dir.join("app.py").is_file() {
                "python3 app.py".into()
            } else {
                "python3 main.py".into()
            }
        }
        Stack::Go => "go run .".into(),
        Stack::Other => String::new(),
    }
}

pub fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A short file tree, two levels deep, folders first.
pub fn file_tree(dir: &Path) -> String {
    fn list(d: &Path) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        if n == ".git" {
                            None
                        } else {
                            Some((n, e.path().is_dir()))
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        v
    }
    let mut out = String::from(".\n");
    let mut shown = 0;
    for (name, is_dir) in list(dir) {
        if shown >= 40 {
            out.push_str("└── …\n");
            break;
        }
        out.push_str(&format!("├── {name}{}\n", if is_dir { "/" } else { "" }));
        shown += 1;
        if is_dir {
            for (child, cdir) in list(&dir.join(&name)).into_iter().take(12) {
                out.push_str(&format!("│   ├── {child}{}\n", if cdir { "/" } else { "" }));
                shown += 1;
            }
        }
    }
    out
}

pub fn generate_readme(dir: &Path, slug: &str, description: &str, stack: Stack) -> String {
    let title = title_from_slug(slug);
    let run = default_run_command(dir, stack);
    let mut s = format!("# {title}\n\n{description}\n\n");
    if stack != Stack::Other {
        s.push_str(&format!("**Stack:** {}\n\n", stack.name()));
    }
    s.push_str("## Getting started\n\n");
    let setup = match stack {
        Stack::Node => "npm install\n",
        Stack::Python if dir.join("requirements.txt").is_file() => {
            "pip install -r requirements.txt\n"
        }
        _ => "",
    };
    if run.is_empty() {
        s.push_str("Open the files in this folder to get started.\n\n");
    } else {
        s.push_str(&format!("```bash\n{setup}{run}\n```\n\n"));
    }
    s.push_str("## Files\n\n```text\n");
    s.push_str(&file_tree(dir));
    s.push_str("```\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_from_filenames() {
        let known = ["Gimbot", "stream-preflight", "stream", "sluice"];
        let m = |zip: &str| existing_match(&slug_from_filename(zip), known);
        assert_eq!(
            m("gimbot-v0.4.2-house-standard.zip").as_deref(),
            Some("Gimbot")
        );
        assert_eq!(m("Gimbot_fixed_fonts.zip").as_deref(), Some("Gimbot"));
        assert_eq!(m("gimbot.zip").as_deref(), Some("Gimbot"));
        assert_eq!(
            m("stream-preflight-v1.2.zip").as_deref(),
            Some("stream-preflight")
        );
        assert_eq!(m("gimbotics.zip"), None, "a different name");
        // The real one.
        assert_eq!(
            m("Gimbot 0.1.0 beta.14 source.zip").as_deref(),
            Some("Gimbot")
        );
        assert_eq!(
            slug_from_filename("Gimbot 0.1.0 beta.14 source.zip"),
            "gimbot"
        );
        assert_eq!(slug_from_filename("my-app-v2.1-full-source.zip"), "my-app");
        assert_eq!(m("brand-new-app.zip"), None);
        assert_eq!(slug_from_filename("sluice-v2.zip"), "sluice");
        assert_eq!(slug_from_filename("My Cool App (v2).zip"), "my-cool-app");
        assert_eq!(slug_from_filename("My App (1).zip"), "my-app");
        assert_eq!(slug_from_filename("sluice-main.zip"), "sluice");
        assert_eq!(slug_from_filename("tool-2026-11-22.zip"), "tool");
        assert_eq!(
            slug_from_filename("json-diff-ui-v0.2.0-beta.1.zip"),
            "json-diff-ui"
        );
        assert_eq!(
            slug_from_filename("clip_sorter-final-backup.zip"),
            "clip-sorter"
        );
        assert_eq!(slug_from_filename("v2.zip"), "v2");
    }

    #[test]
    fn kebab_rules() {
        assert_eq!(kebab("  Hello__World!! "), "hello-world");
        assert!(valid_slug("my-app"));
        assert!(valid_slug("Sluice"), "existing repos keep their capitals");
        assert!(!valid_slug("My App"));
        assert!(!valid_slug(""));
        assert!(!valid_slug(".."));
    }

    #[test]
    fn typing_is_formatted_for_you() {
        assert_eq!(format_typing("My Cool App"), "my-cool-app");
        assert_eq!(
            format_typing("my "),
            "my-",
            "trailing hyphen kept while typing"
        );
        assert_eq!(format_typing("Snake_Case  Name!"), "snake-case-name");
        assert_eq!(finish_name("my-app-"), "my-app");
        assert!(name_problem("").is_some());
        assert!(name_problem(&"a".repeat(101)).is_some());
        assert!(name_problem("Sluice").is_none());
    }

    #[test]
    fn project_dir_ignores_case() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sluice")).unwrap();
        assert_eq!(project_dir(d.path(), "Sluice"), d.path().join("sluice"));
        assert_eq!(project_dir(d.path(), "new-one"), d.path().join("new-one"));
    }

    #[test]
    fn versions_from_zips() {
        let d = tempfile::tempdir().unwrap();
        let mk = |name: &str, entries: &[(&str, &[u8], u32)]| {
            let p = d.path().join(name);
            crate::extract::write_zip(&p, entries).unwrap();
            p
        };
        let a = mk("sluice-v0.11.7.zip", &[("x.txt", b"x", 0o644)]);
        assert_eq!(
            detect_version(&a, "v").unwrap(),
            FoundVersion {
                tag: "v0.11.7".into(),
                source: "the file name".into()
            }
        );
        let b = mk("tool-0.2.0-beta.1 (1).zip", &[("x.txt", b"x", 0o644)]);
        assert_eq!(detect_version(&b, "v").unwrap().tag, "v0.2.0-beta.1");
        let c = mk(
            "crate.zip",
            &[(
                "crate-main/Cargo.toml",
                b"[package]\nname = \"c\"\nversion = \"1.4.2\"\n",
                0o644,
            )],
        );
        assert_eq!(
            detect_version(&c, "v").unwrap(),
            FoundVersion {
                tag: "v1.4.2".into(),
                source: "Cargo.toml".into()
            }
        );
        let n = mk(
            "web.zip",
            &[(
                "package.json",
                b"{\"name\":\"w\",\"version\":\"3.0.1\"}",
                0o644,
            )],
        );
        assert_eq!(detect_version(&n, "v").unwrap().source, "package.json");
        let py = mk(
            "py.zip",
            &[(
                "pkg/pyproject.toml",
                b"[project]\nname='p'\nversion = '0.9.0'\n",
                0o644,
            )],
        );
        assert_eq!(detect_version(&py, "v").unwrap().tag, "v0.9.0");
        let init = mk(
            "lib.zip",
            &[("lib/mylib/__init__.py", b"__version__ = '2.1.0'\n", 0o644)],
        );
        assert_eq!(detect_version(&init, "v").unwrap().tag, "v2.1.0");
        let vf = mk("vf.zip", &[("VERSION", b"0.3.4\n", 0o644)]);
        assert_eq!(detect_version(&vf, "v").unwrap().source, "VERSION");
        let short = mk("sluice-v2.zip", &[("readme.md", b"x", 0o644)]);
        assert_eq!(detect_version(&short, "v").unwrap().tag, "v2");
        let both = mk(
            "app-v5.zip",
            &[("Cargo.toml", b"[package]\nversion = \"5.1.0\"\n", 0o644)],
        );
        assert_eq!(
            detect_version(&both, "v").unwrap().tag,
            "v5.1.0",
            "full version in a file beats a short name version"
        );
        let none = mk("notes.zip", &[("a.txt", b"x", 0o644)]);
        assert!(detect_version(&none, "v").is_none());
    }

    #[test]
    fn env_rules_appended_once() {
        let (a, changed) = add_env_rules("node_modules/\n");
        assert!(changed);
        assert!(a.contains(".env\n") && a.contains("!.env.example"));
        let (b, changed2) = add_env_rules(&a);
        assert!(!changed2);
        assert_eq!(a, b);
    }

    #[test]
    fn detects_stacks() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(detect_stack(d.path()), Stack::Other);
        std::fs::write(d.path().join("app.py"), "print(1)").unwrap();
        assert_eq!(detect_stack(d.path()), Stack::Python);
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_stack(d.path()), Stack::Node);
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_stack(d.path()), Stack::Rust);
    }
}
