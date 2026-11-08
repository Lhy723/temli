//! 独立笔记库（vault）：Markdown 文件索引、双链、反向链接与图谱。
//!
//! 权威边界：
//! - 笔记正文在 vault 目录下的 `.md` 文件，文件是权威，`vault_notes.body` 是检索缓存；
//! - `[[目标]]` / `[[目标|别名]]` 写入 `vault_links`，按标题或路径 stem（大小写不敏感）解析；
//! - 笔记统一写入 `notes_fts`（trigram），少于 3 字符的查询走 LIKE。

use crate::workspace::markdown_title;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const MAX_NOTE_BYTES: u64 = 1024 * 1024;
const SEARCH_LIMIT: i64 = 50;
/// 体积大且几乎不含 Markdown 笔记的目录，直接跳过。
const SKIP_DIRS: &[&str] = &["node_modules"];

// ---------------------------------------------------------------------------
// 公开类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: String,
}

/// 树节点；`is_dir` 为真的中间目录没有 id / updated_at。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: String,
    pub path: String,
    pub title: String,
    pub is_dir: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultNote {
    pub id: String,
    pub vault_id: String,
    pub path: String,
    pub title: String,
    pub body: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultHit {
    pub id: String,
    pub vault_id: String,
    pub path: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backlink {
    pub id: String,
    pub path: String,
    pub title: String,
    /// 来源笔记中写的原始链接目标（未解析）。
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

// ---------------------------------------------------------------------------
// 笔记库
// ---------------------------------------------------------------------------

/// 注册笔记库：`create=true` 时先创建目录；目录必须存在且是真实目录（符号链接会被解析）。
pub fn add_vault(
    conn: &Connection,
    name: &str,
    root_path: &str,
    create: bool,
) -> Result<Vault, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("笔记库名称不能为空".to_string());
    }
    let raw = root_path.trim();
    if raw.is_empty() {
        return Err("笔记库目录不能为空".to_string());
    }
    let path = PathBuf::from(raw);
    if create {
        fs::create_dir_all(&path).map_err(|err| format!("创建笔记库目录失败：{err}"))?;
    }
    let canonical = path
        .canonicalize()
        .map_err(|err| format!("笔记库目录不存在或不可访问：{err}"))?;
    if !canonical.is_dir() {
        return Err(format!("笔记库路径不是目录：{}", canonical.display()));
    }
    let root = canonical.to_string_lossy().into_owned();
    if let Some(existing) = conn
        .query_row(
            "SELECT id FROM vaults WHERE root_path = ?1",
            params![root],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_err)?
    {
        return Err(format!("笔记库目录已注册：{existing}"));
    }

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO vaults (id, name, root_path) VALUES (?1, ?2, ?3)",
        params![id, name, root],
    )
    .map_err(db_err)?;
    get_vault(conn, &id)
}

pub fn get_vault(conn: &Connection, vault_id: &str) -> Result<Vault, String> {
    conn.query_row(
        "SELECT id, name, root_path, created_at FROM vaults WHERE id = ?1",
        params![vault_id],
        read_vault,
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("笔记库不存在：{vault_id}"))
}

