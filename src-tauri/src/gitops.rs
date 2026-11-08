use std::path::{Path, PathBuf};
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

/// Whether `ancestor` is reachable from `descendant`. git 用退出码 1 表示
/// “不是祖先”（正常结果），其余非零退出码（未知 rev、损坏仓库）才是错误。
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| format!("git 执行失败: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git merge-base --is-ancestor {ancestor} {descendant} 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
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

/// Registered worktrees of `repo`, the main worktree included.
pub fn list_worktrees(repo: &Path) -> Result<Vec<PathBuf>, String> {
    let out = run(repo, &["worktree", "list", "--porcelain"])?;
    Ok(out
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}

/// Drop bookkeeping for worktrees whose directories no longer exist.
pub fn prune_worktrees(repo: &Path) -> Result<(), String> {
    run(repo, &["worktree", "prune"])?;
    Ok(())
}

/// Whether a rebase is mid-flight in `repo` (or in the worktree at `repo`).
pub fn rebase_in_progress(repo: &Path) -> bool {
    ["rebase-merge", "rebase-apply"].into_iter().any(|name| {
        run(repo, &["rev-parse", "--git-path", name])
            .ok()
            .map(PathBuf::from)
            .map(|p| if p.is_absolute() { p } else { repo.join(p) })
            .is_some_and(|p| p.exists())
    })
}

/// A failed rebase: `message` is the git error including stderr, `files` lists the
/// unmerged paths (`git diff --name-only --diff-filter=U`). The rebase has already been
/// aborted, so the worktree is left conflict-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseConflict {
    pub message: String,
    pub files: Vec<String>,
}

/// Rebase the worktree's current branch onto `target_ref`. On failure the conflicting
/// paths are collected before aborting, so callers can decide between an automatic retry
/// and a human decision.
pub fn rebase_onto_with_conflicts(worktree: &Path, target_ref: &str) -> Result<(), RebaseConflict> {
    match run(worktree, &["rebase", target_ref]) {
        Ok(_) => Ok(()),
        Err(message) => {
            let files: Vec<String> = run(worktree, &["diff", "--name-only", "--diff-filter=U"])
                .map(|out| {
                    out.lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let _ = run(worktree, &["rebase", "--abort"]);
            Err(RebaseConflict { message, files })
        }
    }
}

/// Compatibility wrapper around [`rebase_onto_with_conflicts`]; returns only the error text.
#[cfg_attr(not(test), allow(dead_code))]
pub fn rebase_onto(worktree: &Path, target_ref: &str) -> Result<(), String> {
    rebase_onto_with_conflicts(worktree, target_ref).map_err(|conflict| conflict.message)
}

/// Hard-reset `repo` to `sha`, discarding tracked changes.
pub fn reset_hard(repo: &Path, sha: &str) -> Result<(), String> {
    run(repo, &["reset", "--hard", sha])?;
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
    fn detects_ancestry_between_commits() {
        let repo = temp_repo();
        let base = head_sha(&repo).unwrap();
        fs::write(repo.join("b.txt"), "b\n").unwrap();
        let next = commit_all(&repo, "second").unwrap().unwrap();

        assert!(is_ancestor(&repo, &base, &next).unwrap());
        assert!(is_ancestor(&repo, &next, &next).unwrap());
        assert!(!is_ancestor(&repo, &next, &base).unwrap());
        assert!(
            is_ancestor(&repo, "no-such-rev", &next).is_err(),
            "未知 rev 必须报错而不是当成非祖先"
        );
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

    #[test]
    fn rebases_diverged_worktree_onto_moved_main() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt-rebase");
        let base = head_sha(&repo).unwrap();

        create_worktree(&repo, &wt, "temli/1-run", &base).unwrap();
        fs::write(wt.join("feat.txt"), "feat\n").unwrap();
        let feat = commit_all(&wt, "feat").unwrap().unwrap();

        fs::write(repo.join("main.txt"), "main\n").unwrap();
        let main = commit_all(&repo, "main").unwrap().unwrap();
        assert_ne!(main, feat);

        rebase_onto(&wt, "main").unwrap();
        assert!(!rebase_in_progress(&wt));
        assert_ne!(head_sha(&wt).unwrap(), feat);
        assert_eq!(rev_parse(&wt, "HEAD~1").unwrap(), main);
        assert_eq!(fs::read_to_string(wt.join("feat.txt")).unwrap(), "feat\n");
        assert_eq!(fs::read_to_string(wt.join("main.txt")).unwrap(), "main\n");

        remove_worktree(&repo, &wt).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn conflicting_rebase_returns_err_and_aborts() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt-conflict");
        create_worktree(&repo, &wt, "temli/2-run", "HEAD").unwrap();

        fs::write(wt.join("a.txt"), "feature\n").unwrap();
        commit_all(&wt, "feature").unwrap();
        fs::write(repo.join("a.txt"), "main\n").unwrap();
        commit_all(&repo, "main").unwrap();

        let err = rebase_onto(&wt, "main").unwrap_err();
        assert!(err.contains("git rebase main 失败"), "{err}");
        assert!(!rebase_in_progress(&wt));
        assert_eq!(current_branch(&wt).unwrap(), "temli/2-run");
        assert!(is_clean(&wt).unwrap());
        assert_eq!(fs::read_to_string(wt.join("a.txt")).unwrap(), "feature\n");

        remove_worktree(&repo, &wt).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn rebase_conflict_reports_unmerged_files_before_aborting() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt-conflict-files");
        create_worktree(&repo, &wt, "temli/5-run", "HEAD").unwrap();

        fs::write(wt.join("a.txt"), "feature\n").unwrap();
        commit_all(&wt, "feature").unwrap();
        fs::write(repo.join("a.txt"), "main\n").unwrap();
        commit_all(&repo, "main").unwrap();

        let conflict = rebase_onto_with_conflicts(&wt, "main").unwrap_err();
        assert!(
            conflict.message.contains("git rebase main 失败"),
            "{conflict:?}"
        );
        assert_eq!(conflict.files, vec!["a.txt".to_string()]);
        assert!(!rebase_in_progress(&wt));
        assert!(is_clean(&wt).unwrap());
        assert_eq!(fs::read_to_string(wt.join("a.txt")).unwrap(), "feature\n");

        remove_worktree(&repo, &wt).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn reset_hard_restores_file_and_moves_head() {
        let repo = temp_repo();
        let base = head_sha(&repo).unwrap();

        fs::write(repo.join("a.txt"), "changed\n").unwrap();
        fs::write(repo.join("extra.txt"), "extra\n").unwrap();
        commit_all(&repo, "second").unwrap();
        fs::write(repo.join("a.txt"), "again\n").unwrap();

        reset_hard(&repo, &base).unwrap();
        assert_eq!(head_sha(&repo).unwrap(), base);
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");
        assert!(!repo.join("extra.txt").exists());
        assert!(is_clean(&repo).unwrap());
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn lists_main_and_added_worktrees() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt-list");
        create_worktree(&repo, &wt, "temli/3-run", "HEAD").unwrap();

        let listed = list_worktrees(&repo).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&fs::canonicalize(&repo).unwrap()));
        assert!(listed.contains(&fs::canonicalize(&wt).unwrap()));

        remove_worktree(&repo, &wt).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn prune_forgets_deleted_worktree() {
        let repo = temp_repo();
        let wt = repo.with_extension("wt-prune");
        create_worktree(&repo, &wt, "temli/4-run", "HEAD").unwrap();
        fs::remove_dir_all(&wt).unwrap();

        prune_worktrees(&repo).unwrap();
        let listed = list_worktrees(&repo).unwrap();
        assert_eq!(listed, vec![fs::canonicalize(&repo).unwrap()]);
        let _ = fs::remove_dir_all(&repo);
    }
}
