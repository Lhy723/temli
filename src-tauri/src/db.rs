use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo_path: String,
    pub github_owner: Option<String>,
    pub github_repo: Option<String>,
    pub default_branch: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterProjectInput {
    pub repo_path: String,
    pub name: Option<String>,
    pub github_owner: Option<String>,
    pub github_repo: Option<String>,
    pub default_branch: Option<String>,
}

pub struct Db {
    conn: Connection,
}

const V1_SCHEMA: &str = r#"
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    repo_path TEXT NOT NULL UNIQUE,
    github_owner TEXT,
    github_repo TEXT,
    default_branch TEXT NOT NULL DEFAULT 'main',
    concurrency_limit INTEGER NOT NULL DEFAULT 1,
    settings_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE items (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('note','doc','issue')),
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    file_path TEXT,
    body_hash TEXT,
    github_number INTEGER,
    github_updated_at TEXT,
    github_version TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE VIRTUAL TABLE items_fts USING fts5(title, body);

CREATE TABLE links (
    id TEXT PRIMARY KEY,
    from_item TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    to_item TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    rel TEXT NOT NULL CHECK (rel IN ('derived_from','implements','blocks','relates')),
    UNIQUE (from_item, to_item, rel)
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL UNIQUE REFERENCES items(id) ON DELETE CASCADE,
    acceptance_json TEXT NOT NULL DEFAULT '[]',
    priority INTEGER NOT NULL DEFAULT 0,
    max_minutes INTEGER NOT NULL DEFAULT 30,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL DEFAULT 1,
    adapter_id TEXT NOT NULL,
    adapter_version TEXT,
    model TEXT,
    worktree_path TEXT,
    branch TEXT,
    base_sha TEXT,
    head_sha TEXT,
    status TEXT NOT NULL DEFAULT 'prepared',
    started_at TEXT,
    ended_at TEXT,
    session_id TEXT,
    error TEXT,
    UNIQUE (task_id, attempt)
);

CREATE TABLE evidence (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    criterion_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('test','build','lint','review','artifact')),
    cmd TEXT,
    exit_code INTEGER,
    duration_ms INTEGER,
    output_path TEXT,
    collected_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE reviews (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE CASCADE,
    verdict TEXT NOT NULL CHECK (verdict IN ('pass','fail','needs_human')),
    summary TEXT,
    risk_flags_json TEXT NOT NULL DEFAULT '[]',
    test_diff_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE decisions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    prompt TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved','expired')),
    resolution TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    resolved_at TEXT
);

CREATE TABLE sync_state (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    resource TEXT NOT NULL,
    etag TEXT,
    last_synced_at TEXT,
    PRIMARY KEY (project_id, resource)
);

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    entity TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    type TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE VIEW task_dependencies AS
SELECT t.item_id AS task_item, b.item_id AS depends_on_item
FROM links l
JOIN tasks t ON t.item_id = l.to_item
JOIN tasks b ON b.item_id = l.from_item
WHERE l.rel = 'blocks';
"#;

impl Db {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                );",
            )
            .map_err(|e| e.to_string())?;

        let current: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        if current < 1 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(V1_SCHEMA).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (1)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        if current < 2 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(
                "ALTER TABLE tasks ADD COLUMN prompt TEXT NOT NULL DEFAULT '';
                 CREATE INDEX IF NOT EXISTS idx_items_project_status ON items(project_id, status);
                 CREATE INDEX IF NOT EXISTS idx_runs_task ON runs(task_id);
                 CREATE INDEX IF NOT EXISTS idx_evidence_run ON evidence(run_id);
                 CREATE INDEX IF NOT EXISTS idx_decisions_status ON decisions(status);
                 CREATE INDEX IF NOT EXISTS idx_events_entity ON events(entity, entity_id);",
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (2)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        if current < 3 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(
                "ALTER TABLE items ADD COLUMN body TEXT NOT NULL DEFAULT '';
                 ALTER TABLE items ADD COLUMN github_state TEXT;
                 ALTER TABLE items ADD COLUMN github_assignee TEXT;
                 ALTER TABLE items ADD COLUMN ghost INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE projects ADD COLUMN auto_merge INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE runs ADD COLUMN pr_number INTEGER;
                 ALTER TABLE runs ADD COLUMN pr_url TEXT;
                 ALTER TABLE runs ADD COLUMN pr_state TEXT;
                 CREATE INDEX IF NOT EXISTS idx_items_github ON items(project_id, github_number);",
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (3)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        if current < 4 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(
                "ALTER TABLE projects ADD COLUMN require_checks INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (4)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        if current < 5 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
                    entity_id UNINDEXED,
                    project_id UNINDEXED,
                    kind UNINDEXED,
                    title,
                    body
                );",
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (5)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        if current < 6 {
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(
                "DROP TABLE IF EXISTS search_fts;
                 CREATE VIRTUAL TABLE search_fts USING fts5(
                    entity_id UNINDEXED,
                    project_id UNINDEXED,
                    kind UNINDEXED,
                    title,
                    body,
                    tokenize='trigram'
                 );",
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (6)", [])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, repo_path, github_owner, github_repo, default_branch, created_at
                 FROM projects ORDER BY created_at ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_path: row.get(2)?,
                    github_owner: row.get(3)?,
                    github_repo: row.get(4)?,
                    default_branch: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, repo_path, github_owner, github_repo, default_branch, created_at
                 FROM projects WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![id], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_path: row.get(2)?,
                    github_owner: row.get(3)?,
                    github_repo: row.get(4)?,
                    default_branch: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.next().transpose().map_err(|e| e.to_string())
    }

    pub fn insert_project(
        &self,
        input: RegisterProjectInput,
        canonical: PathBuf,
    ) -> Result<Project, String> {
        let repo_path = canonical.to_string_lossy().into_owned();
        let name = input
            .name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "project".to_string());
        let branch = input
            .default_branch
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| "main".to_string());
        let id = Uuid::new_v4().to_string();

        self.conn
            .execute(
                "INSERT INTO projects (id, name, repo_path, github_owner, github_repo, default_branch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    name,
                    repo_path,
                    input.github_owner,
                    input.github_repo,
                    branch
                ],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    "该项目已注册".to_string()
                } else {
                    e.to_string()
                }
            })?;

        self.conn
            .query_row(
                "SELECT id, name, repo_path, github_owner, github_repo, default_branch, created_at
                 FROM projects WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Project {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        repo_path: row.get(2)?,
                        github_owner: row.get(3)?,
                        github_repo: row.get(4)?,
                        default_branch: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                },
            )
            .map_err(|e| e.to_string())
    }
}

