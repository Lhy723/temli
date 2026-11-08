use std::path::Path;
use std::process::Command;

fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} 失败: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn rev_parse(repo: &Path, rev: &str) -> Result<String, String> {
    run(repo, &["rev-parse", rev])
}

pub fn head_sha(repo: &Path) -> Result<String, String> {
    rev_parse(repo, "HEAD")
}

pub fn current_branch(repo: &Path) -> Result<String, String> {
    run(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", branch])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Create an isolated worktree on a fresh branch. Fails when the path or branch already exists.
pub fn create_worktree(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    if worktree.exists() {
        return Err(format!("worktree 已存在: {}", worktree.display()));
    }
    if branch_exists(repo, branch) {
        return Err(format!("分支已存在: {branch}"));
    }
    run(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            worktree.to_string_lossy().as_ref(),
            base,
        ],
    )?;
    Ok(())
}

pub fn remove_worktree(repo: &Path, worktree: &Path) -> Result<(), String> {
    run(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            worktree.to_string_lossy().as_ref(),
        ],
    )?;
    Ok(())
}

pub fn is_clean(repo: &Path) -> Result<bool, String> {
    Ok(run(repo, &["status", "--porcelain"])?.is_empty())
}

/// Stage everything and commit when the worktree has changes. Returns the new head when a
/// commit was created, `None` when the tree was already clean.
pub fn commit_all(worktree: &Path, message: &str) -> Result<Option<String>, String> {
    run(worktree, &["add", "-A"])?;
    let staged = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| format!("git 执行失败: {e}"))?;
    if staged.success() {
        return Ok(None);
    }
    run(
        worktree,
        &[
            "-c",
            "user.name=temli",
            "-c",
            "user.email=temli@localhost",
            "commit",
            "-m",
            message,
        ],
    )?;
    head_sha(worktree).map(Some)
}

/// Fast-forward the current branch to `branch`. Refuses dirty trees.
pub fn merge_ff(repo: &Path, branch: &str) -> Result<String, String> {
    if !is_clean(repo)? {
        return Err("目标仓库有未提交改动，拒绝合并".to_string());
    }
    run(repo, &["merge", "--ff-only", branch])?;
    head_sha(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "temli-gitops-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]).unwrap();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&dir, &["add", "-A"]).unwrap();
        run(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        )
        .unwrap();
        dir
    }

    #[test]
    fn creates_worktree_commits_and_fast_forwards() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt");
        let base = head_sha(&repo).unwrap();

        create_worktree(&repo, &wt, "temli/1-run", &base).unwrap();
        assert!(wt.join("a.txt").exists());
        assert_eq!(current_branch(&wt).unwrap(), "temli/1-run");

        fs::write(wt.join("a.txt"), "two\n").unwrap();
        let commit = commit_all(&wt, "change").unwrap();
        assert!(commit.is_some());
        assert_eq!(commit_all(&wt, "noop").unwrap(), None);

        let merged = merge_ff(&repo, "temli/1-run").unwrap();
        assert_eq!(merged, commit.unwrap());
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "two\n");

        remove_worktree(&repo, &wt).unwrap();
        assert!(!wt.exists());
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn refuses_dirty_merge_and_duplicate_worktree() {
        let repo = temp_repo();
        fs::write(repo.join("dirty.txt"), "x\n").unwrap();
        assert!(merge_ff(&repo, "main").is_err());
        fs::remove_file(repo.join("dirty.txt")).unwrap();

        let wt = repo.with_extension("wt2");
        create_worktree(&repo, &wt, "temli/2-run", "HEAD").unwrap();
        assert!(create_worktree(&repo, &wt, "temli/3-run", "HEAD").is_err());
        assert!(
            create_worktree(&repo, &repo.with_extension("wt3"), "temli/2-run", "HEAD").is_err()
        );
        remove_worktree(&repo, &wt).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }
}
