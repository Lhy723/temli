//! M5 文档工作区：仓库 `docs/**/*.md` 索引与 FTS 全文检索。
//!
//! 权威边界（见 `docs/data-model.md`）：
//! - 文档正文在仓库 `docs/**/*.md`，DB 只存索引，文件是正文权威；
//! - 扫描后写入 `search_fts`，扫描 / 保存后立即可以检索；
//! - 笔记已迁到独立笔记库（`vaults`），本模块不再存储笔记。

use crate::db::Project;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

const DOCS_DIR: &str = "docs";
const SEARCH_LIMIT: i64 = 50;

// ---------------------------------------------------------------------------
// 公开类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocMeta {
    pub id: String,
    pub title: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Doc {
    pub id: String,
    pub title: String,
    pub path: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// 文档
// ---------------------------------------------------------------------------

/// 一次扫描中已读取的文档，写事务只处理这些内存数据。
struct ScannedDoc {
    id: String,
    relative: String,
    title: String,
    body: String,
}

/// 扫描仓库 `docs/**/*.md`，增量更新 `items(kind='doc')` 与 `search_fts`，
/// 并删除源文件已消失的文档。返回本次成功索引的文档数。
pub fn scan_docs(conn: &Connection, project: &Project) -> Result<usize, String> {
    let repo_root = project_path(&project.repo_path);
    if !repo_root.is_dir() {
        return Err(format!("仓库目录不存在：{}", repo_root.display()));
    }
    let docs_root = repo_root.join(DOCS_DIR);
    if let Ok(meta) = fs::symlink_metadata(&docs_root)
        && meta.file_type().is_symlink()
    {
        return Err("docs 目录不能是符号链接".to_string());
    }
    let mut files = Vec::new();
    if docs_root.is_dir() {
        collect_markdown_files(&docs_root, &mut files)?;
    }
    files.sort();

    // 先读完文件再开事务，避免在持有写锁时做磁盘 I/O。
    let mut scanned = Vec::with_capacity(files.len());
    let mut seen_early = HashSet::with_capacity(files.len());
    for file in &files {
        let relative = repo_relative(&repo_root, file);
        let id = doc_id(&project.id, &relative);
        // 读取失败或超限时仍登记，避免被误判为已删除。
        seen_early.insert(id.clone());
        match fs::metadata(file) {
            Ok(meta) if meta.len() > 1024 * 1024 => {
                eprintln!("文档 {} 超过 1 MiB，跳过索引", file.display());
                continue;
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("读取文档 {} 元数据失败，跳过：{err}", file.display());
                continue;
            }
        }
        let body = match fs::read_to_string(file) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("读取文档 {} 失败，跳过：{err}", file.display());
                continue;
            }
        };
        scanned.push(ScannedDoc {
            id,
            title: markdown_title(file, &body),
            relative,
            body,
        });
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let mut seen = seen_early;
    for doc in &scanned {
        tx.execute(
            "INSERT INTO items (id, project_id, kind, title, body, file_path, status)
             VALUES (?1, ?2, 'doc', ?3, ?4, ?5, 'open')
             ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 body = excluded.body,
                 file_path = excluded.file_path,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            params![doc.id, project.id, doc.title, doc.body, doc.relative],
        )
        .map_err(db_err)?;
        index_item(&tx, &project.id, &doc.id, "doc", &doc.title, &doc.body)?;
        seen.insert(doc.id.clone());
    }

    let existing: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT id FROM items WHERE project_id = ?1 AND kind = 'doc'")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![project.id], |row| row.get(0))
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };
    for id in existing {
        if seen.contains(&id) {
            continue;
        }
        tx.execute("DELETE FROM search_fts WHERE entity_id = ?1", params![id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM items WHERE id = ?1", params![id])
            .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(scanned.len())
}

pub fn list_docs(conn: &Connection, project_id: &str) -> Result<Vec<DocMeta>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, COALESCE(file_path, '')
             FROM items
             WHERE project_id = ?1 AND kind = 'doc'
             ORDER BY file_path ASC, title ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(DocMeta {
                id: row.get(0)?,
                title: row.get(1)?,
                path: row.get(2)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 读取文档：优先读源文件，文件不存在时回退到扫描时缓存的正文。
pub fn get_doc(conn: &Connection, doc_id: &str) -> Result<Doc, String> {
    let (mut doc, repo_path) = conn
        .query_row(
            "SELECT i.id, i.title, i.body, COALESCE(i.file_path, ''), p.repo_path
             FROM items i JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1 AND i.kind = 'doc'",
            params![doc_id],
            |row| {
                Ok((
                    Doc {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        body: row.get(2)?,
                        path: row.get(3)?,
                    },
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("文档不存在：{doc_id}"))?;

    if !doc.path.is_empty()
        && let Ok(body) = fs::read_to_string(project_path(&repo_path).join(&doc.path))
    {
        doc.body = body;
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------
// 检索
// ---------------------------------------------------------------------------

/// 项目内文档 FTS5 检索；空查询返回空列表，正文片段用 `[...]` 标出命中词。
pub fn search(conn: &Connection, project_id: &str, query: &str) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    if query.trim().chars().count() < 3 {
        return search_like(conn, project_id, query.trim());
    }
    let Some(match_query) = fts_match_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare(
            // search_fts 列为 entity_id, project_id, kind, title, body；片段取正文列 4。
            "SELECT entity_id, kind, title,
                    snippet(search_fts, 4, '[', ']', '…', 12)
             FROM search_fts
             WHERE search_fts MATCH ?1 AND project_id = ?2 AND kind = 'doc'
             ORDER BY rank
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![match_query, project_id, SEARCH_LIMIT], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                snippet: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 两字查询走 LIKE（trigram 最少三字符），避免中文短词搜不到。
fn search_like(conn: &Connection, project_id: &str, query: &str) -> Result<Vec<SearchHit>, String> {
    let pattern = like_pattern(query);
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, title, COALESCE(body, '')
             FROM items
             WHERE project_id = ?1 AND kind = 'doc'
               AND (title LIKE ?2 ESCAPE '\\' OR body LIKE ?2 ESCAPE '\\')
             ORDER BY updated_at DESC
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id, pattern, SEARCH_LIMIT], |row| {
            let body: String = row.get(3)?;
            Ok(SearchHit {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                snippet: like_snippet(&body, query),
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

pub(crate) fn like_pattern(query: &str) -> String {
    format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

pub(crate) fn like_snippet(body: &str, needle: &str) -> String {
    let Some(byte_pos) = body.find(needle) else {
        return body.chars().take(80).collect();
    };
    let start = body[..byte_pos]
        .char_indices()
        .rev()
        .nth(40)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let end = body[byte_pos..]
        .char_indices()
        .nth(needle.chars().count() + 40)
        .map(|(index, _)| byte_pos + index)
        .unwrap_or(body.len());
    format!(
        "…{}[{}]{}…",
        &body[start..byte_pos],
        needle,
        &body[byte_pos + needle.len()..end]
    )
}

/// 把文档写入 `search_fts`；先删同 id 旧行，保证扫描后立刻是唯一索引。
fn index_item(
    conn: &Connection,
    project_id: &str,
    id: &str,
    kind: &str,
    title: &str,
    body: &str,
) -> Result<(), String> {
    conn.execute("DELETE FROM search_fts WHERE entity_id = ?1", params![id])
        .map_err(db_err)?;
    conn.execute(
        "INSERT INTO search_fts (entity_id, project_id, kind, title, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, project_id, kind, title, body],
    )
    .map_err(db_err)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn db_err(err: rusqlite::Error) -> String {
    err.to_string()
}

fn project_path(repo_path: &str) -> PathBuf {
    PathBuf::from(repo_path)
}

/// 递归收集 `*.md`；跳过隐藏目录（如 `.vitepress`）与符号链接。
fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("读取文档目录 {} 失败：{err}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取文档目录失败：{err}"))?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("读取文档类型失败：{err}"))?;
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            collect_markdown_files(&entry.path(), out)?;
        } else if file_type.is_file() {
            let is_markdown = entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
            if is_markdown {
                out.push(entry.path());
            }
        }
    }
    Ok(())
}

/// FNV-1a：跨 Rust 版本稳定的 64 位哈希。
pub(crate) fn stable_hash(input: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 文档 id：项目 id + 仓库相对路径的稳定哈希，重命名会得到新 id。
fn doc_id(project_id: &str, relative_path: &str) -> String {
    format!(
        "doc-{:016x}",
        stable_hash(&format!("{project_id}:{relative_path}"))
    )
}

/// 首个 `# ` 一级标题（跳过 frontmatter 与代码围栏）。
fn first_heading(body: &str) -> Option<String> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let mut in_frontmatter = false;
    let mut in_fence = false;
    let mut first = true;
    for line in body.lines() {
        let trimmed = line.trim();
        if first {
            first = false;
            if trimmed == "---" {
                in_frontmatter = true;
                continue;
            }
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(heading) = line.trim_start().strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

/// Markdown 标题：首个 H1，否则文件名 stem。
pub(crate) fn markdown_title(path: &Path, body: &str) -> String {
    first_heading(body).unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "untitled".to_string())
    })
}

fn repo_relative(repo_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 将用户输入转成安全的多词 FTS5 查询：每个词做引号转义并启用前缀匹配。
pub(crate) fn fts_match_query(query: &str) -> Option<String> {
    let mut parts = Vec::new();
    for token in query.split_whitespace() {
        if !token.chars().any(|character| character.is_alphanumeric()) {
            continue;
        }
        let escaped = token.replace('"', "\"\"");
        parts.push(format!("\"{escaped}\""));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, RegisterProjectInput};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        repo: PathBuf,
        db: Db,
        project: Project,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "temli-workspace-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let repo = root.join("repo");
            fs::create_dir_all(&repo).expect("创建临时仓库目录");
            let db = Db::open(&root.join("temli.db")).expect("打开临时数据库");
            let project = register(&db, &repo, "demo");
            Self {
                root,
                repo,
                db,
                project,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn register(db: &Db, repo: &Path, name: &str) -> Project {
        let canonical = repo.canonicalize().expect("规范化仓库路径");
        db.insert_project(
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

    fn write_doc(repo: &Path, relative: &str, contents: &str) {
        let path = repo.join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("创建文档目录");
        fs::write(path, contents).expect("写入文档");
    }

    #[test]
    fn docs_scan_indexes_updates_and_removes() {
        let fixture = Fixture::new("docs");
        let conn = fixture.db.conn();
        write_doc(
            &fixture.repo,
            "docs/guides/intro.md",
            "# 入门\n\nfirst alpha body\n",
        );
        write_doc(&fixture.repo, "docs/plain.md", "plain beta body\n");
        write_doc(
            &fixture.repo,
            "docs/.hidden/secret.md",
            "# 隐藏\n\nsecret\n",
        );

        assert_eq!(scan_docs(conn, &fixture.project).unwrap(), 2);
        let docs = list_docs(conn, &fixture.project.id).unwrap();
        assert_eq!(docs.len(), 2);
        let intro = docs
            .iter()
            .find(|doc| doc.path == "docs/guides/intro.md")
            .expect("应索引 intro");
        assert!(intro.id.starts_with("doc-"));
        assert_eq!(intro.title, "入门");
        let plain = docs
            .iter()
            .find(|doc| doc.path == "docs/plain.md")
            .expect("应索引 plain");
        assert_eq!(plain.title, "plain", "没有标题时退回文件名");

        assert_eq!(
            get_doc(conn, &intro.id).unwrap().body,
            "# 入门\n\nfirst alpha body\n"
        );
        let hits = search(conn, &fixture.project.id, "alpha").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, intro.id);
        assert_eq!(hits[0].kind, "doc");

        write_doc(
            &fixture.repo,
            "docs/guides/intro.md",
            "# 入门 v2\n\nsecond beta body\n",
        );
        fs::remove_file(fixture.repo.join("docs/plain.md")).unwrap();

        // 文件删除后、重扫前，读取回退到扫描时缓存的正文。
        let plain_body = get_doc(conn, &plain.id).unwrap().body;
        assert_eq!(plain_body, "plain beta body\n");

        assert_eq!(scan_docs(conn, &fixture.project).unwrap(), 1);
        let after = list_docs(conn, &fixture.project.id).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, intro.id, "路径不变时文档 id 应稳定");
        assert_eq!(after[0].title, "入门 v2");
        assert_eq!(
            get_doc(conn, &intro.id).unwrap().body,
            "# 入门 v2\n\nsecond beta body\n"
        );
        assert!(get_doc(conn, &plain.id).is_err(), "源文件删除后应移除文档");
        assert!(
            search(conn, &fixture.project.id, "alpha")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search(conn, &fixture.project.id, "beta")
                .unwrap()
                .is_empty()
        );

        // 两字中文查询走 LIKE。
        let short = search(conn, &fixture.project.id, "入门").unwrap();
        assert_eq!(short.len(), 1);
        assert!(short[0].snippet.contains("[入门]"), "{}", short[0].snippet);
        assert!(search(conn, &fixture.project.id, "   ").unwrap().is_empty());
    }

    #[test]
    fn missing_repo_does_not_delete_indexed_docs() {
        let fixture = Fixture::new("missing");
        write_doc(&fixture.repo, "docs/a.md", "# A\nbody");
        assert_eq!(scan_docs(fixture.db.conn(), &fixture.project).unwrap(), 1);

        fs::rename(&fixture.repo, fixture.root.join("moved")).unwrap();
        assert!(scan_docs(fixture.db.conn(), &fixture.project).is_err());
        assert_eq!(
            list_docs(fixture.db.conn(), &fixture.project.id)
                .unwrap()
                .len(),
            1,
            "仓库不可读时不应删除已索引文档"
        );
    }
}
