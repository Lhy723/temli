//! M5 知识 / 文档工作区：笔记文件、文档索引与 FTS 全文检索。
//!
//! 权威边界（见 `docs/data-model.md`）：
//! - 笔记正文在 `.temli/notes/<uuid>.md`，文件是权威，`items.body` 是检索缓存；
//! - 文档由仓库 `docs/**/*.md` 扫描得到，DB 只存索引；
//! - 两者统一写入 `search_fts`，保存 / 扫描后立即可以检索。

use crate::db::Project;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const NOTES_DIR: &str = ".temli/notes";
const DOCS_DIR: &str = "docs";
const SEARCH_LIMIT: i64 = 50;

// ---------------------------------------------------------------------------
// 公开类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub file_path: String,
    pub created_at: String,
    pub updated_at: String,
}

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
// 笔记
// ---------------------------------------------------------------------------

/// 新建笔记：先写文件，再写 `items` 与 `search_fts`；DB 失败时回滚文件。
pub fn create_note(
    conn: &Connection,
    project: &Project,
    title: &str,
    body: &str,
) -> Result<Note, String> {
    let title = clean_title(title);
    if title.is_empty() {
        return Err("笔记标题不能为空".to_string());
    }
    let id = Uuid::new_v4().to_string();
    let file_path = note_file_path(&id);
    let absolute = project_path(&project.repo_path).join(&file_path);
    let now = now(conn)?;

    write_note_file(&absolute, &id, &title, &now, body)?;
    if let Err(err) = insert_note_row(conn, &project.id, &id, &title, body, &file_path, &now) {
        let _ = fs::remove_file(&absolute);
        return Err(err);
    }
    read_note(conn, &id)
}

