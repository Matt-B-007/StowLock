// SPDX-License-Identifier: GPL-3.0-or-later
//! Plain-English explanations for common failures. The raw text always stays
//! in the log and in bug reports.

#[derive(Debug, Clone, PartialEq)]
pub struct Friendly {
    pub message: String,
    /// A suggested action the UI can offer as a button.
    pub action: Option<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    SignIn,
    SetIdentity,
    SwitchToUpdate,
    Retry,
    BugReport,
}

pub fn explain(raw: &str, step: &str) -> Friendly {
    let r = raw.to_ascii_lowercase();
    let f = |m: &str, a: Option<Action>| Friendly {
        message: m.to_string(),
        action: a,
    };
    if r.contains("non-fast-forward")
        || r.contains("[rejected]")
        || r.contains("fetch first")
        || r.contains("diverged")
        || r.contains("not possible to fast-forward")
    {
        return f("GitHub has changes you don't have yet. StowLock pulls them first; if the histories have split, open the folder and merge by hand, or ask for help.", Some(Action::Retry));
    }
    if r.contains("repository not found")
        || r.contains("could not resolve to a repository")
        || r.contains("http 404")
    {
        return f(
            "Wrong account selected, or you don't have access to this repo.",
            None,
        );
    }
    if r.contains("authentication failed")
        || r.contains("http 401")
        || r.contains("not logged into")
        || r.contains("gh auth login")
        || r.contains("bad credentials")
    {
        return f(
            "You're signed out of GitHub. Click Sign in to fix it.",
            Some(Action::SignIn),
        );
    }
    if r.contains("author identity unknown")
        || r.contains("committer identity unknown")
        || r.contains("please tell me who you are")
    {
        return f(
            "Git doesn't know your name yet. Add your name and email in Settings.",
            Some(Action::SetIdentity),
        );
    }
    if r.contains("exceeds github's file size limit")
        || r.contains("large files detected")
        || r.contains("gh001")
    {
        return f(
            "Some files are over GitHub's 100 MB limit. Remove them or add them to .gitignore.",
            None,
        );
    }
    if r.contains("name already exists on this account")
        || r.contains("already exists") && step == "push"
    {
        return f(
            "A repo with this name already exists. Switch to Update, or pick another name.",
            Some(Action::SwitchToUpdate),
        );
    }
    if r.contains("could not resolve host")
        || r.contains("network is unreachable")
        || r.contains("connection timed out")
        || r.contains("temporary failure in name resolution")
        || r.contains("failed to connect")
    {
        return f(
            "Couldn't reach GitHub. Check your connection; nothing was lost.",
            Some(Action::Retry),
        );
    }
    if r.contains("(timeout)") {
        return f("This step took too long and was stopped. You can retry, or raise the time limit in Settings.", Some(Action::Retry));
    }
    if r.contains("command not found") && r.contains("gh") {
        return f(
            "GitHub's gh tool isn't installed. Open Settings → GitHub account to install it.",
            None,
        );
    }
    if r.contains("command not found") && r.contains("git") {
        return f(
            "Git isn't installed. Open Settings → GitHub account to install it.",
            None,
        );
    }
    f(
        &format!("Something went wrong in {step}."),
        Some(Action::BugReport),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_known_error() {
        let cases = [
            ("! [rejected] main -> main (non-fast-forward)", "GitHub has changes"),
            ("GraphQL: Could not resolve to a Repository with the name 'x/y'", "Wrong account"),
            ("remote: Authentication failed for 'https://github.com/x/y'", "signed out"),
            ("Author identity unknown\n*** Please tell me who you are.", "doesn't know your name"),
            ("remote: error: File big.bin is 150 MB; this exceeds GitHub's file size limit of 100.00 MB", "100 MB"),
            ("GraphQL: Name already exists on this account (createRepository)", "already exists"),
            ("fatal: unable to access: Could not resolve host: github.com", "Couldn't reach GitHub"),
            ("[stowlock] stopped after 120 seconds (timeout)", "too long"),
        ];
        for (raw, want) in cases {
            let e = explain(raw, "push");
            assert!(e.message.contains(want), "{raw} → {}", e.message);
        }
        assert_eq!(
            explain("weird", "stage").message,
            "Something went wrong in stage."
        );
        assert_eq!(explain("weird", "stage").action, Some(Action::BugReport));
    }
}
