//! GitHub issue 同步引擎。
//!
//! 后台线程每 60 秒拉取一次已配置项目的 issue 列表（使用 ETag 条件请求）。
//! 同步只覆盖 GitHub 侧字段（`title` / `body` / `github_state` /
//! `github_assignee` / `github_updated_at` / `ghost`），绝不改写本地任务状态：
//! 任务状态由 [`crate::store::transition`] 单点管理。
//!
//! 远端编辑使用乐观并发：本地保存的 `github_updated_at` 与远端不一致时拒绝写入，
//! 要求用户先同步。

use crate::db::Db;
use crate::github::{GitHubOps, IssuePatch};
use rusqlite::{OptionalExtension, params};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

/// 轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const ISSUES_RESOURCE: &str = "issues";
/// 本地编辑与远端冲突时的固定错误。
pub const REMOTE_CHANGED: &str = "远端已变更，请先同步";

pub struct SyncEngine {
    db: Arc<Mutex<Db>>,
    ops: Arc<dyn GitHubOps>,
    /// 串行化发布：读取 `github_number`、创建远端 issue、回写关联必须在同一临界区，
    /// 否则并发点击“发布”会各自读到 `NULL` 并创建两个 issue。
    publish_lock: Mutex<()>,
}

impl SyncEngine {
    pub fn new(db: Arc<Mutex<Db>>, ops: Arc<dyn GitHubOps>) -> Arc<Self> {
        Arc::new(Self {
            db,
            ops,
            publish_lock: Mutex::new(()),
        })
    }

