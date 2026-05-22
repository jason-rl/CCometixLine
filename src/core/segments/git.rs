use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId, StyleMode};
use std::collections::HashMap;
use std::process::Command;

/// Returns `true` iff `git rev-parse --git-dir` output indicates a linked worktree.
/// Submodules (`.git/modules/`) and the main worktree (`.git`) return `false`.
pub(crate) fn is_linked_worktree_git_dir(s: &str) -> bool {
    let s = s.trim().trim_end_matches('/').replace('\\', "/");
    if s.contains("/modules/") {
        return false;
    }
    match s.split("/worktrees/").nth(1) {
        Some(after) => !after.split('/').next().unwrap_or("").is_empty(),
        None => false,
    }
}

/// Derive the main repo's directory basename from `git rev-parse --git-common-dir` output.
/// Strips a trailing `/.git`, resolves relative paths against `working_dir`, then takes the basename.
pub(crate) fn derive_repo_basename(common: &str, working_dir: &str) -> Option<String> {
    let s = common.trim().replace('\\', "/");
    let s = s.trim_end_matches('/');
    let full = if s.starts_with('/')
        || (s.len() >= 3 && s.chars().nth(1) == Some(':') && s.chars().nth(2) == Some('/'))
    {
        s.to_string()
    } else {
        format!(
            "{}/{}",
            working_dir.trim_end_matches('/').replace('\\', "/"),
            s
        )
    };
    let without_git = full.strip_suffix("/.git").unwrap_or(&full);
    without_git
        .split('/')
        .next_back()
        .filter(|b| !b.is_empty())
        .map(|b| b.to_string())
}

/// Returns `Some(repo_basename)` iff `working_dir` is inside a linked git worktree.
/// `repo_basename` is the directory name of the main worktree.
pub fn detect_linked_worktree(working_dir: &str) -> Option<String> {
    let git_dir = Command::new("git")
        .args(["--no-optional-locks", "rev-parse", "--git-dir"])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())?;

    if !is_linked_worktree_git_dir(&git_dir) {
        return None;
    }

    let common_dir = Command::new("git")
        .args(["--no-optional-locks", "rev-parse", "--git-common-dir"])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())?;

    derive_repo_basename(&common_dir, working_dir)
}

/// Given raw `git for-each-ref --points-at HEAD --format=%(refname)` output lines,
/// returns the short name of the best non-`worktree-` ref to display, or `None` (detached).
/// Prefers local branches (`refs/heads/`), then remote-tracking (`refs/remotes/`).
/// Within each tier: `main` first, then `master`, then alphabetical.
/// Tags are ignored. Refs whose leaf segment starts with `worktree-` are excluded.
pub(crate) fn select_head_ref(refnames: &[&str]) -> Option<String> {
    let mut local_heads: Vec<String> = Vec::new();
    let mut remotes: Vec<String> = Vec::new();

    for refname in refnames {
        let refname = refname.trim();
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            if !name.starts_with("worktree-") {
                local_heads.push(name.to_string());
            }
        } else if let Some(rest) = refname.strip_prefix("refs/remotes/") {
            let leaf = rest.rsplit('/').next().unwrap_or("");
            if !leaf.starts_with("worktree-") && leaf != "HEAD" {
                remotes.push(rest.to_string());
            }
        }
        // tags ignored
    }

    pick_preferred(local_heads).or_else(|| pick_preferred(remotes))
}

fn pick_preferred(mut names: Vec<String>) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    names.sort();
    names.dedup();
    for pref in &["main", "master"] {
        // Local head: bare name "main". Remote ref "origin/main": branch after first slash.
        // Splitting at the first slash avoids false-positives like "origin/feat/main".
        if let Some(n) = names.iter().find(|n| match n.find('/') {
            Some(i) => &n[i + 1..] == *pref,
            None => n.as_str() == *pref,
        }) {
            return Some(n.clone());
        }
    }
    names.into_iter().next()
}

fn pick_head_ref_name(working_dir: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "for-each-ref",
            "--points-at",
            "HEAD",
            "--format=%(refname)",
        ])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?;
    let refnames: Vec<&str> = text.lines().collect();
    select_head_ref(&refnames)
}

