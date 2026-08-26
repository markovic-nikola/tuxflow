//! Git state for the status bar, local or over ssh — one `status
//! --porcelain=v2 --branch` round trip yields branch, ahead/behind and
//! the changed-entry count. Blocking — call from a worker thread.

use crate::remote::{ProjectLocation, sh_quote, ssh_mux_options};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Working-tree entries (changed / untracked / conflicted).
    pub changed: u32,
}

/// A git invocation in the project's real location: plain `git` locally,
/// `ssh host 'cd dir && git …'` (mux, BatchMode) for remote projects.
pub fn git_command(location: &ProjectLocation, args: &[&str]) -> std::process::Command {
    match location {
        ProjectLocation::Local(dir) => {
            let mut cmd = std::process::Command::new("git");
            cmd.args(args).current_dir(dir);
            cmd
        }
        ProjectLocation::Ssh { host, dir } => {
            let mut cmd = std::process::Command::new("ssh");
            cmd.args(ssh_mux_options());
            cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
            cmd.arg(host);
            let git_args = args
                .iter()
                .map(|a| sh_quote(a))
                .collect::<Vec<_>>()
                .join(" ");
            cmd.arg(format!("cd {} && git {}", sh_quote(dir), git_args));
            cmd
        }
    }
}

/// None = not a git repo, git absent, or (remote) host unreachable —
/// the chip simply doesn't show.
pub fn query_status(location: &ProjectLocation) -> Option<GitStatus> {
    let output = git_command(location, &["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_porcelain_v2(text: &str) -> GitStatus {
    let mut status = GitStatus {
        branch: String::new(),
        ahead: 0,
        behind: 0,
        changed: 0,
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            status.branch = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            status.changed += 1;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_ahead_behind_and_changes() {
        let text = "\
# branch.oid 0123abc\n\
# branch.head iced-migration\n\
# branch.upstream origin/iced-migration\n\
# branch.ab +2 -1\n\
1 .M N... 100644 100644 100644 0a 0b src/main.rs\n\
? target/new-file\n";
        let status = parse_porcelain_v2(text);
        assert_eq!(status.branch, "iced-migration");
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
        assert_eq!(status.changed, 2);
    }

    /// Detached HEAD reports "(detached)"; clean tree counts zero.
    #[test]
    fn parses_detached_clean() {
        let text = "# branch.oid 0123abc\n# branch.head (detached)\n";
        let status = parse_porcelain_v2(text);
        assert_eq!(status.branch, "(detached)");
        assert_eq!(status.changed, 0);
        assert_eq!(status.ahead, 0);
    }
}