    /// 启动唯一的同步线程；只能调用一次。
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let engine = Arc::clone(self);
        std::thread::Builder::new()
            .name("temli-sync".to_string())
            .spawn(move || engine.work_loop())
            .map(|_| ())
            .map_err(|err| format!("无法启动同步线程：{err}"))
    }

    fn work_loop(&self) {
        loop {
            if let Err(err) = self.sync_all() {
                eprintln!("GitHub 同步失败：{err}");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// 同步所有已配置 `github_owner` + `github_repo` 的项目。
    pub fn sync_all(&self) -> Result<(), String> {
        let projects = {
            let db = self.lock_db()?;
            db.list_projects()?
        };
        let mut first_error = None;
        for project in projects {
            match repo_slug(&project) {
                Ok(Some(_)) => {}
                Ok(None) => continue,
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    continue;
                }
            }
            if let Err(err) = self.sync_project(&project.id) {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// 同步单个项目。
    pub fn sync_project(&self, project_id: &str) -> Result<(), String> {
        let (repo, etag) = {
            let db = self.lock_db()?;
            let project = db
                .get_project(project_id)?
                .ok_or_else(|| format!("项目不存在：{project_id}"))?;
            let repo = repo_slug(&project)?.ok_or_else(|| "项目未配置 GitHub 仓库".to_string())?;
            let etag: Option<String> = db
                .conn()
                .query_row(
                    "SELECT etag FROM sync_state WHERE project_id = ?1 AND resource = ?2",
                    params![project_id, ISSUES_RESOURCE],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?
                .flatten();
            (repo, etag)
        };

        let page = self.ops.list_issues(&repo, etag.as_deref())?;
        if page.not_modified {
            return Ok(());
        }

        let db = self.lock_db()?;
        let tx = db.conn().unchecked_transaction().map_err(db_err)?;

        let mut seen = HashSet::with_capacity(page.issues.len());
        for issue in &page.issues {
            seen.insert(issue.number as i64);
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM items
                     WHERE project_id = ?1 AND github_number = ?2
                     LIMIT 1",
                    params![project_id, issue.number as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            match existing {
                Some(item_id) => {
                    // 只覆盖 GitHub 侧字段；`items.status` 属于本地状态机。
                    tx.execute(
                        "UPDATE items
                         SET title = ?2,
                             body = ?3,
                             github_state = ?4,
                             github_assignee = ?5,
                             github_updated_at = ?6,
                             ghost = 0,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                         WHERE id = ?1",
                        params![
                            item_id,
                            issue.title,
                            issue.body,
                            issue.state,
                            issue.assignee,
                            issue.updated_at
                        ],
                    )
                    .map_err(db_err)?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO items (id, project_id, kind, title, status, body,
                                            github_number, github_state, github_assignee,
                                            github_updated_at, ghost)
                         VALUES (?1, ?2, 'issue', ?3, 'open', ?4, ?5, ?6, ?7, ?8, 0)",
                        params![
                            Uuid::new_v4().to_string(),
                            project_id,
                            issue.title,
                            issue.body,
                            issue.number as i64,
                            issue.state,
                            issue.assignee,
                            issue.updated_at
                        ],
                    )
                    .map_err(db_err)?;
                }
            }
        }

        // 页面上已消失的关联 issue 标记为 ghost；保留本地行与任务/运行历史。
        let linked: Vec<(String, i64)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, github_number FROM items
                     WHERE project_id = ?1 AND github_number IS NOT NULL AND ghost = 0",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(db_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
        };
        for (item_id, number) in linked {
            if seen.contains(&number) {
                continue;
            }
            tx.execute(
                "UPDATE items
                 SET ghost = 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?1",
                params![item_id],
            )
            .map_err(db_err)?;
        }

        if let Some(etag) = page.etag.as_deref() {
            tx.execute(
                "INSERT INTO sync_state (project_id, resource, etag, last_synced_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                 ON CONFLICT (project_id, resource) DO UPDATE SET
                     etag = excluded.etag,
                     last_synced_at = excluded.last_synced_at",
                params![project_id, ISSUES_RESOURCE, etag],
            )
            .map_err(db_err)?;
        }

        tx.commit().map_err(db_err)
    }

    /// 本地编辑已发布的 issue。
    ///
    /// 先读取远端当前 `updated_at`，与本地保存值不一致时返回
    /// [`REMOTE_CHANGED`]，绝不覆盖别人的修改。
    pub fn update_remote_issue(
        &self,
        item_id: &str,
        title: Option<String>,
        body: Option<String>,
    ) -> Result<(), String> {
        if title.is_none() && body.is_none() {
            return Ok(());
        }

        let (repo, number, local_updated_at) = {
            let db = self.lock_db()?;
            let (project_id, number, updated_at): (String, Option<i64>, Option<String>) = db
                .conn()
                .query_row(
                    "SELECT project_id, github_number, github_updated_at
                     FROM items WHERE id = ?1",
                    params![item_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(db_err)?
                .ok_or_else(|| format!("条目不存在：{item_id}"))?;
            let number = number.ok_or_else(|| "该条目尚未关联 GitHub issue".to_string())?;
            let project = db
                .get_project(&project_id)?
                .ok_or_else(|| format!("项目不存在：{project_id}"))?;
            let repo = repo_slug(&project)?.ok_or_else(|| "项目未配置 GitHub 仓库".to_string())?;
            (repo, number as u64, updated_at)
        };

        let remote = self.ops.get_issue(&repo, number)?;
        if local_updated_at.as_deref() != Some(remote.updated_at.as_str()) {
            return Err(REMOTE_CHANGED.to_string());
        }

        let patch = IssuePatch {
            title,
            body,
            state: None,
            assignee: None,
            expected_updated_at: remote.updated_at,
        };
        let updated = self
            .ops
            .update_issue(&repo, number, &patch)
            .map_err(|err| {
                if err.contains("远端已变更") {
                    REMOTE_CHANGED.to_string()
                } else {
                    err
                }
            })?;

        let db = self.lock_db()?;
        db.conn()
            .execute(
                "UPDATE items
                 SET title = ?2,
                     body = ?3,
                     github_updated_at = ?4,
                     ghost = 0,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?1",
                params![item_id, updated.title, updated.body, updated.updated_at],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// 把任务发布为 GitHub issue；已发布时直接返回既有编号。
    ///
    /// 发布过程在 `publish_lock` 下串行执行：进入临界区后重新读取
    /// `github_number`，因此并发调用只会创建一次远端 issue。创建成功但本地关联
    /// 失败时，错误信息包含远端编号，用户可按编号手动关联而无需再次发布；issue
    /// 正文末尾保留 `temli-task` 标记，便于后续同步核对来源。
    pub fn publish_task(&self, task_id: &str) -> Result<u64, String> {
        let _guard = self
            .publish_lock
            .lock()
            .map_err(|_| "发布锁被占用".to_string())?;

        let (item_id, project_id, title, body, number, ghost, github_state) = {
            let db = self.lock_db()?;
            db.conn()
                .query_row(
                    "SELECT i.id, i.project_id, i.title, i.body, i.github_number,
                            i.ghost, i.github_state
                     FROM tasks t JOIN items i ON i.id = t.item_id
                     WHERE t.id = ?1",
                    params![task_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, bool>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(db_err)?
                .ok_or_else(|| format!("任务不存在：{task_id}"))?
        };

        if ghost {
            return Err(
                "关联的 GitHub issue 已在远端消失（ghost），请在同步恢复或解除关联后再发布"
                    .to_string(),
            );
        }
        // 已有关联（包括远端已关闭的 issue）一律返回既有编号：关联仍有效，
        // 重新发布会制造第二个 issue。
        if let Some(number) = number {
            return Ok(number as u64);
        }
        // `github_state` 只能是同步/发布写入的 GitHub 侧状态；没有编号却已关闭属于
        // 不一致数据，拒绝发布而不是悄悄创建新 issue。
        if github_state.as_deref() == Some("closed") {
            return Err("条目已关闭但未关联 GitHub issue，请先同步后再发布".to_string());
        }

        let repo = {
            let db = self.lock_db()?;
            let project = db
                .get_project(&project_id)?
                .ok_or_else(|| format!("项目不存在：{project_id}"))?;
            repo_slug(&project)?.ok_or_else(|| "项目未配置 GitHub 仓库".to_string())?
        };

        let marker = format!("<!-- temli-task:{task_id} -->");
        let remote_body = if body.contains(&marker) {
            body
        } else {
            format!("{body}\n\n{marker}")
        };
        let remote = self.ops.create_issue(&repo, &title, &remote_body)?;

        let linked = match self.lock_db() {
            Ok(db) => db
                .conn()
                .execute(
                    "UPDATE items
                     SET github_number = ?2,
                         github_state = ?3,
                         github_updated_at = ?4,
                         ghost = 0,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     WHERE id = ?1",
                    params![
                        item_id,
                        remote.number as i64,
                        remote.state,
                        remote.updated_at
                    ],
                )
                .map(|rows| rows == 1)
                .map_err(|err| err.to_string()),
            Err(err) => Err(err),
        };
        match linked {
            Ok(true) => Ok(remote.number),
            Ok(false) => Err(link_failed(remote.number, "条目已不存在")),
            Err(err) => Err(link_failed(remote.number, &err)),
        }
    }

    fn lock_db(&self) -> Result<MutexGuard<'_, Db>, String> {
        self.db.lock().map_err(|_| "数据库锁被占用".to_string())
    }
}

fn repo_slug(project: &crate::db::Project) -> Result<Option<String>, String> {
    let owner = project
        .github_owner
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let repo = project
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (owner, repo) {
        (Some(owner), Some(repo)) => Ok(Some(format!("{owner}/{repo}"))),
        (None, None) => Ok(None),
        _ => Err("项目 GitHub 配置不完整（需要 owner 与 repo）".to_string()),
    }
}

fn db_err(err: rusqlite::Error) -> String {
    err.to_string()
}

/// 远端 issue 已创建但本地关联失败：错误必须带上编号，避免用户重试时再建一个。
fn link_failed(number: u64, reason: &str) -> String {
    format!(
        "GitHub issue #{number} 已创建，但本地关联失败：{reason}。请按该编号手动关联，避免重复创建。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, RegisterProjectInput};
    use crate::github::{CheckRun, IssuePage, PullRequest, RemoteIssue};
    use crate::store::{self, NewTask, TaskStatus};
    use rusqlite::Connection;
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const REMOTE_UPDATED: &str = "2026-02-01T00:00:00Z";

    struct FakeOps {
        issues: Mutex<Vec<RemoteIssue>>,
        etag: Mutex<Option<String>>,
        not_modified: AtomicBool,
        list_calls: AtomicUsize,
        list_etags: Mutex<Vec<Option<String>>>,
        patches: Mutex<Vec<(u64, IssuePatch)>>,
        created: Mutex<Vec<(String, String)>>,
        next_number: AtomicUsize,
    }

    impl FakeOps {
        fn new(issues: Vec<RemoteIssue>) -> Arc<Self> {
            let next = issues.iter().map(|issue| issue.number).max().unwrap_or(0) + 1;
            Arc::new(Self {
                issues: Mutex::new(issues),
                etag: Mutex::new(Some("etag-1".to_string())),
                not_modified: AtomicBool::new(false),
                list_calls: AtomicUsize::new(0),
                list_etags: Mutex::new(Vec::new()),
                patches: Mutex::new(Vec::new()),
                created: Mutex::new(Vec::new()),
                next_number: AtomicUsize::new(next as usize),
            })
        }

        fn set_issues(&self, issues: Vec<RemoteIssue>) {
            *self.issues.lock().unwrap() = issues;
        }

        fn patches(&self) -> Vec<(u64, IssuePatch)> {
            self.patches.lock().unwrap().clone()
        }
    }

    impl GitHubOps for FakeOps {
        fn list_issues(&self, _repo: &str, etag: Option<&str>) -> Result<IssuePage, String> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            self.list_etags
                .lock()
                .unwrap()
                .push(etag.map(str::to_string));
            if self.not_modified.load(Ordering::SeqCst) {
                return Ok(IssuePage {
                    not_modified: true,
                    etag: self.etag.lock().unwrap().clone(),
                    issues: Vec::new(),
                });
            }
            Ok(IssuePage {
                not_modified: false,
                etag: self.etag.lock().unwrap().clone(),
                issues: self.issues.lock().unwrap().clone(),
            })
        }

        fn get_issue(&self, _repo: &str, number: u64) -> Result<RemoteIssue, String> {
            self.issues
                .lock()
                .unwrap()
                .iter()
                .find(|issue| issue.number == number)
                .cloned()
                .ok_or_else(|| format!("issue 不存在：{number}"))
        }

        fn update_issue(
            &self,
            _repo: &str,
            number: u64,
            patch: &IssuePatch,
        ) -> Result<RemoteIssue, String> {
            self.patches.lock().unwrap().push((number, patch.clone()));
            let mut issues = self.issues.lock().unwrap();
            let issue = issues
                .iter_mut()
                .find(|issue| issue.number == number)
                .ok_or_else(|| format!("issue 不存在：{number}"))?;
            if issue.updated_at != patch.expected_updated_at {
                return Err(REMOTE_CHANGED.to_string());
            }
            if let Some(title) = &patch.title {
                issue.title = title.clone();
            }
            if let Some(body) = &patch.body {
                issue.body = body.clone();
            }
            if let Some(state) = &patch.state {
                issue.state = state.clone();
            }
            issue.updated_at = REMOTE_UPDATED.to_string();
            Ok(issue.clone())
        }

        fn create_issue(
            &self,
            _repo: &str,
            title: &str,
            body: &str,
        ) -> Result<RemoteIssue, String> {
            self.created
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
            let number = self.next_number.fetch_add(1, Ordering::SeqCst) as u64;
            let issue = RemoteIssue {
                number,
                title: title.to_string(),
                body: body.to_string(),
                state: "open".to_string(),
                assignee: None,
                updated_at: REMOTE_UPDATED.to_string(),
            };
            self.issues.lock().unwrap().push(issue.clone());
            Ok(issue)
        }

        fn create_pull_request(
            &self,
            _repo: &str,
            _head: &str,
            _base: &str,
            _title: &str,
            _body: &str,
        ) -> Result<PullRequest, String> {
            Err("not implemented".to_string())
        }

        fn pull_request_checks(&self, _repo: &str, _number: u64) -> Result<Vec<CheckRun>, String> {
            Err("not implemented".to_string())
        }

        fn merge_pull_request(&self, _repo: &str, _number: u64, _sha: &str) -> Result<(), String> {
            Err("not implemented".to_string())
        }
    }

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("temli-sync-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_db() -> Db {
        Db::open(&temp_dir().join("temli.db")).unwrap()
    }

    fn project_with_github(db: &Db) -> String {
        let repo = temp_dir();
        db.insert_project(
            RegisterProjectInput {
                repo_path: repo.to_string_lossy().into_owned(),
                name: Some("demo".to_string()),
                github_owner: Some("acme".to_string()),
                github_repo: Some("temli".to_string()),
                default_branch: Some("main".to_string()),
            },
            repo.canonicalize().unwrap(),
        )
        .unwrap()
        .id
    }

    fn issue(number: u64, title: &str, body: &str, state: &str, updated_at: &str) -> RemoteIssue {
        RemoteIssue {
            number,
            title: title.to_string(),
            body: body.to_string(),
            state: state.to_string(),
            assignee: None,
            updated_at: updated_at.to_string(),
        }
    }

    fn engine(db: &Arc<Mutex<Db>>, ops: &Arc<FakeOps>) -> Arc<SyncEngine> {
        SyncEngine::new(Arc::clone(db), ops.clone())
    }

    fn new_task(db: &Arc<Mutex<Db>>, project_id: &str, title: &str) -> String {
        store::create_task(
            db.lock().unwrap().conn(),
            NewTask {
                project_id: project_id.to_string(),
                title: title.to_string(),
                prompt: String::new(),
                criteria: Vec::new(),
            },
        )
        .unwrap()
        .id
    }

    fn item_by_number(
        conn: &Connection,
        project_id: &str,
        number: i64,
    ) -> (String, String, String, Option<String>, bool) {
        conn.query_row(
            "SELECT title, body, status, github_state, ghost
             FROM items WHERE project_id = ?1 AND github_number = ?2",
            params![project_id, number],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn remote_update_upserts_title_body_and_state() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![issue(
            7,
            "旧标题",
            "旧正文",
            "open",
            "2026-01-01T00:00:00Z",
        )]);
        let engine = engine(&db, &ops);

        engine.sync_project(&project_id).unwrap();
        let (title, body, status, github_state, ghost) =
            item_by_number(db.lock().unwrap().conn(), &project_id, 7);
        assert_eq!(title, "旧标题");
        assert_eq!(body, "旧正文");
        assert_eq!(status, "open");
        assert_eq!(github_state.as_deref(), Some("open"));
        assert!(!ghost);

        ops.set_issues(vec![issue(
            7,
            "新标题",
            "新正文",
            "closed",
            "2026-01-05T00:00:00Z",
        )]);
        engine.sync_project(&project_id).unwrap();
        let (title, body, _, github_state, ghost) =
            item_by_number(db.lock().unwrap().conn(), &project_id, 7);
        assert_eq!(title, "新标题");
        assert_eq!(body, "新正文");
        assert_eq!(github_state.as_deref(), Some("closed"));
        assert!(!ghost);

        // 第二次请求带上首次响应保存的 ETag。
        let etags = ops.list_etags.lock().unwrap();
        assert_eq!(etags.len(), 2);
        assert_eq!(etags[0], None);
        assert_eq!(etags[1].as_deref(), Some("etag-1"));
    }

    #[test]
    fn not_modified_is_a_noop() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![issue(
            7,
            "原标题",
            "原正文",
            "open",
            "2026-01-01T00:00:00Z",
        )]);
        let engine = engine(&db, &ops);
        engine.sync_project(&project_id).unwrap();

        ops.set_issues(vec![issue(
            7,
            "不应写入",
            "不应写入",
            "closed",
            "2026-01-05T00:00:00Z",
        )]);
        ops.not_modified.store(true, Ordering::SeqCst);
        engine.sync_project(&project_id).unwrap();

        let (title, _, status, github_state, ghost) =
            item_by_number(db.lock().unwrap().conn(), &project_id, 7);
        assert_eq!(title, "原标题");
        assert_eq!(status, "open");
        assert_eq!(github_state.as_deref(), Some("open"));
        assert!(!ghost);
        assert_eq!(ops.list_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn removed_issue_becomes_ghost_without_losing_history() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![
            issue(7, "七", "正文七", "open", "2026-01-01T00:00:00Z"),
            issue(8, "八", "正文八", "open", "2026-01-01T00:00:00Z"),
        ]);
        let engine = engine(&db, &ops);

        // 任务先关联 #7，再让首次同步把它按 GitHub 数据补全。
        let task_id = {
            let db = db.lock().unwrap();
            let task = store::create_task(
                db.conn(),
                NewTask {
                    project_id: project_id.clone(),
                    title: "七".to_string(),
                    prompt: String::new(),
                    criteria: Vec::new(),
                },
            )
            .unwrap();
            let item_id = store::task_item_id(db.conn(), &task.id).unwrap();
            db.conn()
                .execute(
                    "UPDATE items SET github_number = 7, github_updated_at = ?2 WHERE id = ?1",
                    params![item_id, "2026-01-01T00:00:00Z"],
                )
                .unwrap();
            store::start_run(
                db.conn(),
                store::NewRun {
                    id: None,
                    task_id: task.id.clone(),
                    adapter_id: "fake".to_string(),
                    adapter_version: None,
                    model: None,
                    branch: None,
                    worktree_path: None,
                    base_sha: None,
                },
            )
            .unwrap();
            task.id
        };
        engine.sync_project(&project_id).unwrap();

        ops.set_issues(vec![issue(
            8,
            "八",
            "正文八",
            "open",
            "2026-01-01T00:00:00Z",
        )]);
        engine.sync_project(&project_id).unwrap();

        let db = db.lock().unwrap();
        let (title, _, status, _, ghost) = item_by_number(db.conn(), &project_id, 7);
        assert_eq!(title, "七");
        assert_eq!(status, "draft");
        assert!(ghost);
        let runs: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(runs, 1);
        let tasks: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 1);
    }

    #[test]
    fn sync_never_clobbers_task_status() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![issue(
            7,
            "远端标题",
            "远端正文",
            "open",
            "2026-01-01T00:00:00Z",
        )]);
        let engine = engine(&db, &ops);

        {
            let db = db.lock().unwrap();
            let task = store::create_task(
                db.conn(),
                NewTask {
                    project_id: project_id.clone(),
                    title: "本地任务".to_string(),
                    prompt: String::new(),
                    criteria: Vec::new(),
                },
            )
            .unwrap();
            let item_id = store::task_item_id(db.conn(), &task.id).unwrap();
            db.conn()
                .execute(
                    "UPDATE items SET github_number = 7, github_updated_at = ?2 WHERE id = ?1",
                    params![item_id, "2026-01-01T00:00:00Z"],
                )
                .unwrap();
            assert_eq!(task.status, TaskStatus::Draft);
        }
        engine.sync_project(&project_id).unwrap();

        ops.set_issues(vec![issue(
            7,
            "远端新标题",
            "远端新正文",
            "closed",
            "2026-01-05T00:00:00Z",
        )]);
        engine.sync_project(&project_id).unwrap();

        let db = db.lock().unwrap();
        let (title, body, status, github_state, ghost) = item_by_number(db.conn(), &project_id, 7);
        assert_eq!(title, "远端新标题");
        assert_eq!(body, "远端新正文");
        assert_eq!(status, "draft");
        assert_eq!(github_state.as_deref(), Some("closed"));
        assert!(!ghost);
    }

    #[test]
    fn stale_local_edit_is_rejected() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![issue(
            7,
            "远端标题",
            "远端正文",
            "open",
            "2026-01-02T00:00:00Z",
        )]);
        let engine = engine(&db, &ops);
        engine.sync_project(&project_id).unwrap();

        let item_id: String = db
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT id FROM items WHERE project_id = ?1 AND github_number = 7",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap();
        // 模拟本地时间戳落后于远端。
        db.lock()
            .unwrap()
            .conn()
            .execute(
                "UPDATE items SET github_updated_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
                params![item_id],
            )
            .unwrap();

        let err = engine
            .update_remote_issue(&item_id, Some("本地标题".to_string()), None)
            .unwrap_err();
        assert_eq!(err, REMOTE_CHANGED);
        assert!(ops.patches().is_empty());
        let (title, _, _, _, _) = item_by_number(db.lock().unwrap().conn(), &project_id, 7);
        assert_eq!(title, "远端标题");
    }

    #[test]
    fn fresh_local_edit_succeeds() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(vec![issue(
            7,
            "远端标题",
            "远端正文",
            "open",
            "2026-01-01T00:00:00Z",
        )]);
        let engine = engine(&db, &ops);
        engine.sync_project(&project_id).unwrap();

        let item_id: String = db
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT id FROM items WHERE project_id = ?1 AND github_number = 7",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap();

        engine
            .update_remote_issue(&item_id, Some("本地新标题".to_string()), None)
            .unwrap();

        let patches = ops.patches();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, 7);
        assert_eq!(patches[0].1.expected_updated_at, "2026-01-01T00:00:00Z");
        assert_eq!(patches[0].1.title.as_deref(), Some("本地新标题"));

        let (title, body, _, _, _) = item_by_number(db.lock().unwrap().conn(), &project_id, 7);
        assert_eq!(title, "本地新标题");
        assert_eq!(body, "远端正文");
        let updated_at: String = db
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT github_updated_at FROM items WHERE id = ?1",
                params![item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, REMOTE_UPDATED);
    }

    #[test]
    fn publish_task_links_created_issue() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(Vec::new());
        let engine = engine(&db, &ops);

        let task = {
            let db = db.lock().unwrap();
            store::create_task(
                db.conn(),
                NewTask {
                    project_id: project_id.clone(),
                    title: "发布任务".to_string(),
                    prompt: String::new(),
                    criteria: Vec::new(),
                },
            )
            .unwrap()
        };

        let number = engine.publish_task(&task.id).unwrap();
        assert_eq!(number, 1);
        {
            let created = ops.created.lock().unwrap();
            assert_eq!(created.len(), 1);
            assert_eq!(created[0].0, "发布任务");
            assert!(
                created[0]
                    .1
                    .contains(&format!("<!-- temli-task:{} -->", task.id)),
                "创建正文应带任务标记：{}",
                created[0].1
            );
        }
        {
            let db = db.lock().unwrap();
            let item_id = store::task_item_id(db.conn(), &task.id).unwrap();
            let (github_number, github_state): (i64, String) = db
                .conn()
                .query_row(
                    "SELECT github_number, github_state FROM items WHERE id = ?1",
                    params![item_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(github_number, 1);
            assert_eq!(github_state, "open");
        }

        // 重复发布会直接返回既有编号，不重复创建。
        assert_eq!(engine.publish_task(&task.id).unwrap(), 1);
        assert_eq!(ops.created.lock().unwrap().len(), 1);
    }

    #[test]
    fn publish_task_returns_existing_link_without_creating() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(Vec::new());
        let engine = engine(&db, &ops);
        let task_id = new_task(&db, &project_id, "已发布任务");

        let item_id = store::task_item_id(db.lock().unwrap().conn(), &task_id).unwrap();
        db.lock()
            .unwrap()
            .conn()
            .execute(
                "UPDATE items SET github_number = 42, github_state = 'open' WHERE id = ?1",
                params![item_id],
            )
            .unwrap();

        assert_eq!(engine.publish_task(&task_id).unwrap(), 42);
        assert!(ops.created.lock().unwrap().is_empty());
    }

    #[test]
    fn publish_task_concurrent_calls_create_one_issue() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(Vec::new());
        let engine = engine(&db, &ops);
        let task_id = new_task(&db, &project_id, "并发发布");

        let barrier = Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let engine = Arc::clone(&engine);
            let barrier = Arc::clone(&barrier);
            let task_id = task_id.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                engine.publish_task(&task_id)
            }));
        }

        let results: Vec<Result<u64, String>> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        for result in &results {
            assert_eq!(*result.as_ref().unwrap(), 1);
        }
        assert_eq!(ops.created.lock().unwrap().len(), 1);
    }

    #[test]
    fn publish_task_reports_remote_number_when_link_fails() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(Vec::new());
        let engine = engine(&db, &ops);
        let task_id = new_task(&db, &project_id, "关联失败");

        db.lock()
            .unwrap()
            .conn()
            .execute_batch(
                "CREATE TRIGGER block_publish_link
                 BEFORE UPDATE OF github_number ON items
                 BEGIN SELECT RAISE(ABORT, 'link blocked'); END;",
            )
            .unwrap();

        let err = engine.publish_task(&task_id).unwrap_err();
        assert_eq!(ops.created.lock().unwrap().len(), 1);
        assert!(
            err.contains("#1"),
            "错误信息应包含已创建的 issue 编号：{err}"
        );
        assert!(err.contains("link blocked"), "错误信息应保留原因：{err}");

        let item_id = store::task_item_id(db.lock().unwrap().conn(), &task_id).unwrap();
        let number: Option<i64> = db
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT github_number FROM items WHERE id = ?1",
                params![item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(number, None);
    }

    #[test]
    fn publish_task_refuses_ghost_item() {
        let db = Arc::new(Mutex::new(open_db()));
        let project_id = project_with_github(&db.lock().unwrap());
        let ops = FakeOps::new(Vec::new());
        let engine = engine(&db, &ops);
        let task_id = new_task(&db, &project_id, "已消失");

        let item_id = store::task_item_id(db.lock().unwrap().conn(), &task_id).unwrap();
        db.lock()
            .unwrap()
            .conn()
            .execute(
                "UPDATE items SET github_number = 7, github_state = 'open', ghost = 1
                 WHERE id = ?1",
                params![item_id],
            )
            .unwrap();

        let err = engine.publish_task(&task_id).unwrap_err();
        assert!(err.contains("ghost"), "错误信息应说明 ghost 状态：{err}");
        assert!(ops.created.lock().unwrap().is_empty());
    }
}