pub fn list_vaults(conn: &Connection) -> Result<Vec<Vault>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, root_path, created_at FROM vaults ORDER BY created_at ASC, id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt.query_map([], read_vault).map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 注销笔记库：只删除 DB 索引，绝不删除磁盘文件。
pub fn remove_vault(conn: &Connection, vault_id: &str) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let changed = tx
        .execute("DELETE FROM vaults WHERE id = ?1", params![vault_id])
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("笔记库不存在：{vault_id}"));
    }
    tx.execute(
        "DELETE FROM notes_fts WHERE vault_id = ?1",
        params![vault_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 索引
// ---------------------------------------------------------------------------

struct ScannedNote {
    id: String,
    path: String,
    title: String,
    body: String,
}

/// 递归索引 vault 下的 Markdown；返回本次成功读取的笔记数。
///
/// 隐藏目录（`.obsidian`、`.git` 等）、`node_modules`、符号链接和超过 1 MiB 的
/// 文件不索引；此前已索引但本次被跳过的笔记保留旧行，避免临时读失败丢数据。
pub fn index_vault(conn: &Connection, vault: &Vault) -> Result<usize, String> {
    let root = PathBuf::from(&vault.root_path);
    let meta = fs::symlink_metadata(&root)
        .map_err(|err| format!("读取笔记库目录 {} 失败：{err}", root.display()))?;
    if !meta.is_dir() {
        return Err(format!("笔记库路径不是目录：{}", root.display()));
    }

    let mut files = Vec::new();
    collect_markdown_files(&root, &mut files)?;
    files.sort();

    let mut seen = HashSet::with_capacity(files.len());
    let mut scanned = Vec::with_capacity(files.len());
    for file in &files {
        let relative = relative_path(&root, file);
        let id = note_id(&vault.id, &relative);
        seen.insert(id.clone());

        match fs::metadata(file) {
            Ok(meta) if meta.len() > MAX_NOTE_BYTES => {
                eprintln!("笔记 {} 超过 1 MiB，跳过索引", file.display());
                continue;
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("读取笔记 {} 元数据失败，跳过：{err}", file.display());
                continue;
            }
        }
        let body = match fs::read_to_string(file) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("读取笔记 {} 失败，跳过：{err}", file.display());
                continue;
            }
        };
        scanned.push(ScannedNote {
            id,
            title: markdown_title(Path::new(&relative), &body),
            path: relative,
            body,
        });
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    // 先删掉源文件已消失的笔记与 FTS 行；链接由外键级联清理。
    let existing: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT id FROM vault_notes WHERE vault_id = ?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![vault.id], |row| row.get(0))
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };
    for id in existing {
        if seen.contains(&id) {
            continue;
        }
        tx.execute("DELETE FROM notes_fts WHERE note_id = ?1", params![id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM vault_notes WHERE id = ?1", params![id])
            .map_err(db_err)?;
    }

    let at = now(conn)?;
    for note in &scanned {
        upsert_note(
            &tx,
            &note.id,
            &vault.id,
            &note.path,
            &note.title,
            &note.body,
            &at,
        )?;
        index_note_fts(&tx, &note.id, &vault.id, &note.title, &note.body)?;
    }
    rebuild_links(&tx, &vault.id)?;
    tx.commit().map_err(db_err)?;
    Ok(scanned.len())
}

/// 扁平树：笔记路径 + 全部中间目录，前端据此组装树。
pub fn vault_tree(conn: &Connection, vault_id: &str) -> Result<Vec<VaultEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, title, updated_at
             FROM vault_notes
             WHERE vault_id = ?1
             ORDER BY path ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![vault_id], |row| {
            Ok(VaultEntry {
                id: row.get(0)?,
                path: row.get(1)?,
                title: row.get(2)?,
                is_dir: false,
                updated_at: row.get(3)?,
            })
        })
        .map_err(db_err)?;

    let mut entries: BTreeMap<String, VaultEntry> = BTreeMap::new();
    for row in rows {
        let entry = row.map_err(db_err)?;
        let segments: Vec<&str> = entry.path.split('/').collect();
        let mut prefix = String::new();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            entries.entry(prefix.clone()).or_insert_with(|| VaultEntry {
                id: String::new(),
                path: prefix.clone(),
                title: segment.to_string(),
                is_dir: true,
                updated_at: String::new(),
            });
        }
        entries.insert(entry.path.clone(), entry);
    }
    Ok(entries.into_values().collect())
}

// ---------------------------------------------------------------------------
// 笔记读写
// ---------------------------------------------------------------------------

/// 新建笔记：路径相对 vault 根，自动补 `.md` 并创建父目录；DB 失败时回滚文件。
pub fn create_vault_note(
    conn: &Connection,
    vault: &Vault,
    path: &str,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<VaultNote, String> {
    let relative = normalize_note_path(path)?;
    let title = title.map(clean_title).filter(|title| !title.is_empty());
    let content = match title.as_deref() {
        Some(title) => set_heading(body.unwrap_or(""), title),
        None => body.unwrap_or("").to_string(),
    };

    let absolute = Path::new(&vault.root_path).join(&relative);
    if absolute.exists() {
        return Err(format!("笔记文件已存在：{relative}"));
    }
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM vault_notes WHERE vault_id = ?1 AND path = ?2
             )",
            params![vault.id, relative],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists {
        return Err(format!("笔记已存在：{relative}"));
    }

    if let Some(parent) = absolute.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建笔记目录失败：{err}"))?;
        ensure_inside_vault(&vault.root_path, parent)?;
    }
    write_atomic(&absolute, &content)?;

    let id = note_id(&vault.id, &relative);
    let title = markdown_title(Path::new(&relative), &content);
    let at = now(conn)?;
    let written = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        upsert_note(&tx, &id, &vault.id, &relative, &title, &content, &at)?;
        index_note_fts(&tx, &id, &vault.id, &title, &content)?;
        rebuild_links(&tx, &vault.id)?;
        tx.commit().map_err(db_err)
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&absolute);
        return Err(err);
    }
    get_vault_note(conn, &id)
}