pub fn list_notes(conn: &Connection, project_id: &str) -> Result<Vec<NoteMeta>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, updated_at, COALESCE(file_path, '')
             FROM items
             WHERE project_id = ?1 AND kind = 'note'
             ORDER BY updated_at DESC, title ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(NoteMeta {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
                file_path: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 读取笔记：文件是正文权威；文件缺失或无法解析时回退到 DB 缓存正文。
pub fn get_note(conn: &Connection, note_id: &str) -> Result<Note, String> {
    let (mut note, repo_path) = conn
        .query_row(
            "SELECT i.id, i.title, i.body, COALESCE(i.file_path, ''), i.created_at,
                    i.updated_at, p.repo_path
             FROM items i JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1 AND i.kind = 'note'",
            params![note_id],
            |row| {
                Ok((
                    Note {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        body: row.get(2)?,
                        file_path: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    },
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("笔记不存在：{note_id}"))?;

    if note.file_path.is_empty() {
        note.file_path = note_file_path(&note.id);
    }
    note.body = note_body_from_file(&repo_path, &note.file_path, note.body);
    Ok(note)
}

pub fn update_note(
    conn: &Connection,
    note_id: &str,
    title: &str,
    body: &str,
) -> Result<Note, String> {
    let title = clean_title(title);
    if title.is_empty() {
        return Err("笔记标题不能为空".to_string());
    }
    let (file_path, repo_path, project_id) = conn
        .query_row(
            "SELECT i.file_path, p.repo_path, i.project_id
             FROM items i JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1 AND i.kind = 'note'",
            params![note_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("笔记不存在：{note_id}"))?;

    let file_path = file_path
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| note_file_path(note_id));
    let updated_at = now(conn)?;
    let absolute = project_path(&repo_path).join(&file_path);
    write_note_file(&absolute, note_id, &title, &updated_at, body)?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let changed = tx
        .execute(
            "UPDATE items
             SET title = ?1, body = ?2, updated_at = ?3
             WHERE id = ?4 AND kind = 'note'",
            params![title, body, updated_at, note_id],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("笔记不存在：{note_id}"));
    }
    index_item(&tx, &project_id, note_id, "note", &title, body)?;
    tx.commit().map_err(db_err)?;
    read_note(conn, note_id)
}

/// 删除笔记：删除文件、`items` 行与 FTS 索引（`links` 由外键级联清理）。
pub fn delete_note(conn: &Connection, note_id: &str) -> Result<(), String> {
    let (file_path, repo_path) = conn
        .query_row(
            "SELECT i.file_path, p.repo_path
             FROM items i JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1 AND i.kind = 'note'",
            params![note_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("笔记不存在：{note_id}"))?;

    let file_path = file_path
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| note_file_path(note_id));
    let absolute = project_path(&repo_path).join(&file_path);
    match fs::remove_file(&absolute) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("删除笔记文件失败：{err}")),
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM search_fts WHERE entity_id = ?1",
        params![note_id],
    )
    .map_err(db_err)?;
    tx.execute(
        "DELETE FROM items WHERE id = ?1 AND kind = 'note'",
        params![note_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
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
    if let Ok(meta) = fs::symlink_metadata(&docs_root) {
        if meta.file_type().is_symlink() {
            return Err("docs 目录不能是符号链接".to_string());
        }
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
            title: doc_title(file, &body),
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

    if !doc.path.is_empty() {
        if let Ok(body) = fs::read_to_string(project_path(&repo_path).join(&doc.path)) {
            doc.body = body;
        }
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------
// 检索
// ---------------------------------------------------------------------------

/// 项目内 FTS5 检索；空查询返回空列表，正文片段用 `[...]` 标出命中词。
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
             WHERE search_fts MATCH ?1 AND project_id = ?2
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
             WHERE project_id = ?1 AND kind IN ('note', 'doc')
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

fn like_pattern(query: &str) -> String {
    format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

fn like_snippet(body: &str, needle: &str) -> String {
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

/// 把笔记 / 文档写入 `search_fts`；先删同 id 旧行，保证保存后立刻是唯一索引。
pub fn index_item(
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

fn now(conn: &Connection) -> Result<String, String> {
    conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
        row.get(0)
    })
    .map_err(db_err)
}

fn project_path(repo_path: &str) -> PathBuf {
    PathBuf::from(repo_path)
}

/// 标题写进单行 frontmatter：折叠换行与连续空白，避免破坏文件结构。
fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn note_file_path(note_id: &str) -> String {
    format!("{NOTES_DIR}/{note_id}.md")
}

fn note_file_content(id: &str, title: &str, updated_at: &str, body: &str) -> String {
    format!("---\nid: {id}\ntitle: {title}\nupdated_at: {updated_at}\n---\n{body}")
}

fn write_note_file(
    absolute: &Path,
    id: &str,
    title: &str,
    updated_at: &str,
    body: &str,
) -> Result<(), String> {
    if let Some(parent) = absolute.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建笔记目录失败：{err}"))?;
    }
    fs::write(absolute, note_file_content(id, title, updated_at, body))
        .map_err(|err| format!("写入笔记文件失败：{err}"))
}

/// 解析简单 frontmatter；没有合法 frontmatter 返回 `None`，正文保持原样。
fn parse_frontmatter(raw: &str) -> Option<(Vec<(&str, &str)>, &str)> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let first_end = raw.find('\n')?;
    if raw[..first_end].trim() != "---" {
        return None;
    }
    let mut fields = Vec::new();
    let mut pos = first_end + 1;
    while pos <= raw.len() {
        let line_end = raw[pos..]
            .find('\n')
            .map(|offset| pos + offset)
            .unwrap_or(raw.len());
        let line = &raw[pos..line_end];
        if line.trim() == "---" {
            let body_start = if line_end < raw.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some((fields, &raw[body_start..]));
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.push((key.trim(), value.trim()));
        }
        if line_end == raw.len() {
            break;
        }
        pos = line_end + 1;
    }
    None
}

fn note_body_from_file(repo_path: &str, file_path: &str, fallback: String) -> String {
    match fs::read_to_string(project_path(repo_path).join(file_path)) {
        Ok(raw) => match parse_frontmatter(&raw) {
            Some((_, body)) => body.to_string(),
            None => {
                let without_bom = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
                if without_bom.trim().is_empty() || without_bom.trim_start().starts_with("---") {
                    // 空文件或 frontmatter 损坏：回退 DB 缓存正文。
                    fallback
                } else {
                    // 外部编辑移除了 frontmatter；整个文件仍是用户最新正文。
                    raw
                }
            }
        },
        Err(_) => fallback,
    }
}

fn insert_note_row(
    conn: &Connection,
    project_id: &str,
    id: &str,
    title: &str,
    body: &str,
    file_path: &str,
    at: &str,
) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO items
             (id, project_id, kind, title, status, file_path, body, created_at, updated_at)
         VALUES (?1, ?2, 'note', ?3, 'open', ?4, ?5, ?6, ?6)",
        params![id, project_id, title, file_path, body, at],
    )
    .map_err(db_err)?;
    index_item(&tx, project_id, id, "note", title, body)?;
    tx.commit().map_err(db_err)
}

fn read_note(conn: &Connection, note_id: &str) -> Result<Note, String> {
    conn.query_row(
        "SELECT id, title, body, COALESCE(file_path, ''), created_at, updated_at
         FROM items
         WHERE id = ?1 AND kind = 'note'",
        params![note_id],
        |row| {
            Ok(Note {
                id: row.get(0)?,
                title: row.get(1)?,
                body: row.get(2)?,
                file_path: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("笔记不存在：{note_id}"))
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
                .map_or(false, |extension| extension.eq_ignore_ascii_case("md"));
            if is_markdown {
                out.push(entry.path());
            }
        }
    }
    Ok(())
}

/// FNV-1a：跨 Rust 版本稳定的 64 位哈希。
fn stable_hash(input: &str) -> u64 {
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

fn doc_title(path: &Path, body: &str) -> String {
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
                return heading.to_string();
            }
        }
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "untitled".to_string())
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
fn fts_match_query(query: &str) -> Option<String> {
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

        fn add_project(&self, label: &str) -> Project {
            let repo = self.root.join(format!("repo-{label}"));
            fs::create_dir_all(&repo).expect("创建第二个临时仓库");
            register(&self.db, &repo, label)
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

    fn note_path(fixture: &Fixture, note: &Note) -> PathBuf {
        fixture.repo.join(&note.file_path)
    }

    #[test]
    fn note_create_update_delete_roundtrip_and_search() {
        let fixture = Fixture::new("note");
        let conn = fixture.db.conn();

        let note = create_note(conn, &fixture.project, "记录", "alpha first").unwrap();
        let path = note_path(&fixture, &note);
        assert!(path.exists(), "笔记文件应写入 .temli/notes");
        assert_eq!(list_notes(conn, &fixture.project.id).unwrap().len(), 1);
        assert_eq!(get_note(conn, &note.id).unwrap().body, "alpha first");

        let raw = fs::read_to_string(&path).unwrap();
        let (fields, body) = parse_frontmatter(&raw).expect("笔记文件应带 frontmatter");
        assert_eq!(body, "alpha first");
        assert!(
            fields
                .iter()
                .any(|(key, value)| *key == "id" && *value == note.id.as_str())
        );
        assert!(fields.iter().any(|(key, _)| *key == "title"));
        assert!(fields.iter().any(|(key, _)| *key == "updated_at"));

        let updated = update_note(conn, &note.id, "记录 v2", "beta second").unwrap();
        assert_eq!(updated.title, "记录 v2");
        assert!(fs::read_to_string(&path).unwrap().contains("beta second"));
        assert!(
            search(conn, &fixture.project.id, "alpha")
                .unwrap()
                .is_empty(),
            "更新后旧正文不应再命中"
        );
        let hits = search(conn, &fixture.project.id, "beta").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, note.id);
        assert_eq!(hits[0].kind, "note");
        assert!(hits[0].snippet.contains("[beta]"), "{}", hits[0].snippet);

        delete_note(conn, &note.id).unwrap();
        assert!(!path.exists(), "删除笔记应删除文件");
        assert!(list_notes(conn, &fixture.project.id).unwrap().is_empty());
        assert!(get_note(conn, &note.id).is_err());
        assert!(
            search(conn, &fixture.project.id, "beta")
                .unwrap()
                .is_empty()
        );
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
    }

    #[test]
    fn search_is_project_scoped_with_body_snippet() {
        let fixture = Fixture::new("search");
        let other = fixture.add_project("other");
        let conn = fixture.db.conn();

        let mine = create_note(conn, &fixture.project, "One", "shared alpha body").unwrap();
        let theirs = create_note(conn, &other, "Two", "shared alpha body").unwrap();

        let hits = search(conn, &fixture.project.id, "alpha").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, mine.id);
        assert!(hits[0].snippet.contains("[alpha]"), "{}", hits[0].snippet);

        let other_hits = search(conn, &other.id, "alpha").unwrap();
        assert_eq!(other_hits.len(), 1);
        assert_eq!(other_hits[0].id, theirs.id);

        assert!(search(conn, &fixture.project.id, "   ").unwrap().is_empty());
        assert!(
            search(conn, &fixture.project.id, "missing-term")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn note_get_prefers_file_and_falls_back_to_db_body() {
        let fixture = Fixture::new("fallback");
        let conn = fixture.db.conn();
        let note = create_note(conn, &fixture.project, "标题", "数据库正文").unwrap();
        let path = note_path(&fixture, &note);

        fs::remove_file(&path).unwrap();
        assert_eq!(get_note(conn, &note.id).unwrap().body, "数据库正文");

        // 外部编辑移除 frontmatter：整个文件是最新正文。
        fs::write(&path, "手工编辑的正文").unwrap();
        assert_eq!(get_note(conn, &note.id).unwrap().body, "手工编辑的正文");

        // 空文件视为文件丢失，回退 DB 缓存正文。
        fs::write(&path, "").unwrap();
        assert_eq!(get_note(conn, &note.id).unwrap().body, "数据库正文");

        // 有 frontmatter 起始标记但缺少结束分隔符：回退 DB 缓存正文。
        fs::write(&path, "---\nid: broken").unwrap();
        assert_eq!(get_note(conn, &note.id).unwrap().body, "数据库正文");

        // 合法 frontmatter 时取分隔符之后的正文。
        fs::write(
            &path,
            format!(
                "---\nid: {}\ntitle: 标题\nupdated_at: 2026-01-01T00:00:00.000Z\n---\n文件正文",
                note.id
            ),
        )
        .unwrap();
        assert_eq!(get_note(conn, &note.id).unwrap().body, "文件正文");
        assert_eq!(
            list_notes(conn, &fixture.project.id).unwrap()[0].id,
            note.id
        );
    }
    #[test]
    fn search_supports_short_cjk_queries() {
        let fixture = Fixture::new("cjk");
        let conn = fixture.db.conn();
        create_note(conn, &fixture.project, "中文", "这是一段笔记正文").unwrap();

        let two = search(conn, &fixture.project.id, "笔记").unwrap();
        assert_eq!(two.len(), 1, "两字中文查询应命中");
        assert!(two[0].snippet.contains("[笔记]"), "{}", two[0].snippet);

        let three = search(conn, &fixture.project.id, "笔记正").unwrap();
        assert_eq!(three.len(), 1, "trigram 查询应命中");
        assert!(
            search(conn, &fixture.project.id, "不存在")
                .unwrap()
                .is_empty()
        );
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
