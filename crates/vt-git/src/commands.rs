use std::path::Path;
use thiserror::Error;
use tokio::process::Command;
use vt_core::types::{GitFileStatus, WorktreeAddResult, WorktreeRemoveResult, Worktree};

use crate::parser;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a git repository: {0}")]
    NotARepo(String),
}

async fn execute_git_command(args: &[&str], cwd: &Path) -> Result<String, GitError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(GitError::CommandFailed(stderr))
    }
}

pub async fn is_git_repository(path: &Path) -> bool {
    execute_git_command(&["rev-parse", "--git-dir"], path)
        .await
        .is_ok()
}

pub async fn list_worktrees(project_path: &Path) -> Result<Vec<Worktree>, GitError> {
    let output = execute_git_command(&["worktree", "list", "--porcelain"], project_path).await?;
    Ok(parser::parse_worktrees(&output))
}

pub async fn get_git_status(worktree_path: &Path) -> Result<Vec<GitFileStatus>, GitError> {
    let output =
        execute_git_command(&["status", "--porcelain=v1"], worktree_path).await?;
    Ok(parser::parse_git_status(&output))
}

pub async fn get_git_diff(
    worktree_path: &Path,
    file_path: Option<&str>,
) -> Result<String, GitError> {
    let mut args = vec!["diff"];
    if let Some(fp) = file_path {
        args.push("--");
        args.push(fp);
    }
    execute_git_command(&args, worktree_path).await
}

pub async fn get_git_diff_staged(
    worktree_path: &Path,
    file_path: Option<&str>,
) -> Result<String, GitError> {
    let mut args = vec!["diff", "--staged"];
    if let Some(fp) = file_path {
        args.push("--");
        args.push(fp);
    }
    execute_git_command(&args, worktree_path).await
}

pub async fn add_worktree(
    project_path: &Path,
    branch_name: &str,
) -> Result<WorktreeAddResult, GitError> {
    let project_name = project_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    let worktree_dir = format!("{}-{}", project_name, branch_name);
    let worktree_path = project_path
        .parent()
        .unwrap_or(project_path)
        .join(&worktree_dir);

    let wt_str = worktree_path.to_string_lossy();
    execute_git_command(
        &["worktree", "add", "-b", branch_name, &wt_str],
        project_path,
    )
    .await?;

    Ok(WorktreeAddResult {
        path: worktree_path,
        branch: branch_name.to_string(),
    })
}

pub async fn remove_worktree(
    project_path: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<WorktreeRemoveResult, GitError> {
    let wt_str = worktree_path.to_string_lossy();
    execute_git_command(
        &["worktree", "remove", "--force", &wt_str],
        project_path,
    )
    .await?;

    // Try to delete the branch (non-fatal if it fails)
    let warning = match execute_git_command(
        &["branch", "-D", branch_name],
        project_path,
    )
    .await
    {
        Ok(_) => None,
        Err(e) => Some(format!("Branch deletion warning: {}", e)),
    };

    Ok(WorktreeRemoveResult {
        success: true,
        warning,
    })
}

pub async fn get_current_branch(worktree_path: &Path) -> Result<String, GitError> {
    let output =
        execute_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], worktree_path).await?;
    Ok(output.trim().to_string())
}