#[derive(Debug)]
pub struct GitInfo {
    pub branch: String,
    pub status: GitStatus,
    pub ahead: u32,
    pub behind: u32,
    pub sha: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum GitStatus {
    Clean,
    Dirty,
    Conflicts,
}

pub struct GitSegment {
    show_sha: bool,
    mode: StyleMode,
    branch_prefixes: HashMap<String, String>,
}

impl Default for GitSegment {
    fn default() -> Self {
        Self::new()
    }
}

impl GitSegment {
    pub fn new() -> Self {
        Self {
            show_sha: false,
            mode: StyleMode::Plain,
            branch_prefixes: HashMap::new(),
        }
    }

    pub fn with_sha(mut self, show_sha: bool) -> Self {
        self.show_sha = show_sha;
        self
    }

    pub fn with_mode(mut self, mode: StyleMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_branch_prefixes(mut self, prefixes: HashMap<String, String>) -> Self {
        self.branch_prefixes = prefixes;
        self
    }

    /// Replace the longest matching branch prefix with its configured value.
    /// Matching is longest-first so `"feat/long/"` beats `"feat/"` when both apply.
    fn apply_branch_prefix(&self, branch: &str) -> String {
        if self.branch_prefixes.is_empty() {
            return branch.to_string();
        }
        // Sort candidates by prefix length (longest first) for deterministic match
        let mut candidates: Vec<(&str, &str)> = self
            .branch_prefixes
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        candidates.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        for (prefix, replacement) in candidates {
            if let Some(rest) = branch.strip_prefix(prefix) {
                return format!("{}{}", replacement, rest);
            }
        }
        branch.to_string()
    }

    fn get_git_info(&self, working_dir: &str) -> Option<GitInfo> {
        if !self.is_git_repository(working_dir) {
            return None;
        }

        let in_linked_worktree = detect_linked_worktree(working_dir).is_some();

        let branch = self
            .get_branch(working_dir, in_linked_worktree)
            .unwrap_or_else(|| {
                if self.mode != StyleMode::Plain {
                    "\u{f127}".to_string() // nf-fa-chain_broken: detached HEAD
                } else {
                    "detached".to_string()
                }
            });
        let status = self.get_status(working_dir);
        let (ahead, behind) = self.get_ahead_behind(working_dir);
        let sha = if self.show_sha {
            self.get_sha(working_dir)
        } else {
            None
        };

        Some(GitInfo {
            branch,
            status,
            ahead,
            behind,
            sha,
        })
    }

    fn is_git_repository(&self, working_dir: &str) -> bool {
        Command::new("git")
            .args(["--no-optional-locks", "rev-parse", "--git-dir"])
            .current_dir(working_dir)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn get_branch(&self, working_dir: &str, in_linked_worktree: bool) -> Option<String> {
        let mut candidate: Option<String> = None;

        if let Ok(output) = Command::new("git")
            .args(["--no-optional-locks", "branch", "--show-current"])
            .current_dir(working_dir)
            .output()
        {
            if output.status.success() {
                if let Ok(b) = String::from_utf8(output.stdout) {
                    let b = b.trim().to_string();
                    if !b.is_empty() {
                        candidate = Some(b);
                    }
                }
            }
        }

        if candidate.is_none() {
            if let Ok(output) = Command::new("git")
                .args(["--no-optional-locks", "symbolic-ref", "--short", "HEAD"])
                .current_dir(working_dir)
                .output()
            {
                if output.status.success() {
                    if let Ok(b) = String::from_utf8(output.stdout) {
                        let b = b.trim().to_string();
                        if !b.is_empty() {
                            candidate = Some(b);
                        }
                    }
                }
            }
        }

        // When both branch commands return empty (jj colocated repos, genuine detached HEAD),
        // try to find a local branch or remote-tracking ref pointing at HEAD.
        let branch = match candidate {
            Some(b) => b,
            None => return pick_head_ref_name(working_dir),
        };

        if in_linked_worktree && branch.starts_with("worktree-") {
            return pick_head_ref_name(working_dir);
        }

        Some(branch)
    }

    fn get_status(&self, working_dir: &str) -> GitStatus {
        let output = Command::new("git")
            .args(["--no-optional-locks", "status", "--porcelain"])
            .current_dir(working_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let status_text = String::from_utf8(output.stdout).unwrap_or_default();

                if status_text.trim().is_empty() {
                    return GitStatus::Clean;
                }

                if status_text.contains("UU")
                    || status_text.contains("AA")
                    || status_text.contains("DD")
                {
                    GitStatus::Conflicts
                } else {
                    GitStatus::Dirty
                }
            }
            _ => GitStatus::Clean,
        }
    }

    fn get_ahead_behind(&self, working_dir: &str) -> (u32, u32) {
        let ahead = self.get_commit_count(working_dir, "@{u}..HEAD");
        let behind = self.get_commit_count(working_dir, "HEAD..@{u}");
        (ahead, behind)
    }

    fn get_commit_count(&self, working_dir: &str, range: &str) -> u32 {
        let output = Command::new("git")
            .args(["--no-optional-locks", "rev-list", "--count", range])
            .current_dir(working_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => String::from_utf8(output.stdout)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Returns the GitHub HTTPS branch URL if the tracking remote is a GitHub remote
    /// and the remote-tracking ref exists locally. Returns `None` (silently) otherwise.
    fn get_github_branch_url(&self, working_dir: &str, branch: &str) -> Option<String> {
        let remote = self.get_tracking_remote(working_dir, branch)?;
        let remote_url = self.get_remote_url(working_dir, &remote)?;
        let (owner, repo) = Self::parse_github_owner_repo(&remote_url)?;
        let remote_branch = self.get_remote_branch_name(working_dir, branch);
        if !self.remote_tracking_ref_exists(working_dir, &remote, &remote_branch) {
            return None;
        }
        Some(format!(
            "https://github.com/{}/{}/tree/{}",
            owner,
            repo,
            Self::percent_encode_path(&remote_branch),
        ))
    }

    /// Percent-encode a URL path segment per RFC 3986, preserving unreserved chars
    /// (`A-Z a-z 0-9 - _ . ~`) and `/` (so `feat/my-branch` stays readable).
    fn percent_encode_path(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 8);
        for &b in s.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    out.push(b as char)
                }
                _ => {
                    out.push('%');
                    out.push(
                        char::from_digit((b >> 4) as u32, 16)
                            .unwrap()
                            .to_ascii_uppercase(),
                    );
                    out.push(
                        char::from_digit((b & 0xf) as u32, 16)
                            .unwrap()
                            .to_ascii_uppercase(),
                    );
                }
            }
        }
        out
    }

