use super::*;
use crate::db::{Db, RegisterProjectInput};
use rusqlite::{Connection, params};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

pub(crate) fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("temli-store-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn open_db() -> Db {
    Db::open(&temp_dir().join("temli.db")).unwrap()
}

pub(crate) fn project(db: &Db) -> String {
    let repo = temp_dir();
    db.insert_project(
        RegisterProjectInput {
            repo_path: repo.to_string_lossy().into_owned(),
            name: Some("demo".into()),
            github_owner: None,
            github_repo: None,
            default_branch: Some("main".into()),
        },
        repo.canonicalize().unwrap(),
    )
    .unwrap()
    .id
}

pub(crate) fn criteria() -> Vec<Criterion> {
    vec![
        Criterion {
            id: "c1".into(),
            cmd: "cargo test".into(),
            expect_exit: 0,
            expect_match: None,
        },
        Criterion {
            id: "c2".into(),
            cmd: "cargo clippy -- -D warnings".into(),
            expect_exit: 0,
            expect_match: Some("Finished".into()),
        },
    ]
}

pub(crate) fn new_task(project_id: &str, criteria: Vec<Criterion>) -> NewTask {
    NewTask {
        project_id: project_id.to_string(),
        title: "实现证据门".into(),
        prompt: "实现证据门并跑通验收".into(),
        criteria,
    }
}

pub(crate) fn new_run(task_id: &str) -> NewRun {
    NewRun {
        id: None,
        task_id: task_id.to_string(),
        adapter_id: "fake".into(),
        adapter_version: Some("0.1.0".into()),
        model: None,
        branch: Some("temli/1-run".into()),
        worktree_path: Some("/tmp/temli-wt".into()),
        base_sha: None,
    }
}

pub(crate) fn start_task_run(db: &Db, task_id: &str) -> RunRow {
    start_run(db.conn(), new_run(task_id)).unwrap()
}

pub(crate) fn ready_to_running(conn: &Connection, task_id: &str) {
    transition(conn, task_id, TaskStatus::Ready).unwrap();
    transition(conn, task_id, TaskStatus::Queued).unwrap();
    transition(conn, task_id, TaskStatus::Running).unwrap();
}

pub(crate) fn create_queued_task(conn: &Connection, project_id: &str, title: &str) -> TaskRow {
    let mut input = new_task(project_id, criteria());
    input.title = title.to_string();
    let task = create_task(conn, input).unwrap();
    transition(conn, &task.id, TaskStatus::Ready).unwrap();
    transition(conn, &task.id, TaskStatus::Queued).unwrap();
    task
}

pub(crate) fn insert_item(
    conn: &Connection,
    project_id: &str,
    kind: &str,
    title: &str,
    body: &str,
) -> String {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO items (id, project_id, kind, title, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, project_id, kind, title, body],
    )
    .unwrap();
    id
}

pub(crate) fn merge_task(conn: &Connection, task_id: &str) {
    transition(conn, task_id, TaskStatus::Running).unwrap();
    transition(conn, task_id, TaskStatus::Verifying).unwrap();
    transition(conn, task_id, TaskStatus::Merged).unwrap();
}

pub(crate) fn set_item_created_at(conn: &Connection, task_id: &str, created_at: &str) {
    conn.execute(
        "UPDATE items SET created_at = ?2
         WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
        rusqlite::params![task_id, created_at],
    )
    .unwrap();
}

pub(crate) fn write_evidence_output(contents: &str) -> String {
    let path = temp_dir().join("evidence.log");
    fs::write(&path, contents).unwrap();
    path.to_string_lossy().into_owned()
}

pub(crate) fn evidence(run_id: &str, criterion_id: &str, exit_code: i32) -> NewEvidence {
    let output_path = write_evidence_output("Finished\n");
    evidence_with_output(run_id, criterion_id, exit_code, Some(output_path))
}

pub(crate) fn evidence_with_output(
    run_id: &str,
    criterion_id: &str,
    exit_code: i32,
    output_path: Option<String>,
) -> NewEvidence {
    NewEvidence {
        run_id: run_id.to_string(),
        criterion_id: Some(criterion_id.to_string()),
        kind: "test".into(),
        cmd: Some("cargo test".into()),
        exit_code: Some(exit_code),
        duration_ms: Some(42),
        output_path,
    }
}

/// 不挂验收标准的辅助证据：artifact / review 两类，退出码为 0 也不该计入
/// 验收证据的通过数。
pub(crate) fn auxiliary_evidence(run_id: &str, kind: &str) -> NewEvidence {
    NewEvidence {
        run_id: run_id.to_string(),
        criterion_id: None,
        kind: kind.into(),
        cmd: None,
        exit_code: Some(0),
        duration_ms: Some(7),
        output_path: None,
    }
}