pub fn canonical_repo(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if !candidate.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("无法解析路径：{e}"))?;
    if !canonical.is_dir() {
        return Err("路径不是目录".to_string());
    }

    let canonical_str = canonical.to_string_lossy().into_owned();
    let worktree = Command::new("git")
        .args(["-C", &canonical_str, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|e| format!("无法执行 git：{e}"))?;
    if !worktree.status.success() || String::from_utf8_lossy(&worktree.stdout).trim() != "true" {
        return Err("路径不是 git 工作区".to_string());
    }

    let ignored = Command::new("git")
        .args(["-C", &canonical_str, "check-ignore", "-q", ".temli/"])
        .status()
        .map_err(|e| format!("无法执行 git：{e}"))?;
    if !ignored.success() {
        return Err("`.temli/` 未被 .gitignore 忽略，请先添加该条目".to_string());
    }

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("temli-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_init(dir: &Path) {
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .expect("git");
        assert!(status.success());
    }

    fn repo_with_ignore() -> PathBuf {
        let dir = temp_dir();
        git_init(&dir);
        fs::write(dir.join(".gitignore"), ".temli/\n").unwrap();
        dir
    }

    #[test]
    fn rejects_non_git_directory() {
        let dir = temp_dir();
        let err = canonical_repo(dir.to_str().unwrap()).unwrap_err();
        assert!(err.contains("不是 git 工作区"), "{err}");
    }

    #[test]
    fn rejects_repo_without_temli_ignore() {
        let dir = temp_dir();
        git_init(&dir);
        let err = canonical_repo(dir.to_str().unwrap()).unwrap_err();
        assert!(err.contains(".temli/"), "{err}");
    }

    #[test]
    fn accepts_repo_with_temli_ignore() {
        let dir = repo_with_ignore();
        let path = canonical_repo(dir.to_str().unwrap()).unwrap();
        assert_eq!(path, dir.canonicalize().unwrap());
    }

    #[test]
    fn migrations_are_idempotent_and_projects_roundtrip() {
        let repo = repo_with_ignore();
        let canonical = repo.canonicalize().unwrap();
        let db_path = temp_dir().join("temli.db");

        {
            let db = Db::open(&db_path).unwrap();
            let project = db
                .insert_project(
                    RegisterProjectInput {
                        repo_path: repo.to_string_lossy().into_owned(),
                        name: Some("demo".into()),
                        github_owner: Some("Lhy723".into()),
                        github_repo: Some("temli-demo".into()),
                        default_branch: Some("main".into()),
                    },
                    canonical.clone(),
                )
                .unwrap();
            assert_eq!(project.name, "demo");
            assert_eq!(project.github_owner.as_deref(), Some("Lhy723"));
            assert_eq!(db.list_projects().unwrap().len(), 1);
        }

        let reopened = Db::open(&db_path).unwrap();
        let projects = reopened.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "demo");

        let err = reopened
            .insert_project(
                RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: None,
                    github_owner: None,
                    github_repo: None,
                    default_branch: None,
                },
                canonical,
            )
            .unwrap_err();
        assert!(err.contains("已注册"), "{err}");
    }
}
