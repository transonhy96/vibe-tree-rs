use vt_core::types::{GitFileStatus, Worktree};
use std::path::PathBuf;

/// Parse `git worktree list --porcelain` output into structured worktrees.
pub fn parse_worktrees(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut head: Option<String> = None;
    let mut branch: Option<String> = None;

    for line in output.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            head = Some(h.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(extract_branch_name(b).to_string());
        } else if line == "detached" {
            branch = None;
        } else if line.is_empty() {
            if let (Some(p), Some(h)) = (path.take(), head.take()) {
                worktrees.push(Worktree {
                    path: p,
                    branch: branch.take(),
                    head: h,
                });
            }
            branch = None;
        }
    }

    // Handle last entry (no trailing blank line)
    if let (Some(p), Some(h)) = (path, head) {
        worktrees.push(Worktree {
            path: p,
            branch,
            head: h,
        });
    }

    worktrees
}

/// Parse `git status --porcelain=v1` output into structured file statuses.
pub fn parse_git_status(output: &str) -> Vec<GitFileStatus> {
    output
        .lines()
        .filter(|line| line.len() >= 3)
        .map(|line| {
            let status_code = &line[..2];
            let file_path = &line[3..];
            let x = status_code.as_bytes()[0];
            let y = status_code.as_bytes()[1];

            GitFileStatus {
                path: file_path.to_string(),
                status_code: status_code.to_string(),
                staged: x != b' ' && x != b'?',
                modified: y != b' ' && y != b'?',
            }
        })
        .collect()
}

/// Extract branch name from a full ref like `refs/heads/main`.
pub fn extract_branch_name(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
}

/// Check if a branch name is a main/master branch.
pub fn is_main_branch(name: &str) -> bool {
    matches!(name, "main" | "master")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktrees() {
        let output = "\
worktree /home/user/project
HEAD abc123def456
branch refs/heads/main

worktree /home/user/project-feature
HEAD def789abc012
branch refs/heads/feature-x

";
        let wt = parse_worktrees(output);
        assert_eq!(wt.len(), 2);
        assert_eq!(wt[0].branch.as_deref(), Some("main"));
        assert_eq!(wt[1].branch.as_deref(), Some("feature-x"));
        assert_eq!(wt[0].path, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn test_parse_worktrees_detached() {
        let output = "\
worktree /home/user/project
HEAD abc123
detached

";
        let wt = parse_worktrees(output);
        assert_eq!(wt.len(), 1);
        assert!(wt[0].branch.is_none());
    }

    #[test]
    fn test_parse_git_status() {
        let output = " M src/main.rs\nA  src/new.rs\n?? untracked.txt\n";
        let status = parse_git_status(output);
        assert_eq!(status.len(), 3);
        assert!(!status[0].staged);
        assert!(status[0].modified);
        assert!(status[1].staged);
        assert!(!status[1].modified);
    }

    #[test]
    fn test_extract_branch_name() {
        assert_eq!(extract_branch_name("refs/heads/main"), "main");
        assert_eq!(extract_branch_name("main"), "main");
    }

    #[test]
    fn test_is_main_branch() {
        assert!(is_main_branch("main"));
        assert!(is_main_branch("master"));
        assert!(!is_main_branch("feature-x"));
    }
}
