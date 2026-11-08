//! 全局搜索：一次查询同时命中任务 / 同步 issue、文档与全部笔记库。
//!
//! 权威边界：
//! - 任务与同步 issue 都是 `items(kind='issue')`，统一映射为 `task` 命中；
//!   有 `tasks` 行时命中 id 用任务 id（前端按任务 id 深链），未转换的同步
//!   issue 回退条目 id，只能定位到项目、无法选中具体任务；
//! - 文档来自 `items(kind='doc')`，指定项目时只搜该项目；
//! - 笔记来自 `vault_notes`，不受项目过滤（笔记库与项目无关）；
//! - 两字中文查询也要命中，因此统一走 LIKE（trigram FTS 至少三字符）。

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 50;
/// 命中词前后各保留的字符数。
const SNIPPET_RADIUS: usize = 40;
/// 没有命中词可定位时截取的字符数。
const SNIPPET_FALLBACK: usize = 80;

/// 全局搜索命中：`kind` 为 `task` / `doc` / `note`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalHit {
    pub kind: String,
    /// 任务命中为 `tasks.id`（未转换的同步 issue 为条目 id，无法深链到具体任务），
    /// 文档命中为 `items.id`，笔记命中为 `vault_notes.id`。
    pub id: String,
    pub title: String,
    pub snippet: String,
    /// 任务 / 文档所属项目；笔记为 `None`。
    pub project_id: Option<String>,
    /// 笔记所属笔记库；任务 / 文档为 `None`。
    pub vault_id: Option<String>,
}