/// 保存笔记：可只改标题、正文或路径；路径变化时重命名文件并重算稳定 id。
pub fn save_vault_note(
    conn: &Connection,
    note_id_value: &str,
    title: Option<&str>,
    body: Option<&str>,
    path: Option<&str>,
) -> Result<VaultNote, String> {
    let (note, root) = load_note_and_root(conn, note_id_value)?;
    let target_path = match path {
        Some(path) => normalize_note_path(path)?,
        None => note.path.clone(),
    };
    let disk_path = Path::new(&root).join(&note.path);
    let disk_content = fs::read_to_string(&disk_path).ok();
    let title = title.map(clean_title).filter(|title| !title.is_empty());

    let content = match (title.as_deref(), body) {
        (Some(title), Some(body)) => set_heading(body, title),
        (Some(title), None) => set_heading(disk_content.as_deref().unwrap_or(&note.body), title),
        (None, Some(body)) => body.to_string(),
        (None, None) => disk_content.unwrap_or_else(|| note.body.clone()),
    };

    let moved = target_path != note.path;
    let target_absolute = Path::new(&root).join(&target_path);
    if moved {
        let duplicate: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM vault_notes
                     WHERE vault_id = ?1 AND path = ?2 AND id <> ?3
                 )",
                params![note.vault_id, target_path, note.id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if duplicate {
            return Err(format!("笔记路径已被占用：{target_path}"));
        }
        let same_file = target_absolute
            .canonicalize()
            .ok()
            .zip(disk_path.canonicalize().ok())
            .is_some_and(|(target, source)| target == source);
        if target_absolute.exists() && !same_file {
            return Err(format!("笔记文件已存在：{target_path}"));
        }
    }

    if let Some(parent) = target_absolute.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建笔记目录失败：{err}"))?;
        ensure_inside_vault(&root, parent)?;
    }
    write_atomic(&target_absolute, &content)?;
    if moved {
        let same_file = disk_path
            .canonicalize()
            .ok()
            .zip(target_absolute.canonicalize().ok())
            .is_some_and(|(source, target)| source == target);
        if !same_file && disk_path.exists() {
            let _ = fs::remove_file(&disk_path);
        }
    }

    let at = now(conn)?;
    let derived_title = markdown_title(Path::new(&target_path), &content);
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    if moved {
        let new_id = note_id(&note.vault_id, &target_path);
        // 删除旧行会让其它笔记指向它的链接变为未解析；随后整库重建链接恢复。
        tx.execute("DELETE FROM notes_fts WHERE note_id = ?1", params![note.id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM vault_notes WHERE id = ?1", params![note.id])
            .map_err(db_err)?;
        upsert_note(
            &tx,
            &new_id,
            &note.vault_id,
            &target_path,
            &derived_title,
            &content,
            &at,
        )?;
        index_note_fts(&tx, &new_id, &note.vault_id, &derived_title, &content)?;
        rebuild_links(&tx, &note.vault_id)?;
        tx.commit().map_err(db_err)?;
        return get_vault_note(conn, &new_id);
    }

    tx.execute(
        "UPDATE vault_notes
         SET title = ?1, body = ?2, updated_at = ?3
         WHERE id = ?4",
        params![derived_title, content, at, note.id],
    )
    .map_err(db_err)?;
    index_note_fts(&tx, &note.id, &note.vault_id, &derived_title, &content)?;
    rebuild_links(&tx, &note.vault_id)?;
    tx.commit().map_err(db_err)?;
    get_vault_note(conn, &note.id)
}

/// 删除笔记：删除文件、行、FTS 与链接；文件不存在视为已删除。
pub fn delete_vault_note(conn: &Connection, note_id_value: &str) -> Result<(), String> {
    let (note, root) = load_note_and_root(conn, note_id_value)?;
    let absolute = Path::new(&root).join(&note.path);
    match fs::remove_file(&absolute) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("删除笔记文件失败：{err}")),
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute("DELETE FROM notes_fts WHERE note_id = ?1", params![note.id])
        .map_err(db_err)?;
    tx.execute("DELETE FROM vault_notes WHERE id = ?1", params![note.id])
        .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

/// 读取笔记：文件是正文权威；文件缺失时回退到 DB 缓存正文。
pub fn get_vault_note(conn: &Connection, note_id_value: &str) -> Result<VaultNote, String> {
    let (mut note, root) = load_note_and_root(conn, note_id_value)?;
    let absolute = Path::new(&root).join(&note.path);
    if let Ok(body) = fs::read_to_string(absolute) {
        note.title = markdown_title(Path::new(&note.path), &body);
        note.body = body;
    }
    Ok(note)
}

// ---------------------------------------------------------------------------
// 检索 / 反向链接 / 图谱
// ---------------------------------------------------------------------------

/// vault 内检索：少于 3 字符走 LIKE（trigram 最少三字符），否则走 FTS5。
pub fn search_vault(
    conn: &Connection,
    vault_id: &str,
    query: &str,
) -> Result<Vec<VaultHit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query
        .split_whitespace()
        .any(|token| token.chars().count() < 3)
    {
        return search_like(conn, vault_id, query);
    }
    let Some(match_query) = crate::workspace::fts_match_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare(
            // notes_fts 列为 note_id, vault_id, title, body；片段取正文列 3。
            "SELECT notes_fts.note_id, n.vault_id, n.path, notes_fts.title,
                    snippet(notes_fts, 3, '[', ']', '…', 12)
             FROM notes_fts
             JOIN vault_notes n ON n.id = notes_fts.note_id
             WHERE notes_fts MATCH ?1 AND notes_fts.vault_id = ?2
             ORDER BY rank
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![match_query, vault_id, SEARCH_LIMIT], |row| {
            Ok(VaultHit {
                id: row.get(0)?,
                vault_id: row.get(1)?,
                path: row.get(2)?,
                title: row.get(3)?,
                snippet: row.get(4)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn search_like(conn: &Connection, vault_id: &str, query: &str) -> Result<Vec<VaultHit>, String> {
    let pattern = crate::workspace::like_pattern(query);
    let mut stmt = conn
        .prepare(
            "SELECT id, vault_id, path, title, body
             FROM vault_notes
             WHERE vault_id = ?1
               AND (title LIKE ?2 ESCAPE '\\' OR body LIKE ?2 ESCAPE '\\')
             ORDER BY updated_at DESC
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![vault_id, pattern, SEARCH_LIMIT], |row| {
            let body: String = row.get(4)?;
            Ok(VaultHit {
                id: row.get(0)?,
                vault_id: row.get(1)?,
                path: row.get(2)?,
                title: row.get(3)?,
                snippet: crate::workspace::like_snippet(&body, query),
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 反向链接：指向该笔记的来源笔记与原始目标文本。
pub fn note_backlinks(conn: &Connection, note_id_value: &str) -> Result<Vec<Backlink>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT n.id, n.path, n.title, l.target
             FROM vault_links l
             JOIN vault_notes n ON n.id = l.from_note
             WHERE l.resolved_note = ?1
             ORDER BY n.title ASC, n.id ASC, l.target ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![note_id_value], |row| {
            Ok(Backlink {
                id: row.get(0)?,
                path: row.get(1)?,
                title: row.get(2)?,
                target: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 图谱：全部笔记为节点，已解析的链接为有向边（同一对节点只保留一条边）。
pub fn vault_graph(conn: &Connection, vault_id: &str) -> Result<Graph, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, title
             FROM vault_notes
             WHERE vault_id = ?1
             ORDER BY title ASC, path ASC",
        )
        .map_err(db_err)?;
    let nodes = stmt
        .query_map(params![vault_id], |row| {
            Ok(GraphNode {
                id: row.get(0)?,
                path: row.get(1)?,
                title: row.get(2)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT l.from_note, l.resolved_note
             FROM vault_links l
             JOIN vault_notes n ON n.id = l.resolved_note
             WHERE l.vault_id = ?1 AND n.vault_id = ?1
             ORDER BY l.from_note ASC, l.resolved_note ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![vault_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_err)?;

    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for row in rows {
        let (from, to) = row.map_err(db_err)?;
        if seen.insert((from.clone(), to.clone())) {
            edges.push(GraphEdge { from, to });
        }
    }
    Ok(Graph { nodes, edges })
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

fn read_vault(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vault> {
    Ok(Vault {
        id: row.get(0)?,
        name: row.get(1)?,
        root_path: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn load_note_and_root(
    conn: &Connection,
    note_id_value: &str,
) -> Result<(VaultNote, String), String> {
    conn.query_row(
        "SELECT n.id, n.vault_id, n.path, n.title, n.body, n.updated_at, v.root_path
         FROM vault_notes n JOIN vaults v ON v.id = n.vault_id
         WHERE n.id = ?1",
        params![note_id_value],
        |row| {
            Ok((
                VaultNote {
                    id: row.get(0)?,
                    vault_id: row.get(1)?,
                    path: row.get(2)?,
                    title: row.get(3)?,
                    body: row.get(4)?,
                    updated_at: row.get(5)?,
                },
                row.get::<_, String>(6)?,
            ))
        },
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("笔记不存在：{note_id_value}"))
}

fn upsert_note(
    conn: &Connection,
    id: &str,
    vault_id: &str,
    path: &str,
    title: &str,
    body: &str,
    at: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO vault_notes (id, vault_id, path, title, body, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
             vault_id = excluded.vault_id,
             path = excluded.path,
             title = excluded.title,
             body = excluded.body,
             updated_at = excluded.updated_at",
        params![id, vault_id, path, title, body, at],
    )
    .map_err(db_err)?;
    Ok(())
}

fn index_note_fts(
    conn: &Connection,
    note_id_value: &str,
    vault_id: &str,
    title: &str,
    body: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM notes_fts WHERE note_id = ?1",
        params![note_id_value],
    )
    .map_err(db_err)?;
    conn.execute(
        "INSERT INTO notes_fts (note_id, vault_id, title, body) VALUES (?1, ?2, ?3, ?4)",
        params![note_id_value, vault_id, title, body],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 从 DB 里每个笔记的正文重建整库链接：标题 / 路径变化后解析结果自然更新。
fn rebuild_links(conn: &Connection, vault_id: &str) -> Result<(), String> {
    let notes: Vec<(String, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, path, title, body FROM vault_notes
                 WHERE vault_id = ?1 ORDER BY path ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![vault_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };

    let mut resolver: HashMap<String, String> = HashMap::new();
    for (id, path, title, _) in &notes {
        for key in note_keys(title, path) {
            resolver.entry(key).or_insert_with(|| id.clone());
        }
    }

    conn.execute(
        "DELETE FROM vault_links WHERE vault_id = ?1",
        params![vault_id],
    )
    .map_err(db_err)?;
    for (from, _, _, body) in &notes {
        // 按去重后的目标写行（保留首次出现的大小写形式）。
        let mut unique: BTreeMap<String, String> = BTreeMap::new();
        for target in parse_wikilinks(body) {
            unique
                .entry(target.to_lowercase())
                .or_insert_with(|| target.clone());
        }
        for (key, target) in unique {
            let resolved = resolver.get(&key).map(String::as_str);
            let link_id = format!(
                "link-{:016x}",
                crate::workspace::stable_hash(&format!("{from}:{target}"))
            );
            conn.execute(
                "INSERT INTO vault_links (id, vault_id, from_note, target, resolved_note)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![link_id, vault_id, from, target, resolved],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}

fn note_keys(title: &str, path: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(3);
    keys.push(title.to_lowercase());
    let stem = Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !stem.is_empty() {
        keys.push(stem.to_lowercase());
    }
    let without_extension = path
        .get(..path.len().saturating_sub(3))
        .filter(|_prefix| path.to_lowercase().ends_with(".md"))
        .unwrap_or(path);
    if !without_extension.is_empty() {
        keys.push(without_extension.to_lowercase());
    }
    keys
}

/// `[[目标]]` / `[[目标|别名]]` / `[[目标#标题]]`：取目标部分并去掉两侧空白。
fn parse_wikilinks(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = &after[..end];
        let target = inner.split_once('|').map_or(inner, |(target, _)| target);
        let target = target.split_once('#').map_or(target, |(target, _)| target);
        let target = target.trim();
        if !target.is_empty() {
            links.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    links
}

fn note_id(vault_id: &str, relative: &str) -> String {
    format!(
        "note-{:016x}",
        crate::workspace::stable_hash(&format!("{vault_id}:{relative}"))
    )
}

fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 校验并规范化相对路径：统一 `/`，拒绝绝对路径与 `..`，自动补 `.md`。
fn normalize_note_path(input: &str) -> Result<String, String> {
    let normalized = input.trim().replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    if normalized.is_empty() {
        return Err("笔记路径不能为空".to_string());
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("笔记路径不能包含 ..".to_string()),
            other => {
                if other.contains('\0') {
                    return Err("笔记路径包含非法字符".to_string());
                }
                parts.push(other);
            }
        }
    }
    if parts.is_empty() {
        return Err("笔记路径不能为空".to_string());
    }
    let mut path = parts.join("/");
    if !path.to_lowercase().ends_with(".md") {
        path.push_str(".md");
    }
    Ok(path)
}

/// 确认目标目录（规范化后）仍在笔记库根目录内，防止通过符号链接写出库外。
fn ensure_inside_vault(root: &str, directory: &Path) -> Result<(), String> {
    let real_root = Path::new(root)
        .canonicalize()
        .map_err(|err| format!("读取笔记库目录失败：{err}"))?;
    let real_directory = directory
        .canonicalize()
        .map_err(|err| format!("读取笔记目录失败：{err}"))?;
    if !real_directory.starts_with(&real_root) {
        return Err(format!("笔记路径超出笔记库目录：{}", directory.display()));
    }
    Ok(())
}

/// 原子写：先写同目录临时文件，再 rename 覆盖目标。
fn write_atomic(target: &Path, content: &str) -> Result<(), String> {
    let temp = target.with_extension("md.temli-tmp");
    fs::write(&temp, content).map_err(|err| format!("写入笔记文件失败：{err}"))?;
    if let Err(err) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("替换笔记文件失败：{err}"));
    }
    Ok(())
}

/// 把 `title` 写成正文里的首个 H1：已有 H1 时替换，否则插到 frontmatter 之后。
fn set_heading(content: &str, title: &str) -> String {
    let heading = format!("# {}", clean_title(title));
    let bom = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let (bom_prefix, scan) = content.split_at(bom);
    let mut out = String::with_capacity(content.len() + heading.len() + 4);
    out.push_str(bom_prefix);
    let mut in_frontmatter = false;
    let mut in_fence = false;
    let mut first = true;
    let mut replaced = false;

    for line in scan.split_inclusive('\n') {
        let body = line.trim_end_matches(|character| character == '\r' || character == '\n');
        let trimmed = body.trim();
        if first {
            first = false;
            if trimmed == "---" {
                in_frontmatter = true;
                out.push_str(line);
                continue;
            }
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            out.push_str(line);
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if !in_fence && !replaced {
            if let Some(text) = body.trim_start().strip_prefix("# ") {
                if !text.trim().is_empty() {
                    let indent = &body[..body.len() - body.trim_start().len()];
                    out.push_str(indent);
                    out.push_str(&heading);
                    out.push_str(&line[body.len()..]);
                    replaced = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }

    if replaced {
        return out;
    }
    let preamble = preamble_len(content);
    let (head, tail) = content.split_at(preamble);
    let mut out = String::with_capacity(content.len() + heading.len() + 4);
    out.push_str(head);
    if preamble > bom && !head.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&heading);
    out.push('\n');
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    out
}

/// BOM + 合法 frontmatter 的字节长度；没有 frontmatter 时只返回 BOM 长度。
fn preamble_len(content: &str) -> usize {
    let bom = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let rest = &content[bom..];
    if !rest.starts_with("---") {
        return bom;
    }
    let mut offset = bom;
    let mut lines = rest.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first
        .trim_end_matches(|character| character == '\r' || character == '\n')
        .trim()
        != "---"
    {
        return bom;
    }
    offset += first.len();
    for line in lines {
        offset += line.len();
        if line
            .trim_end_matches(|character| character == '\r' || character == '\n')
            .trim()
            == "---"
        {
            return offset;
        }
    }
    bom
}

/// 递归收集 `*.md`；跳过隐藏目录、`node_modules` 与符号链接。
fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("读取笔记目录 {} 失败：{err}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取笔记目录失败：{err}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|err| format!("读取笔记类型失败：{err}"))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            collect_markdown_files(&entry.path(), out)?;
        } else if file_type.is_file() && is_markdown(&entry.path()) {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
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
                "temli-vaults-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("创建临时目录");
            let db = Db::open(&root.join("temli.db")).expect("打开临时数据库");
            Self { root, db }
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

        fn write(root: &str, relative: &str, contents: &str) -> PathBuf {
            let path = Path::new(root).join(relative);
            fs::create_dir_all(path.parent().unwrap()).expect("创建笔记目录");
            fs::write(&path, contents).expect("写入笔记");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn title_of(conn: &Connection, vault_id: &str, title: &str) -> VaultNote {
        let tree = vault_tree(conn, vault_id).expect("读取树");
        let entry = tree
            .iter()
            .find(|entry| !entry.is_dir && entry.title == title)
            .unwrap_or_else(|| panic!("找不到标题为 {title} 的笔记"));
        get_vault_note(conn, &entry.id).expect("读取笔记")
    }

    #[test]
    fn vault_add_list_remove_keeps_files() {
        let fixture = Fixture::new("vault");
        let conn = fixture.db.conn();
        let dir = fixture.root.join("notes");

        let vault = add_vault(conn, " 我的笔记 ", dir.to_str().unwrap(), true).unwrap();
        assert_eq!(vault.name, "我的笔记");
        assert!(dir.is_dir(), "create=true 应创建目录");
        let canonical = dir.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(vault.root_path, canonical);
        assert_eq!(list_vaults(conn).unwrap().len(), 1);

        let duplicate = add_vault(conn, "重复", dir.to_str().unwrap(), false).unwrap_err();
        assert!(duplicate.contains("已注册"), "{duplicate}");

        let missing = fixture.root.join("missing");
        assert!(
            add_vault(conn, "缺失", missing.to_str().unwrap(), false).is_err(),
            "目录不存在且 create=false 时应失败"
        );

        Fixture::write(&vault.root_path, "a.md", "# A");
        remove_vault(conn, &vault.id).unwrap();
        assert!(
            Path::new(&vault.root_path).join("a.md").exists(),
            "注销不能删除文件"
        );
        assert!(list_vaults(conn).unwrap().is_empty());
        assert!(remove_vault(conn, &vault.id).is_err());
    }

    #[test]
    fn index_nested_folders_and_wikilinks() {
        let fixture = Fixture::new("index");
        let vault = fixture.vault("brain");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        Fixture::write(root, "Alpha.md", "# Alpha\n\n根笔记\n");
        Fixture::write(
            root,
            "folder/Beta.md",
            "# Beta\n\n[[alpha]] 与 [[Gamma|g]] 与 [[Deep]]\n",
        );
        Fixture::write(root, "folder/sub/Gamma.md", "# Gamma\n\n![[Beta]]\n");
        Fixture::write(root, "folder/sub/Deep.md", "# Deep title\n\n正文\n");
        Fixture::write(root, ".obsidian/hidden.md", "# Hidden\n");
        Fixture::write(root, "node_modules/pkg/readme.md", "# Dep\n");

        assert_eq!(index_vault(conn, &vault).unwrap(), 4);

        let tree = vault_tree(conn, &vault.id).unwrap();
        let paths: Vec<&str> = tree.iter().map(|entry| entry.path.as_str()).collect();
        assert!(paths.contains(&"folder"), "{paths:?}");
        assert!(paths.contains(&"folder/sub"), "{paths:?}");
        assert!(!paths.iter().any(|path| path.starts_with('.')), "{paths:?}");
        assert!(
            !paths.iter().any(|path| path.contains("node_modules")),
            "{paths:?}"
        );
        assert_eq!(tree.iter().filter(|entry| entry.is_dir).count(), 2);
        let alpha = tree
            .iter()
            .find(|entry| entry.path == "Alpha.md")
            .expect("索引 Alpha");
        assert_eq!(alpha.title, "Alpha");
        assert_eq!(
            get_vault_note(conn, &alpha.id).unwrap().body,
            "# Alpha\n\n根笔记\n"
        );

        // [[alpha]] 大小写不敏感；[[Gamma]] 标题命中；[[Deep]] 路径 stem 命中。
        let backlinks = note_backlinks(conn, &alpha.id).unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].path, "folder/Beta.md");
        assert_eq!(backlinks[0].target, "alpha");

        let gamma = title_of(conn, &vault.id, "Gamma");
        assert_eq!(note_backlinks(conn, &gamma.id).unwrap()[0].target, "Gamma");
        let deep = title_of(conn, &vault.id, "Deep title");
        assert_eq!(note_backlinks(conn, &deep.id).unwrap()[0].target, "Deep");
        let beta = title_of(conn, &vault.id, "Beta");
        assert_eq!(note_backlinks(conn, &beta.id).unwrap()[0].target, "Beta");

        let graph = vault_graph(conn, &vault.id).unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.edges.len(), 4);
        let has_edge = |from: &str, to: &str| {
            graph
                .edges
                .iter()
                .any(|edge| edge.from == from && edge.to == to)
        };
        assert!(has_edge(&beta.id, &alpha.id));
        assert!(has_edge(&beta.id, &gamma.id));
        assert!(has_edge(&beta.id, &deep.id));
        assert!(has_edge(&gamma.id, &beta.id));

        fs::remove_file(Path::new(root).join("folder/sub/Gamma.md")).unwrap();
        assert_eq!(index_vault(conn, &vault).unwrap(), 3);
        let graph = vault_graph(conn, &vault.id).unwrap();
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges.len(), 2, "已删除笔记的边应消失");
        assert!(note_backlinks(conn, &beta.id).unwrap().is_empty());
        let tree = vault_tree(conn, &vault.id).unwrap();
        assert!(!tree.iter().any(|entry| entry.path.contains("Gamma")));
    }

    #[test]
    fn note_create_save_delete_roundtrip() {
        let fixture = Fixture::new("notes");
        let vault = fixture.vault("notes");
        let conn = fixture.db.conn();

        let first =
            create_vault_note(conn, &vault, "ideas/first", Some("想法"), Some("正文")).unwrap();
        assert_eq!(first.path, "ideas/first.md");
        assert_eq!(first.title, "想法");
        let file = Path::new(&vault.root_path).join("ideas/first.md");
        assert!(file.exists());
        assert!(fs::read_to_string(&file).unwrap().contains("# 想法"));
        assert!(create_vault_note(conn, &vault, "ideas/first.md", None, None).is_err());

        let second = create_vault_note(
            conn,
            &vault,
            "second",
            None,
            Some("# 第二\n\n原文 [[想法]]"),
        )
        .unwrap();
        assert_eq!(second.title, "第二");

        let saved = save_vault_note(
            conn,
            &second.id,
            Some("第二篇"),
            Some("新正文，仍链接 [[想法]]"),
            None,
        )
        .unwrap();
        assert_eq!(saved.title, "第二篇");
        assert_eq!(note_backlinks(conn, &first.id).unwrap().len(), 1);
        assert!(
            search_vault(conn, &vault.id, "新正文")
                .unwrap()
                .iter()
                .any(|hit| hit.id == second.id)
        );
        assert!(
            search_vault(conn, &vault.id, "原文")
                .unwrap()
                .iter()
                .all(|hit| hit.id != second.id),
            "保存后旧正文不应再命中"
        );

        let moved = save_vault_note(conn, &second.id, None, None, Some("archive/second")).unwrap();
        assert_eq!(moved.path, "archive/second.md");
        assert_ne!(moved.id, second.id, "路径变化后稳定 id 重算");
        assert!(!Path::new(&vault.root_path).join("second.md").exists());
        assert!(
            Path::new(&vault.root_path)
                .join("archive/second.md")
                .exists()
        );
        assert_eq!(note_backlinks(conn, &first.id).unwrap().len(), 1);

        // 外部编辑：文件是正文权威。
        fs::write(
            Path::new(&vault.root_path).join(&moved.path),
            "# 外部\n\n外部正文",
        )
        .unwrap();
        let reread = get_vault_note(conn, &moved.id).unwrap();
        assert_eq!(reread.title, "外部");
        assert!(reread.body.contains("外部正文"));

        // 文件被外部删除：回退到最近一次保存的 DB 缓存正文。
        fs::remove_file(Path::new(&vault.root_path).join(&moved.path)).unwrap();
        let cached = get_vault_note(conn, &moved.id).unwrap();
        assert_eq!(cached.title, "第二篇");
        assert!(cached.body.contains("新正文"));

        delete_vault_note(conn, &moved.id).unwrap();
        assert!(!Path::new(&vault.root_path).join(&moved.path).exists());
        assert!(get_vault_note(conn, &moved.id).is_err());
        assert!(note_backlinks(conn, &first.id).unwrap().is_empty());
        assert!(search_vault(conn, &vault.id, "新正文").unwrap().is_empty());
    }

    #[test]
    fn search_supports_two_and_three_char_cjk() {
        let fixture = Fixture::new("cjk");
        let vault = fixture.vault("cjk");
        let other = fixture.vault("other");
        let conn = fixture.db.conn();

        let note = create_vault_note(conn, &vault, "中文", Some("中文"), Some("这是一段笔记正文"))
            .unwrap();
        create_vault_note(conn, &other, "中文", Some("中文"), Some("这是一段笔记正文")).unwrap();

        let two = search_vault(conn, &vault.id, "笔记").unwrap();
        assert_eq!(two.len(), 1, "两字中文查询应命中");
        assert_eq!(two[0].id, note.id);
        assert_eq!(two[0].path, "中文.md");
        assert!(two[0].snippet.contains("[笔记]"), "{}", two[0].snippet);

        let three = search_vault(conn, &vault.id, "笔记正").unwrap();
        assert_eq!(three.len(), 1, "trigram 查询应命中");
        assert!(three[0].snippet.contains("笔记正"), "{}", three[0].snippet);

        assert!(search_vault(conn, &vault.id, "不存在").unwrap().is_empty());
        assert!(search_vault(conn, &vault.id, "   ").unwrap().is_empty());
        assert_ne!(vault.id, other.id);
        assert_eq!(search_vault(conn, &other.id, "笔记").unwrap().len(), 1);
    }
}