    fn get_tracking_remote(&self, working_dir: &str, branch: &str) -> Option<String> {
        let key = format!("branch.{}.remote", branch);
        let output = Command::new("git")
            .args(["--no-optional-locks", "config", "--get", &key])
            .current_dir(working_dir)
            .output()
            .ok()?;
        if output.status.success() {
            let remote = String::from_utf8(output.stdout).ok()?.trim().to_string();
            if !remote.is_empty() {
                return Some(remote);
            }
        }
        None
    }

    fn get_remote_url(&self, working_dir: &str, remote: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["--no-optional-locks", "remote", "get-url", remote])
            .current_dir(working_dir)
            .output()
            .ok()?;
        if output.status.success() {
            let url = String::from_utf8(output.stdout).ok()?.trim().to_string();
            if !url.is_empty() {
                return Some(url);
            }
        }
        None
    }

    /// Parse GitHub remote URLs into `(owner, repo)`. Returns `None` for non-GitHub remotes.
    ///
    /// Supported formats:
    /// - SSH scp-style: `git@github.com:owner/repo[.git]`
    /// - SSH URL:       `ssh://git@github.com/owner/repo[.git]`
    /// - HTTPS:         `https://github.com/owner/repo[.git]`
    fn parse_github_owner_repo(url: &str) -> Option<(String, String)> {
        let url = url.trim();
        // Match on exact host prefixes to avoid false-positives like `notgithub.com`
        let path = url
            .strip_prefix("git@github.com:")
            .or_else(|| url.strip_prefix("https://github.com/"))
            .or_else(|| url.strip_prefix("http://github.com/"))
            .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
        let path = path.trim_end_matches('/').trim_end_matches(".git");
        let slash = path.find('/')?;
        let owner = &path[..slash];
        // Take only the first two components; ignore sub-paths like owner/repo/extra
        let repo = path[slash + 1..].split('/').next()?;
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some((owner.to_string(), repo.to_string()))
    }

    /// Returns the remote branch name from `branch.<name>.merge`, defaulting to the
    /// local branch name if the config key is absent.
    fn get_remote_branch_name(&self, working_dir: &str, branch: &str) -> String {
        let key = format!("branch.{}.merge", branch);
        if let Ok(output) = Command::new("git")
            .args(["--no-optional-locks", "config", "--get", &key])
            .current_dir(working_dir)
            .output()
        {
            if output.status.success() {
                if let Ok(merge_ref) = String::from_utf8(output.stdout) {
                    let merge_ref = merge_ref.trim();
                    if let Some(name) = merge_ref.strip_prefix("refs/heads/") {
                        return name.to_string();
                    }
                    if !merge_ref.is_empty() {
                        return merge_ref.to_string();
                    }
                }
            }
        }
        branch.to_string()
    }

    fn remote_tracking_ref_exists(&self, working_dir: &str, remote: &str, branch: &str) -> bool {
        let ref_path = format!("refs/remotes/{}/{}", remote, branch);
        Command::new("git")
            .args([
                "--no-optional-locks",
                "show-ref",
                "--verify",
                "--quiet",
                &ref_path,
            ])
            .current_dir(working_dir)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn get_sha(&self, working_dir: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["--no-optional-locks", "rev-parse", "--short=7", "HEAD"])
            .current_dir(working_dir)
            .output()
            .ok()?;

        if output.status.success() {
            let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
            if sha.is_empty() {
                None
            } else {
                Some(sha)
            }
        } else {
            None
        }
    }
}