/// 跨项目任务、项目文档与笔记库的统一 LIKE 检索。
///
/// 空查询返回空列表；`limit` 超过 50 时封顶，传 0 使用默认 20。
/// 结果先按种类优先级（task > doc > note），同种类内按 `updated_at` 倒序。
pub fn global_search(
    conn: &Connection,
    query: &str,
    project_id: Option<&str>,
    limit: usize,
) -> Result<Vec<GlobalHit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let project_id = project_id.map(str::trim).filter(|id| !id.is_empty());
    let limit = if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX_LIMIT)
    };
    let pattern = crate::workspace::like_pattern(query);
    let mut stmt = conn
        .prepare(
            // 列：id, kind, title, body, project_id, vault_id, updated_at, priority。
            // 有 tasks 行的 issue 用任务 id，前端 focusTaskId 按任务 id 匹配。
            "SELECT COALESCE(t.id, i.id) AS id, 'task' AS kind, i.title, i.body,
                    i.project_id, NULL AS vault_id, i.updated_at, 0 AS priority
             FROM items i
             LEFT JOIN tasks t ON t.item_id = i.id
             WHERE i.kind = 'issue'
               AND (?1 IS NULL OR i.project_id = ?1)
               AND (i.title LIKE ?2 ESCAPE '\\' OR i.body LIKE ?2 ESCAPE '\\')
             UNION ALL
             SELECT i.id, 'doc', i.title, i.body, i.project_id, NULL, i.updated_at, 1
             FROM items i
             WHERE i.kind = 'doc'
               AND (?1 IS NULL OR i.project_id = ?1)
               AND (i.title LIKE ?2 ESCAPE '\\' OR i.body LIKE ?2 ESCAPE '\\')
             UNION ALL
             SELECT n.id, 'note', n.title, n.body, NULL, n.vault_id, n.updated_at, 2
             FROM vault_notes n
             WHERE n.title LIKE ?2 ESCAPE '\\' OR n.body LIKE ?2 ESCAPE '\\'
             ORDER BY priority ASC, updated_at DESC, id ASC
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id, pattern, limit as i64], |row| {
            let title: String = row.get(2)?;
            let body: String = row.get(3)?;
            Ok(GlobalHit {
                id: row.get(0)?,
                kind: row.get(1)?,
                snippet: make_snippet(&title, &body, query),
                title,
                project_id: row.get(4)?,
                vault_id: row.get(5)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 命中词前后各 40 字符，用 `[..]` 标出命中；找不到时退回正文前 80 字符。
fn make_snippet(title: &str, body: &str, needle: &str) -> String {
    if let Some((start, end)) = find_ci(body, needle) {
        return around(body, start, end);
    }
    if let Some((start, end)) = find_ci(title, needle) {
        return around(title, start, end);
    }
    let fallback: String = body.chars().take(SNIPPET_FALLBACK).collect();
    if fallback.trim().is_empty() {
        title.chars().take(SNIPPET_FALLBACK).collect()
    } else {
        fallback
    }
}

/// 与 SQLite `LIKE` 一致：ASCII 大小写不敏感，其余字符精确比较。
fn find_ci(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() {
        return None;
    }
    for (start, _) in haystack.char_indices() {
        let mut end = start;
        let mut matched = true;
        let mut chars = haystack[start..].chars();
        for needle_char in &needle_chars {
            match chars.next() {
                Some(hay_char) if hay_char.eq_ignore_ascii_case(needle_char) => {
                    end += hay_char.len_utf8();
                }
                _ => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            return Some((start, end));
        }
    }
    None
}

/// 截取 `[start, end)` 前后各 40 字符并加省略号。
fn around(text: &str, start: usize, end: usize) -> String {
    let before = text[..start]
        .char_indices()
        .rev()
        .nth(SNIPPET_RADIUS - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let after = text[end..]
        .char_indices()
        .nth(SNIPPET_RADIUS)
        .map(|(index, _)| end + index)
        .unwrap_or(text.len());
    format!(
        "…{}[{}]{}…",
        &text[before..start],
        &text[start..end],
        &text[end..after]
    )
}

fn db_err(err: rusqlite::Error) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, Project, RegisterProjectInput};
    use crate::store::{NewTask, create_task, task_item_id};
    use crate::vaults::{Vault, add_vault, create_vault_note};
    use crate::workspace::{list_docs, scan_docs};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        db: Db,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "temli-search-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("创建临时目录");
            let db = Db::open(&root.join("temli.db")).expect("打开临时数据库");
            Self { root, db }
        }

        fn project(&self, name: &str) -> Project {
            let repo = self.root.join(name);
            fs::create_dir_all(&repo).expect("创建仓库目录");
            let canonical = repo.canonicalize().expect("规范化仓库路径");
            self.db
                .insert_project(
                    RegisterProjectInput {
                        repo_path: canonical.to_string_lossy().into_owned(),
                        name: Some(name.to_string()),
                        github_owner: None,
                        github_repo: None,
                        default_branch: Some("main".to_string()),
                    },
                    canonical,
                )
                .expect("登记项目")
        }

        fn vault(&self, name: &str) -> Vault {
            let dir = self.root.join(name);
            add_vault(
                self.db.conn(),
                name,
                dir.to_str().expect("路径为 UTF-8"),
                true,
            )
            .expect("注册笔记库")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_doc(project: &Project, relative: &str, contents: &str) {
        let path = Path::new(&project.repo_path).join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("创建文档目录");
        fs::write(path, contents).expect("写入文档");
    }

    /// 任务的条目 id 用于改 `items.updated_at`，任务 id 才是命中结果里的 id。
    struct CreatedTask {
        task_id: String,
        item_id: String,
    }

    fn add_task(conn: &Connection, project_id: &str, title: &str) -> CreatedTask {
        let task = create_task(
            conn,
            NewTask {
                project_id: project_id.to_string(),
                title: title.to_string(),
                prompt: String::new(),
                criteria: Vec::new(),
            },
        )
        .expect("创建任务");
        let item_id = task_item_id(conn, &task.id).expect("读取任务条目");
        CreatedTask {
            task_id: task.id,
            item_id,
        }
    }

    fn set_updated_at(conn: &Connection, table: &str, id: &str, at: &str) {
        conn.execute(
            &format!("UPDATE {table} SET updated_at = ?2 WHERE id = ?1"),
            params![id, at],
        )
        .expect("更新时间");
    }

    #[test]
    fn finds_tasks_docs_and_notes_in_priority_order() {
        let fixture = Fixture::new("kinds");
        let conn = fixture.db.conn();
        let project = fixture.project("alpha-repo");
        let vault = fixture.vault("brain");

        write_doc(
            &project,
            "docs/guide.md",
            "# Needle guide\n\nbody around needle\n",
        );
        assert_eq!(scan_docs(conn, &project).unwrap(), 1);
        let doc = list_docs(conn, &project.id).unwrap().remove(0);
        let task = add_task(conn, &project.id, "Needle task");
        let note = create_vault_note(
            conn,
            &vault,
            "needle-note.md",
            Some("Needle note"),
            Some("note body"),
        )
        .unwrap();

        // 时间顺序故意与优先级相反：note 最新，task 最旧。
        set_updated_at(conn, "items", &task.item_id, "2024-01-01T00:00:00.000Z");
        set_updated_at(conn, "items", &doc.id, "2024-01-02T00:00:00.000Z");
        set_updated_at(conn, "vault_notes", &note.id, "2024-01-03T00:00:00.000Z");

        let hits = global_search(conn, "needle", Some(&project.id), 20).unwrap();
        let kinds: Vec<&str> = hits.iter().map(|hit| hit.kind.as_str()).collect();
        assert_eq!(kinds, vec!["task", "doc", "note"]);

        assert_eq!(hits[0].id, task.task_id, "任务命中应返回任务 id");
        assert_eq!(hits[0].project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(hits[0].vault_id, None);
        assert!(hits[0].snippet.contains("[Needle]"), "{}", hits[0].snippet);

        assert_eq!(hits[1].id, doc.id);
        assert_eq!(hits[1].project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(hits[1].vault_id, None);
        assert!(hits[1].snippet.contains("[Needle]"), "{}", hits[1].snippet);

        assert_eq!(hits[2].id, note.id);
        assert_eq!(hits[2].project_id, None);
        assert_eq!(hits[2].vault_id.as_deref(), Some(vault.id.as_str()));
        assert!(hits[2].snippet.contains("[Needle]"), "{}", hits[2].snippet);
    }

    #[test]
    fn orders_same_kind_by_updated_at_desc() {
        let fixture = Fixture::new("order");
        let conn = fixture.db.conn();
        let project = fixture.project("order-repo");
        let older = add_task(conn, &project.id, "Ordered task one");
        let newer = add_task(conn, &project.id, "Ordered task two");
        set_updated_at(conn, "items", &older.item_id, "2024-01-01T00:00:00.000Z");
        set_updated_at(conn, "items", &newer.item_id, "2024-05-01T00:00:00.000Z");

        let hits = global_search(conn, "Ordered", Some(&project.id), 20).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, newer.task_id);
        assert_eq!(hits[1].id, older.task_id);
    }

    #[test]
    fn includes_synced_issues_as_tasks() {
        let fixture = Fixture::new("issue");
        let conn = fixture.db.conn();
        let project = fixture.project("issue-repo");

        // 与 sync.rs 一致：同步 issue 只有 items 行，没有 tasks 行。
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, status, body, github_number)
             VALUES ('issue-1', ?1, 'issue', 'Synced issue', 'open', '复现步骤 synced', 42)",
            params![project.id],
        )
        .unwrap();

        let hits = global_search(conn, "synced", None, 20).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, "task");
        assert_eq!(hits[0].id, "issue-1");
        assert_eq!(hits[0].project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(hits[0].vault_id, None);
        assert!(hits[0].snippet.contains("[synced]"), "{}", hits[0].snippet);
    }

    #[test]
    fn scopes_items_to_project_without_cross_leakage() {
        let fixture = Fixture::new("scope");
        let conn = fixture.db.conn();
        let alpha = fixture.project("alpha");
        let beta = fixture.project("beta");
        let vault = fixture.vault("shared-brain");

        write_doc(&alpha, "docs/a.md", "# Shared\n\nalpha only\n");
        write_doc(&beta, "docs/b.md", "# Shared\n\nbeta only\n");
        scan_docs(conn, &alpha).unwrap();
        scan_docs(conn, &beta).unwrap();
        let alpha_doc = list_docs(conn, &alpha.id).unwrap().remove(0);
        let beta_doc = list_docs(conn, &beta.id).unwrap().remove(0);
        let alpha_task = add_task(conn, &alpha.id, "Shared alpha task");
        let beta_task = add_task(conn, &beta.id, "Shared beta task");
        let note = create_vault_note(
            conn,
            &vault,
            "shared.md",
            Some("Shared note"),
            Some("shared body"),
        )
        .unwrap();

        let scoped = global_search(conn, "Shared", Some(&alpha.id), 20).unwrap();
        let ids: Vec<&str> = scoped.iter().map(|hit| hit.id.as_str()).collect();
        assert!(ids.contains(&alpha_doc.id.as_str()), "{ids:?}");
        assert!(ids.contains(&alpha_task.task_id.as_str()), "{ids:?}");
        assert!(ids.contains(&note.id.as_str()), "笔记不受项目过滤：{ids:?}");
        assert!(
            !ids.contains(&beta_doc.id.as_str()),
            "泄漏了其它项目文档：{ids:?}"
        );
        assert!(
            !ids.contains(&beta_task.task_id.as_str()),
            "泄漏了其它项目任务：{ids:?}"
        );

        let all = global_search(conn, "Shared", None, 20).unwrap();
        let all_ids: Vec<&str> = all.iter().map(|hit| hit.id.as_str()).collect();
        assert!(all_ids.contains(&beta_doc.id.as_str()), "{all_ids:?}");
        assert!(all_ids.contains(&beta_task.task_id.as_str()), "{all_ids:?}");
    }

    #[test]
    fn finds_two_char_cjk_queries() {
        let fixture = Fixture::new("cjk");
        let conn = fixture.db.conn();
        let project = fixture.project("cjk-repo");
        let vault = fixture.vault("cjk-vault");

        write_doc(
            &project,
            "docs/guide.md",
            "# 入门指南\n\n这里讲快速入门。\n",
        );
        scan_docs(conn, &project).unwrap();
        let doc = list_docs(conn, &project.id).unwrap().remove(0);
        let note =
            create_vault_note(conn, &vault, "笔记.md", Some("笔记正题"), Some("正文内容")).unwrap();
        let task = add_task(conn, &project.id, "写笔记");

        let hits = global_search(conn, "笔记", Some(&project.id), 20).unwrap();
        let ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
        assert!(ids.contains(&note.id.as_str()), "{ids:?}");
        assert!(ids.contains(&task.task_id.as_str()), "{ids:?}");
        let note_hit = hits.iter().find(|hit| hit.id == note.id).unwrap();
        assert!(note_hit.snippet.contains("[笔记]"), "{}", note_hit.snippet);

        let doc_hits = global_search(conn, "入门", Some(&project.id), 20).unwrap();
        assert!(doc_hits.iter().any(|hit| hit.id == doc.id), "{doc_hits:?}");
    }

    #[test]
    fn escapes_like_wildcards() {
        let fixture = Fixture::new("escape");
        let conn = fixture.db.conn();
        let project = fixture.project("escape-repo");
        let vault = fixture.vault("escape-vault");

        write_doc(
            &project,
            "docs/percent.md",
            "# 覆盖率\n\n达到 100% coverage\n",
        );
        write_doc(&project, "docs/plain.md", "# 普通\n\n没有任何符号\n");
        scan_docs(conn, &project).unwrap();
        let docs = list_docs(conn, &project.id).unwrap();
        let percent = docs
            .iter()
            .find(|doc| doc.path == "docs/percent.md")
            .expect("索引 percent");
        let plain = docs
            .iter()
            .find(|doc| doc.path == "docs/plain.md")
            .expect("索引 plain");

        let percent_hits = global_search(conn, "%", Some(&project.id), 20).unwrap();
        assert_eq!(percent_hits.len(), 1, "{percent_hits:?}");
        assert_eq!(percent_hits[0].id, percent.id);
        assert!(
            percent_hits[0].snippet.contains("[%]"),
            "{}",
            percent_hits[0].snippet
        );
        assert!(
            !global_search(conn, "%", None, 20)
                .unwrap()
                .iter()
                .any(|hit| hit.id == plain.id),
            "% 不能匹配任意内容"
        );

        let note = create_vault_note(
            conn,
            &vault,
            "snake_case.md",
            Some("snake_case"),
            Some("name"),
        )
        .unwrap();
        let underscore_hits = global_search(conn, "_", None, 20).unwrap();
        assert!(
            underscore_hits.iter().any(|hit| hit.id == note.id),
            "{underscore_hits:?}"
        );
        assert!(
            !underscore_hits.iter().any(|hit| hit.id == plain.id),
            "_ 不能匹配任意单字符"
        );
    }

    #[test]
    fn empty_query_returns_nothing_and_limit_is_clamped() {
        let fixture = Fixture::new("limit");
        let conn = fixture.db.conn();
        let project = fixture.project("limit-repo");

        assert!(
            global_search(conn, "", Some(&project.id), 20)
                .unwrap()
                .is_empty()
        );
        assert!(global_search(conn, "   ", None, 20).unwrap().is_empty());

        for index in 0..60 {
            add_task(conn, &project.id, &format!("Limit hit {index}"));
        }

        assert_eq!(global_search(conn, "Limit", None, 3).unwrap().len(), 3);
        // limit=0 使用默认 20；超过 50 时封顶 50。
        assert_eq!(global_search(conn, "Limit", None, 0).unwrap().len(), 20);
        assert_eq!(global_search(conn, "Limit", None, 100).unwrap().len(), 50);
    }
}