impl Segment for GitSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let git_info = self.get_git_info(&input.workspace.current_dir)?;

        let mut metadata = HashMap::new();
        metadata.insert("branch".to_string(), git_info.branch.clone());
        metadata.insert("status".to_string(), format!("{:?}", git_info.status));
        metadata.insert("ahead".to_string(), git_info.ahead.to_string());
        metadata.insert("behind".to_string(), git_info.behind.to_string());

        if let Some(ref sha) = git_info.sha {
            metadata.insert("sha".to_string(), sha.clone());
        }

        let primary = git_info.branch;
        let mut status_parts = Vec::new();

        match git_info.status {
            GitStatus::Clean => status_parts.push("✓".to_string()),
            GitStatus::Dirty => status_parts.push("●".to_string()),
            GitStatus::Conflicts => status_parts.push("⚠".to_string()),
        }

        if git_info.ahead > 0 {
            status_parts.push(format!("↑{}", git_info.ahead));
        }
        if git_info.behind > 0 {
            status_parts.push(format!("↓{}", git_info.behind));
        }

        if let Some(ref sha) = git_info.sha {
            status_parts.push(sha.clone());
        }

        // Add GitHub branch URL for OSC 8 hyperlink if the tracking remote is GitHub
        // and the remote-tracking ref exists locally. Silently skipped otherwise.
        if let Some(url) = self.get_github_branch_url(&input.workspace.current_dir, &primary) {
            metadata.insert("hyperlink_url".to_string(), url);
        }

        // Apply prefix substitution for display AFTER the hyperlink URL is set,
        // so the link always points to the real remote branch regardless of display labels.
        let primary = self.apply_branch_prefix(&primary);

        Some(SegmentData {
            primary,
            secondary: status_parts.join(" "),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Git
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linked_worktree_absolute() {
        assert!(is_linked_worktree_git_dir(
            "/home/u/repo/.git/worktrees/feat-x"
        ));
    }

    #[test]
    fn test_linked_worktree_trailing_slash() {
        assert!(is_linked_worktree_git_dir(
            "/home/u/repo/.git/worktrees/feat-x/"
        ));
    }

    #[test]
    fn test_linked_worktree_relative() {
        assert!(is_linked_worktree_git_dir(".git/worktrees/feat-x"));
    }

    #[test]
    fn test_linked_worktree_windows() {
        assert!(is_linked_worktree_git_dir(r"C:\repo\.git\worktrees\feat-x"));
    }

    #[test]
    fn test_main_worktree_dot_git() {
        assert!(!is_linked_worktree_git_dir(".git"));
    }

    #[test]
    fn test_main_worktree_absolute() {
        assert!(!is_linked_worktree_git_dir("/home/u/repo/.git"));
    }

    #[test]
    fn test_submodule_rejected() {
        assert!(!is_linked_worktree_git_dir("/home/u/repo/.git/modules/sub"));
    }

    #[test]
    fn test_empty_worktree_name_rejected() {
        assert!(!is_linked_worktree_git_dir("/home/u/repo/.git/worktrees/"));
    }

    #[test]
    fn test_derive_repo_basename_absolute() {
        assert_eq!(
            derive_repo_basename("/home/u/MyRepo/.git\n", "/any/cwd"),
            Some("MyRepo".to_string())
        );
    }

    #[test]
    fn test_derive_repo_basename_relative() {
        assert_eq!(
            derive_repo_basename(".git\n", "/home/u/MyRepo"),
            Some("MyRepo".to_string())
        );
    }

    #[test]
    fn test_derive_repo_basename_trailing_slash() {
        assert_eq!(
            derive_repo_basename("/home/u/MyRepo/.git/\n", "/any/cwd"),
            Some("MyRepo".to_string())
        );
    }

    #[test]
    fn test_derive_repo_basename_empty() {
        assert_eq!(derive_repo_basename("", "/any/cwd"), None);
    }

    #[test]
    fn test_select_head_ref_prefers_local_branch() {
        assert_eq!(
            select_head_ref(&[
                "refs/heads/main",
                "refs/heads/worktree-foo",
                "refs/remotes/origin/main",
            ]),
            Some("main".to_string())
        );
    }

    #[test]
    fn test_select_head_ref_falls_back_to_remote() {
        assert_eq!(
            select_head_ref(&["refs/heads/worktree-foo", "refs/remotes/origin/main"]),
            Some("origin/main".to_string())
        );
    }

    #[test]
    fn test_select_head_ref_detached_when_only_worktree_refs() {
        assert_eq!(
            select_head_ref(&[
                "refs/heads/worktree-foo",
                "refs/remotes/origin/worktree-foo",
            ]),
            None
        );
    }

    #[test]
    fn test_select_head_ref_master_priority() {
        assert_eq!(
            select_head_ref(&["refs/heads/feature", "refs/heads/master"]),
            Some("master".to_string())
        );
    }

    #[test]
    fn test_select_head_ref_alphabetical_tiebreak() {
        assert_eq!(
            select_head_ref(&["refs/heads/aaa", "refs/heads/zzz"]),
            Some("aaa".to_string())
        );
    }

    #[test]
    fn test_select_head_ref_ignores_tags() {
        assert_eq!(
            select_head_ref(&["refs/heads/worktree-foo", "refs/tags/v1.0"]),
            None
        );
    }

    #[test]
    fn test_select_head_ref_no_false_positive_path_suffixed_trunk() {
        // "origin/feat/main" must NOT be treated as the trunk "main" branch;
        // only "origin/main" (branch-after-first-slash == "main") should match.
        assert_eq!(
            select_head_ref(&["refs/heads/worktree-foo", "refs/remotes/origin/feat/main",]),
            Some("origin/feat/main".to_string()) // remote fallback, but not trunk-priority
        );
        // When the real origin/main is also present it wins over origin/feat/main.
        assert_eq!(
            select_head_ref(&["refs/remotes/origin/main", "refs/remotes/origin/feat/main",]),
            Some("origin/main".to_string())
        );
    }

    #[test]
    fn test_select_head_ref_ignores_remote_head_pseudo_ref() {
        // refs/remotes/origin/HEAD is a symbolic pointer, not a real branch — must be excluded.
        assert_eq!(
            select_head_ref(&["refs/heads/worktree-foo", "refs/remotes/origin/HEAD",]),
            None
        );
    }

    #[test]
    fn test_select_head_ref_jj_colocated_repo() {
        // jj colocated repos emit refs/jj/keep/… at HEAD alongside normal local branches.
        // The jj refs must be ignored; the local branch must be returned.
        assert_eq!(
            select_head_ref(&[
                "refs/heads/jason/db-scaling-restore",
                "refs/jj/keep/f42d5964de3337502aaf82fe74d0cad292f8d868",
                "refs/remotes/origin/jason/db-scaling-restore",
            ]),
            Some("jason/db-scaling-restore".to_string())
        );
    }

    fn parse(url: &str) -> Option<(String, String)> {
        GitSegment::parse_github_owner_repo(url)
    }

    #[test]
    fn test_parse_github_ssh() {
        assert_eq!(
            parse("git@github.com:owner/repo.git"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_parse_github_ssh_no_git_suffix() {
        assert_eq!(
            parse("git@github.com:owner/repo"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_parse_github_https() {
        assert_eq!(
            parse("https://github.com/owner/repo.git"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_parse_github_https_no_git_suffix() {
        assert_eq!(
            parse("https://github.com/owner/repo"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_parse_github_ssh_url_form() {
        assert_eq!(
            parse("ssh://git@github.com/owner/repo.git"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_parse_non_github_remote() {
        assert_eq!(parse("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(parse("https://bitbucket.org/owner/repo"), None);
        // Must not match on substring — these contain "github.com" but are different hosts
        assert_eq!(parse("https://notgithub.com/owner/repo"), None);
        assert_eq!(parse("https://gist.github.com/user/abc123"), None);
    }

    #[test]
    fn test_parse_github_with_trailing_slash() {
        assert_eq!(
            parse("https://github.com/owner/repo/"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn test_branch_prefix_substitution() {
        let seg = GitSegment::new().with_branch_prefixes(
            [
                ("feat/".to_string(), "✨ ".to_string()),
                ("fix/".to_string(), "🐛 ".to_string()),
                ("chore/".to_string(), "🔧 ".to_string()),
            ]
            .into(),
        );
        assert_eq!(seg.apply_branch_prefix("feat/my-feature"), "✨ my-feature");
        assert_eq!(seg.apply_branch_prefix("fix/issue-42"), "🐛 issue-42");
        assert_eq!(seg.apply_branch_prefix("main"), "main"); // no match → unchanged
    }

    #[test]
    fn test_branch_prefix_longest_wins() {
        let seg = GitSegment::new().with_branch_prefixes(
            [
                ("feat/".to_string(), "short".to_string()),
                ("feat/team/".to_string(), "long".to_string()),
            ]
            .into(),
        );
        // Longer prefix should win
        assert_eq!(seg.apply_branch_prefix("feat/team/foo"), "longfoo");
        // Shorter prefix matches when longer doesn't
        assert_eq!(seg.apply_branch_prefix("feat/solo"), "shortsolo");
    }

    #[test]
    fn test_branch_prefix_empty_map() {
        let seg = GitSegment::new(); // no prefixes
        assert_eq!(seg.apply_branch_prefix("feat/foo"), "feat/foo");
    }

    #[test]
    fn test_percent_encode_path() {
        let enc = |s: &str| GitSegment::percent_encode_path(s);
        assert_eq!(enc("main"), "main");
        assert_eq!(enc("feat/my-branch"), "feat/my-branch");
        assert_eq!(enc("feat/my branch"), "feat/my%20branch");
        assert_eq!(enc("issue#42"), "issue%2342");
        assert_eq!(enc("a?b=c"), "a%3Fb%3Dc");
    }
}
