//! 独立笔记库（vault）：Markdown 文件索引、双链、反向链接与图谱。
//!
//! 权威边界：
//! - 笔记正文在 vault 目录下的 `.md` 文件，文件是权威，`vault_notes.body` 是检索缓存；
//! - `[[目标]]` / `[[目标|别名]]` 写入 `vault_links`，按标题、路径 stem 或 frontmatter `aliases`
//!   （大小写不敏感）解析；
//! - 笔记统一写入 `notes_fts`（trigram），少于 3 字符的查询走 LIKE。

use crate::workspace::markdown_title;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const MAX_NOTE_BYTES: u64 = 1024 * 1024;
/// 附件上限：25 MiB（解码后的字节数）。
const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
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

/// 标签及其笔记数（按数量降序）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultTag {
    pub tag: String,
    pub count: i64,
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
        rebuild_note_tags(&tx, &note.id, &note.body)?;
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
    let title = title.map(clean_title).filter(|title| !title.is_empty());
    let content = match title.as_deref() {
        Some(title) => set_heading(body.unwrap_or(""), title),
        None => body.unwrap_or("").to_string(),
    };
    create_vault_note_with_content(conn, vault, path, &content)
}

/// 用现成正文新建笔记（模板落地也走这里）：不注入标题，正文原样写入。
pub fn create_vault_note_with_content(
    conn: &Connection,
    vault: &Vault,
    path: &str,
    content: &str,
) -> Result<VaultNote, String> {
    let relative = normalize_note_path(path)?;

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
    write_atomic(&absolute, content)?;

    let id = note_id(&vault.id, &relative);
    let title = markdown_title(Path::new(&relative), content);
    let at = now(conn)?;
    let written = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        upsert_note(&tx, &id, &vault.id, &relative, &title, content, &at)?;
        index_note_fts(&tx, &id, &vault.id, &title, content)?;
        rebuild_note_tags(&tx, &id, content)?;
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
        rebuild_note_tags(&tx, &new_id, &content)?;
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
    rebuild_note_tags(&tx, &note.id, &content)?;
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
// 标签
// ---------------------------------------------------------------------------

/// 标签列表：按笔记数降序，同数量按标签名升序。
pub fn vault_tags(conn: &Connection, vault_id: &str) -> Result<Vec<VaultTag>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.tag, COUNT(*) AS note_count
             FROM vault_note_tags t
             JOIN vault_notes n ON n.id = t.note_id
             WHERE n.vault_id = ?1
             GROUP BY t.tag
             ORDER BY note_count DESC, t.tag ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![vault_id], |row| {
            Ok(VaultTag {
                tag: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 标签下的笔记：按标题升序（大小写不敏感）。
pub fn notes_by_tag(
    conn: &Connection,
    vault_id: &str,
    tag: &str,
) -> Result<Vec<VaultEntry>, String> {
    let tag = normalize_tag(tag).ok_or_else(|| format!("标签无效：{tag}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT n.id, n.path, n.title, n.updated_at
             FROM vault_notes n
             JOIN vault_note_tags t ON t.note_id = n.id
             WHERE n.vault_id = ?1 AND t.tag = ?2
             ORDER BY n.title COLLATE NOCASE ASC, n.path ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![vault_id, tag], |row| {
            Ok(VaultEntry {
                id: row.get(0)?,
                path: row.get(1)?,
                title: row.get(2)?,
                is_dir: false,
                updated_at: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 附件
// ---------------------------------------------------------------------------

/// 保存附件：base64 解码后写入 `attachments/<笔记名>-<uuid8>.<ext>`，返回相对路径。
pub fn save_vault_attachment(
    conn: &Connection,
    vault_id: &str,
    note_id_value: &str,
    file_name: &str,
    data_base64: &str,
) -> Result<String, String> {
    let (note, root) = load_note_and_root(conn, note_id_value)?;
    if note.vault_id != vault_id {
        return Err("笔记不属于该笔记库".to_string());
    }
    if data_base64.trim().is_empty() {
        return Err("附件内容为空".to_string());
    }
    let bytes = base64_decode(data_base64)?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err("附件超过 25 MB".to_string());
    }

    let extension = attachment_extension(file_name);
    let note_stem = Path::new(&note.path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let note_stem = {
        let cleaned = sanitize_name_part(&note_stem, 40);
        if cleaned.is_empty() {
            "note".to_string()
        } else {
            cleaned
        }
    };

    let directory = Path::new(&root).join("attachments");
    fs::create_dir_all(&directory).map_err(|err| format!("创建附件目录失败：{err}"))?;
    ensure_inside_vault(&root, &directory)?;

    let mut chosen = String::new();
    for _ in 0..8 {
        let unique = Uuid::new_v4().simple().to_string();
        let candidate = format!("attachments/{note_stem}-{}.{extension}", &unique[..8]);
        if !Path::new(&root).join(&candidate).exists() {
            chosen = candidate;
            break;
        }
    }
    if chosen.is_empty() {
        return Err("生成附件文件名失败，请重试".to_string());
    }

    let target = Path::new(&root).join(&chosen);
    let temp = target.with_extension("temli-attachment-tmp");
    fs::write(&temp, &bytes).map_err(|err| format!("写入附件失败：{err}"))?;
    if let Err(err) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("保存附件失败：{err}"));
    }
    Ok(chosen)
}

/// 读取附件：只允许笔记库内 `attachments/` 下的路径，base64 返回（≤25 MB）。
pub fn read_vault_attachment(
    conn: &Connection,
    vault_id: &str,
    relative_path: &str,
) -> Result<String, String> {
    let vault = get_vault(conn, vault_id)?;
    let root = Path::new(&vault.root_path);
    let relative = normalize_attachment_path(relative_path)?;
    let attachments = root.join("attachments");
    let real_directory = attachments
        .canonicalize()
        .map_err(|err| format!("附件目录不存在：{err}"))?;
    let target = root.join(&relative);
    let real_target = target
        .canonicalize()
        .map_err(|err| format!("附件不存在：{err}"))?;
    if !real_target.starts_with(&real_directory) {
        return Err("附件路径超出 attachments 目录".to_string());
    }
    let meta = fs::metadata(&real_target).map_err(|err| format!("读取附件失败：{err}"))?;
    if !meta.is_file() {
        return Err("附件路径不是文件".to_string());
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err("附件超过 25 MB".to_string());
    }
    let bytes = fs::read(&real_target).map_err(|err| format!("读取附件失败：{err}"))?;
    Ok(base64_encode(&bytes))
}

// ---------------------------------------------------------------------------
// 模板 / 属性 / 重命名
// ---------------------------------------------------------------------------

/// 列出 `<vault>/templates/*.md`（只扫描一级目录），按标题升序。
pub fn list_vault_templates(conn: &Connection, vault_id: &str) -> Result<Vec<VaultEntry>, String> {
    let vault = get_vault(conn, vault_id)?;
    let root = Path::new(&vault.root_path);
    let directory = root.join("templates");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("读取模板目录失败：{err}")),
    };

    let mut templates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取模板目录失败：{err}"))?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("读取模板类型失败：{err}"))?;
        if !file_type.is_file() || !is_markdown(&entry.path()) {
            continue;
        }
        let path = relative_path(root, &entry.path());
        let body = match fs::read_to_string(entry.path()) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("读取模板 {} 失败，跳过：{err}", entry.path().display());
                continue;
            }
        };
        let indexed: Option<(String, String)> = conn
            .query_row(
                "SELECT id, updated_at FROM vault_notes WHERE vault_id = ?1 AND path = ?2",
                params![vault_id, path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_err)?;
        let (id, updated_at) = indexed.unwrap_or_else(|| (note_id(vault_id, &path), String::new()));
        templates.push(VaultEntry {
            id,
            title: markdown_title(Path::new(&path), &body),
            path,
            is_dir: false,
            updated_at,
        });
    }
    templates.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(templates)
}

/// 设置 frontmatter 属性：保留其它键与正文，空值删除键；`tags` 会触发标签重建。
pub fn set_note_properties(
    conn: &Connection,
    note_id_value: &str,
    properties: &BTreeMap<String, String>,
) -> Result<VaultNote, String> {
    for key in properties.keys() {
        if !is_property_key(key) {
            return Err(format!("属性名不合法：{key}"));
        }
    }
    let (note, root) = load_note_and_root(conn, note_id_value)?;
    let absolute = Path::new(&root).join(&note.path);
    let content = fs::read_to_string(&absolute).unwrap_or_else(|_| note.body.clone());
    let updated = set_frontmatter_properties(&content, properties);
    if updated == content {
        return get_vault_note(conn, note_id_value);
    }
    save_vault_note(conn, note_id_value, None, Some(updated.as_str()), None)
}

/// 重命名笔记（按新路径重算 id）：移动文件、迁移索引并改写全库指向旧标题/文件名的 wikilink。
pub fn rename_vault_note(
    conn: &Connection,
    note_id_value: &str,
    new_path: &str,
) -> Result<VaultNote, String> {
    let (note, root) = load_note_and_root(conn, note_id_value)?;
    let target_path = normalize_note_path(new_path)?;
    if target_path == note.path {
        return get_vault_note(conn, note_id_value);
    }

    let disk_path = Path::new(&root).join(&note.path);
    let target_absolute = Path::new(&root).join(&target_path);
    let content = fs::read_to_string(&disk_path).unwrap_or_else(|_| note.body.clone());

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

    let at = now(conn)?;
    if let Some(parent) = target_absolute.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建笔记目录失败：{err}"))?;
        ensure_inside_vault(&root, parent)?;
    }
    write_atomic(&target_absolute, &content)?;
    if !same_file && disk_path.exists() {
        let _ = fs::remove_file(&disk_path);
    }

    let old_title = markdown_title(Path::new(&note.path), &content);
    let new_title = markdown_title(Path::new(&target_path), &content);
    let new_id = note_id(&note.vault_id, &target_path);
    let written = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        // id 由 vault 内相对路径决定，重命名后必须迁移到新 id，旧行与旧 FTS 一并删除。
        tx.execute("DELETE FROM notes_fts WHERE note_id = ?1", params![note.id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM vault_notes WHERE id = ?1", params![note.id])
            .map_err(db_err)?;
        upsert_note(
            &tx,
            &new_id,
            &note.vault_id,
            &target_path,
            &new_title,
            &content,
            &at,
        )?;
        index_note_fts(&tx, &new_id, &note.vault_id, &new_title, &content)?;
        rebuild_note_tags(&tx, &new_id, &content)?;
        rebuild_links(&tx, &note.vault_id)?;
        tx.commit().map_err(db_err)
    })();
    if let Err(err) = written {
        let _ = fs::write(&disk_path, &content);
        if !same_file {
            let _ = fs::remove_file(&target_absolute);
        }
        return Err(err);
    }

    let replacements = rename_replacements(&note.path, &target_path, &old_title, &new_title);
    if !replacements.is_empty() {
        rewrite_vault_links(conn, &note.vault_id, &replacements)?;
    }
    get_vault_note(conn, &new_id)
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

/// 从 DB 里每个笔记的正文重建整库链接：标题 / 路径 / frontmatter 别名变化后解析结果自然更新。
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
    for (id, path, title, body) in &notes {
        for key in note_keys(title, path, body) {
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

/// 笔记的解析键：标题、路径 stem、无扩展名路径，最后是 frontmatter 别名；
/// 调用方按先到先得注册，越靠前的键优先级越高。
fn note_keys(title: &str, path: &str, body: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(8);
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
    // frontmatter 别名作为额外键，已有标题 / 路径键保持优先。
    keys.extend(parse_aliases(body));
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

// ---------------------------------------------------------------------------
// frontmatter / 标签
// ---------------------------------------------------------------------------

struct FrontmatterParts<'a> {
    /// 去掉分隔行的 frontmatter 内部文本。
    block: &'a str,
    /// 关闭分隔行之后的正文。
    body: &'a str,
    /// 原文件使用的换行符。
    newline: &'static str,
}

/// 拆分 BOM + frontmatter；没有合法 frontmatter 时返回 `None`。
fn split_frontmatter(content: &str) -> Option<FrontmatterParts<'_>> {
    let bom_len = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let rest = &content[bom_len..];
    let mut lines = rest.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return None;
    }
    let newline = if first.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let block_start = bom_len + first.len();
    let mut offset = block_start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            let preamble = offset + line.len();
            return Some(FrontmatterParts {
                block: &content[block_start..offset],
                body: &content[preamble..],
                newline,
            });
        }
        offset += line.len();
    }
    None
}

/// frontmatter 里的别名：`aliases` / `alias`，支持 flow `[a, b]`、逗号分隔与块列表。
///
/// 只扫描 frontmatter 块，正文里的 `aliases:` 字样不参与；复杂嵌套（mapping、
/// 带 `key: value` 的列表项）直接忽略。
fn parse_aliases(content: &str) -> Vec<String> {
    let Some(parts) = split_frontmatter(content) else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    let mut in_list = false;
    for line in parts.block.lines() {
        let trimmed = line.trim();
        if in_list && trimmed.starts_with('-') {
            let item = trimmed.trim_start_matches('-').trim();
            // `- key: value` 是 YAML mapping 列表项，属于不支持的复杂嵌套。
            if item.contains(": ") || item.ends_with(':') {
                in_list = false;
                continue;
            }
            for alias in split_alias_value(item) {
                push_alias(&alias, &mut aliases, &mut seen);
            }
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            let is_alias_key =
                key.eq_ignore_ascii_case("aliases") || key.eq_ignore_ascii_case("alias");
            // 只接受顶格的键；缩进的 `aliases` 属于不支持的嵌套结构。
            let top_level = !line.starts_with(' ') && !line.starts_with('\t');
            if top_level && is_alias_key {
                in_list = true;
                for alias in split_alias_value(value) {
                    push_alias(&alias, &mut aliases, &mut seen);
                }
                continue;
            }
            in_list = false;
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            // 缩进注释保留列表状态，其它普通行结束列表。
            in_list = false;
        }
    }
    aliases
}

/// YAML 别名值：`[a, b]` / `a, b` / 单个值；flow mapping 等复杂嵌套忽略。
fn split_alias_value(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = if value.starts_with('[') {
        value.find(']').map_or(value, |end| &value[..=end])
    } else if value.starts_with('{') {
        return Vec::new();
    } else {
        // 不在 flow 列表内时，` #` 之后是 YAML 行内注释。
        value.split(" #").next().unwrap_or(value)
    };
    let value = value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(',')
        .map(|part| part.trim().trim_matches(['"', '\'']).trim())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn push_alias(raw: &str, aliases: &mut Vec<String>, seen: &mut HashSet<String>) {
    let raw = raw.trim().trim_matches(['"', '\'']).trim();
    if raw.is_empty() {
        return;
    }
    let alias = raw.to_lowercase();
    if seen.insert(alias.clone()) {
        aliases.push(alias);
    }
}

/// 页面里的标签：frontmatter `tags`（列表 / 逗号分隔）+ 正文行内 `#tag`。
fn parse_tags(content: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    let body = match split_frontmatter(content) {
        Some(parts) => {
            collect_frontmatter_tags(parts.block, &mut tags, &mut seen);
            parts.body
        }
        None => content,
    };
    collect_inline_tags(body, &mut tags, &mut seen);
    tags
}

fn collect_frontmatter_tags(block: &str, tags: &mut Vec<String>, seen: &mut HashSet<String>) {
    let mut in_list = false;
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            if key.eq_ignore_ascii_case("tags") || key.eq_ignore_ascii_case("tag") {
                in_list = true;
                for part in split_tag_value(value) {
                    push_tag(&part, tags, seen);
                }
                continue;
            }
            in_list = false;
            continue;
        }
        if in_list && trimmed.starts_with('-') {
            for part in split_tag_value(trimmed.trim_start_matches('-')) {
                push_tag(&part, tags, seen);
            }
        } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
            // 缩进注释保留列表状态，其它普通行结束列表。
            in_list = false;
        }
    }
}

/// YAML 标签值：`[a, b]` / `a, b` / `a b`（tag 本身不能含空格）。
fn split_tag_value(value: &str) -> Vec<String> {
    let value = value.trim();
    // 不在 flow 列表内时，` #` 之后是 YAML 行内注释。
    let value = if value.starts_with('[') {
        value.find(']').map_or(value, |end| &value[..=end])
    } else {
        value.split(" #").next().unwrap_or(value)
    };
    let value = value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn push_tag(raw: &str, tags: &mut Vec<String>, seen: &mut HashSet<String>) {
    if let Some(tag) = normalize_tag(raw)
        && seen.insert(tag.clone())
    {
        tags.push(tag);
    }
}

/// 归一化标签：去 `#`/引号，只允许字母数字（含中文）、`_`、`-`、`/`，小写。
fn normalize_tag(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches(|character: char| {
        matches!(
            character,
            '#' | '"' | '\'' | '[' | ']' | ',' | ' ' | '\t' | '\r' | '\n'
        )
    });
    if raw.is_empty() {
        return None;
    }
    let mut normalized = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-' | '/') {
            normalized.push(character);
        } else {
            return None;
        }
    }
    let normalized = normalized.trim_end_matches('/');
    // 至少保留一个非数字字符，避免把 `#123` 当成标签。
    if normalized.is_empty()
        || !normalized
            .chars()
            .any(|character| character.is_alphanumeric() && !character.is_numeric())
    {
        return None;
    }
    Some(normalized.to_lowercase())
}

fn is_tag_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '/')
}

fn is_tag_boundary(previous: Option<char>) -> bool {
    match previous {
        None => true,
        Some(character) => {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | '['
                        | '（'
                        | '【'
                        | '「'
                        | '《'
                        | '，'
                        | '。'
                        | '、'
                        | '；'
                        | '：'
                        | '！'
                        | '？'
                        | ','
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                        | '"'
                        | '\''
                        | '“'
                        | '”'
                        | '‘'
                        | '’'
                )
        }
    }
}

/// 正文行内标签；忽略围栏代码块，并要求 `#` 前是空白/标点，避免 URL 片段误判。
fn collect_inline_tags(body: &str, tags: &mut Vec<String>, seen: &mut HashSet<String>) {
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let characters: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < characters.len() {
            let is_link_anchor = index >= 2
                && matches!(
                    (characters[index - 2], characters[index - 1]),
                    ('[', '[') | (']', '(')
                );
            if characters[index] == '#'
                && !is_link_anchor
                && is_tag_boundary(if index == 0 {
                    None
                } else {
                    Some(characters[index - 1])
                })
            {
                let mut end = index + 1;
                while end < characters.len() && is_tag_char(characters[end]) {
                    end += 1;
                }
                if end > index + 1 {
                    let raw: String = characters[index + 1..end].iter().collect();
                    push_tag(&raw, tags, seen);
                }
                index = end;
                continue;
            }
            index += 1;
        }
    }
}

/// 重建单篇笔记的标签行（先清空再写入，幂等）。
fn rebuild_note_tags(conn: &Connection, note_id_value: &str, content: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM vault_note_tags WHERE note_id = ?1",
        params![note_id_value],
    )
    .map_err(db_err)?;
    let tags = parse_tags(content);
    if tags.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("INSERT OR IGNORE INTO vault_note_tags (note_id, tag) VALUES (?1, ?2)")
        .map_err(db_err)?;
    for tag in tags {
        stmt.execute(params![note_id_value, tag]).map_err(db_err)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 属性（frontmatter 编辑）
// ---------------------------------------------------------------------------

fn is_property_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn format_yaml_property(key: &str, value: &str) -> String {
    let plain = !value.is_empty()
        && !value.starts_with(' ')
        && !value.starts_with('\t')
        && !value.ends_with(' ')
        && !value.ends_with('\t')
        && !value.starts_with('-')
        && !value.starts_with('?')
        && !value.chars().any(|character| {
            matches!(
                character,
                ':' | '#'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
                    | '\n'
                    | '\r'
                    | '\t'
            )
        });
    if plain {
        format!("{key}: {value}")
    } else {
        let quoted = serde_json::to_string(value)
            .unwrap_or_else(|_| format!("\"{}\"", value.replace('"', "\\\"").replace('\n', "\\n")));
        format!("{key}: {quoted}")
    }
}

/// 合并 frontmatter：替换传入键、空值删除、其它键与正文原样保留。
fn set_frontmatter_properties(content: &str, properties: &BTreeMap<String, String>) -> String {
    if properties.is_empty() {
        return content.to_string();
    }
    let bom_len = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let bom = &content[..bom_len];

    if let Some(parts) = split_frontmatter(content) {
        let mut lines: Vec<String> = Vec::new();
        let mut handled: HashSet<String> = HashSet::new();
        let mut skip_indented = false;
        for line in parts.block.lines() {
            let trimmed = line.trim();
            let top_level =
                !trimmed.is_empty() && !line.starts_with(' ') && !line.starts_with('\t');
            if top_level && let Some((key, _)) = line.split_once(':') {
                let key = key.trim();
                if let Some(value) = properties.get(key) {
                    skip_indented = true;
                    if handled.insert(key.to_string()) && !value.is_empty() {
                        lines.push(format_yaml_property(key, value));
                    }
                    continue;
                }
                skip_indented = false;
            } else if skip_indented && (line.starts_with(' ') || line.starts_with('\t')) {
                continue;
            }
            if !trimmed.is_empty() {
                skip_indented = false;
            }
            lines.push(line.to_string());
        }
        for (key, value) in properties {
            if value.is_empty() || handled.contains(key.as_str()) {
                continue;
            }
            lines.push(format_yaml_property(key, value));
        }

        let mut out = String::with_capacity(content.len() + 32);
        if lines.is_empty() {
            out.push_str(bom);
            out.push_str(parts.body);
        } else {
            out.push_str(bom);
            out.push_str("---");
            out.push_str(parts.newline);
            out.push_str(&lines.join(parts.newline));
            out.push_str(parts.newline);
            out.push_str("---");
            out.push_str(parts.newline);
            out.push_str(parts.body);
        }
        return out;
    }

    let lines: Vec<String> = properties
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format_yaml_property(key, value))
        .collect();
    if lines.is_empty() {
        return content.to_string();
    }
    let newline = if content[bom_len..].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = String::with_capacity(content.len() + 64);
    out.push_str(bom);
    out.push_str("---");
    out.push_str(newline);
    out.push_str(&lines.join(newline));
    out.push_str(newline);
    out.push_str("---");
    out.push_str(newline);
    out.push_str(&content[bom_len..]);
    out
}

// ---------------------------------------------------------------------------
// 重命名：wikilink 改写
// ---------------------------------------------------------------------------

fn path_without_markdown_extension(path: &str) -> String {
    let stem = path
        .get(path.len().saturating_sub(3)..)
        .filter(|suffix| suffix.eq_ignore_ascii_case(".md"))
        .map(|_| &path[..path.len() - 3])
        .unwrap_or(path);
    stem.to_string()
}

fn rename_replacements(
    old_path: &str,
    new_path: &str,
    old_title: &str,
    new_title: &str,
) -> Vec<(String, String)> {
    let old_stem = Path::new(old_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_stem = Path::new(new_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut pairs: Vec<(String, String)> = Vec::new();
    let candidates = [
        (old_title.to_string(), new_title.to_string()),
        (old_stem, new_stem),
        (
            path_without_markdown_extension(old_path),
            path_without_markdown_extension(new_path),
        ),
    ];
    for (old, new) in candidates {
        let old = old.trim();
        let new = new.trim();
        if old.is_empty() || new.is_empty() || old == new {
            continue;
        }
        let key = old.to_lowercase();
        if pairs.iter().any(|(existing, _)| existing == &key) {
            continue;
        }
        pairs.push((key, new.to_string()));
    }
    pairs
}

/// 改写单个文件里的 `[[旧目标]]` / `[[旧目标#标题]]` / `[[旧目标|别名]]`；
/// 围栏代码块（``` / ~~~）内原样保留。
fn rewrite_wikilinks(content: &str, replacements: &[(String, String)]) -> String {
    if replacements.is_empty() || !content.contains("[[") {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let mut in_fence = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&rewrite_wikilinks_line(line, replacements));
    }
    out
}

/// 改写一行内的 wikilink（行尾换行符原样保留）。
fn rewrite_wikilinks_line(line: &str, replacements: &[(String, String)]) -> String {
    if !line.contains("[[") {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let (target_part, alias_suffix) = match inner.split_once('|') {
            Some((target, alias)) => (target, Some(alias)),
            None => (inner, None),
        };
        let (target, heading_suffix) = match target_part.split_once('#') {
            Some((target, heading)) => (target, Some(heading)),
            None => (target_part, None),
        };
        let key = target.trim().to_lowercase();
        match replacements.iter().find(|(old, _)| old == &key) {
            Some((_, new)) => {
                out.push_str("[[");
                out.push_str(new);
                if let Some(heading) = heading_suffix {
                    out.push('#');
                    out.push_str(heading);
                }
                if let Some(alias) = alias_suffix {
                    out.push('|');
                    out.push_str(alias);
                }
                out.push_str("]]");
            }
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// 全库改写 wikilink，并同步 DB 正文缓存、FTS 与标签。
fn rewrite_vault_links(
    conn: &Connection,
    vault_id: &str,
    replacements: &[(String, String)],
) -> Result<(), String> {
    let vault = get_vault(conn, vault_id)?;
    let root = vault.root_path;
    let notes: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, path, body FROM vault_notes
                 WHERE vault_id = ?1 ORDER BY path ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![vault_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };

    let at = now(conn)?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    for (id, path, cached_body) in notes {
        let absolute = Path::new(&root).join(&path);
        let content = fs::read_to_string(&absolute).unwrap_or(cached_body);
        let updated = rewrite_wikilinks(&content, replacements);
        if updated == content {
            continue;
        }
        write_atomic(&absolute, &updated)?;
        let title = markdown_title(Path::new(&path), &updated);
        tx.execute(
            "UPDATE vault_notes SET title = ?1, body = ?2, updated_at = ?3 WHERE id = ?4",
            params![title, updated, at, id],
        )
        .map_err(db_err)?;
        index_note_fts(&tx, &id, vault_id, &title, &updated)?;
        rebuild_note_tags(&tx, &id, &updated)?;
    }
    rebuild_links(&tx, vault_id)?;
    tx.commit().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 附件路径 / base64
// ---------------------------------------------------------------------------

/// 文件名清洗：字母数字（含中文）、`_`、`-`；空白折叠为 `-`，截断到 `max_chars`。
fn sanitize_name_part(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for character in input.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            out.push(character);
            last_dash = character == '-';
        } else if character.is_whitespace() && !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let limited: String = out.trim_matches('-').chars().take(max_chars).collect();
    limited.trim_matches('-').to_string()
}

/// 从用户文件名取扩展名：只保留 ASCII 字母数字，缺失/非法时用 `bin`。
fn attachment_extension(file_name: &str) -> String {
    let normalized = file_name.trim().replace('\\', "/");
    let name = Path::new(&normalized)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.trim().to_lowercase())
        .unwrap_or_default();
    if !extension.is_empty()
        && extension.chars().count() <= 16
        && extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        extension
    } else {
        "bin".to_string()
    }
}

/// 只接受 `attachments/...` 相对路径，拒绝绝对路径与 `..`。
fn normalize_attachment_path(input: &str) -> Result<String, String> {
    let normalized = input.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("附件路径不能为空".to_string());
    }
    if normalized.starts_with('/') {
        return Err("附件路径必须是相对笔记库的路径".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("附件路径不能包含 ..".to_string()),
            other => {
                if other.contains('\0') {
                    return Err("附件路径包含非法字符".to_string());
                }
                parts.push(other);
            }
        }
    }
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("attachments") {
        return Err("只允许读取 attachments/ 目录下的附件".to_string());
    }
    let rest = parts[1..].join("/");
    if rest.is_empty() {
        return Err("附件路径不能为空".to_string());
    }
    Ok(format!("attachments/{rest}"))
}

/// 标准 base64（兼容 URL-safe 与 data URL 前缀）；解码后超过 25 MB 直接报错。
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim();
    let payload = if let Some(rest) = input.strip_prefix("data:") {
        rest.split_once(";base64,")
            .map(|(_, payload)| payload)
            .unwrap_or(input)
    } else {
        input
    };
    if payload.len() as u64 > MAX_ATTACHMENT_BYTES * 2 {
        return Err("附件超过 25 MB".to_string());
    }
    let mut out: Vec<u8> = Vec::with_capacity(payload.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in payload.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => {
                return Err(format!(
                    "附件数据不是合法 base64（非法字符：{}）",
                    char::from(byte)
                ));
            }
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1u32 << bits) - 1;
        }
        if out.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err("附件超过 25 MB".to_string());
        }
    }
    if bits == 6 {
        return Err("附件数据不是合法 base64（长度无效）".to_string());
    }
    Ok(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        out.push(char::from(ALPHABET[(first >> 2) as usize]));
        out.push(char::from(
            ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize],
        ));
        if chunk.len() > 1 {
            out.push(char::from(
                ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize],
            ));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[(third & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
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
        let body = line.trim_end_matches(['\r', '\n']);
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
        if !in_fence
            && !replaced
            && let Some(text) = body.trim_start().strip_prefix("# ")
            && !text.trim().is_empty()
        {
            let indent = &body[..body.len() - body.trim_start().len()];
            out.push_str(indent);
            out.push_str(&heading);
            out.push_str(&line[body.len()..]);
            replaced = true;
            continue;
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
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return bom;
    }
    offset += first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
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
    fn wikilink_aliases_resolve_from_frontmatter() {
        let fixture = Fixture::new("aliases");
        let vault = fixture.vault("brain");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        // flow 列表：去引号、大小写折叠、重复项只留一个。
        Fixture::write(
            root,
            "flow.md",
            "---\naliases: [Flow Alias, flow alias, \"Quoted Alias\", '单引号别名']\n---\n# Flow Note\n\n正文\n",
        );
        // 块列表，`alias` 单数同样生效。
        Fixture::write(
            root,
            "block.md",
            "---\nalias:\n  - Block One\n  - 'Block Two'\n---\n# Block Note\n\n正文\n",
        );
        // 逗号分隔。
        Fixture::write(
            root,
            "comma.md",
            "---\naliases: Comma One, Comma Two\n---\n# Comma Note\n\n正文\n",
        );
        // 没有 aliases 的旧笔记继续按标题解析。
        Fixture::write(root, "plain.md", "# Plain Note\n\n正文\n");
        // 正文里的 aliases 字样不是 frontmatter，不参与解析。
        Fixture::write(root, "body.md", "# Body Note\n\naliases: Fake Alias\n");
        Fixture::write(
            root,
            "source.md",
            "# Source\n\n[[FLOW ALIAS]] [[Quoted Alias]] [[单引号别名]] [[block one]] [[Block Two]] \
             [[Comma One]] [[comma two]] [[Plain Note]] [[Fake Alias]]\n",
        );

        assert_eq!(index_vault(conn, &vault).unwrap(), 6);

        let backlink_targets = |note_id: &str| -> Vec<String> {
            note_backlinks(conn, note_id)
                .unwrap()
                .into_iter()
                .map(|backlink| backlink.target)
                .collect()
        };

        let flow = title_of(conn, &vault.id, "Flow Note");
        let mut targets = backlink_targets(&flow.id);
        targets.sort();
        assert_eq!(targets, vec!["FLOW ALIAS", "Quoted Alias", "单引号别名"]);

        let block = title_of(conn, &vault.id, "Block Note");
        let mut targets = backlink_targets(&block.id);
        targets.sort();
        assert_eq!(targets, vec!["Block Two", "block one"]);

        let comma = title_of(conn, &vault.id, "Comma Note");
        let mut targets = backlink_targets(&comma.id);
        targets.sort();
        assert_eq!(targets, vec!["Comma One", "comma two"]);

        let plain = title_of(conn, &vault.id, "Plain Note");
        assert_eq!(backlink_targets(&plain.id), vec!["Plain Note"]);

        let unresolved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vault_links
                 WHERE vault_id = ?1 AND target = 'Fake Alias' AND resolved_note IS NULL",
                params![vault.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved, 1, "正文里的 aliases 不应参与解析");
    }

    #[test]
    fn wikilink_alias_conflicts_follow_path_order() {
        let fixture = Fixture::new("alias-conflict");
        let vault = fixture.vault("brain");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        // a.md 的别名先于 b.md 的标题注册 → 别名胜。
        Fixture::write(root, "a.md", "---\naliases: [Conflict]\n---\n# A Note\n");
        Fixture::write(root, "b.md", "# Conflict\n");
        // c.md 的标题先于 d.md 的别名注册 → 标题胜。
        Fixture::write(root, "c.md", "# Winner\n");
        Fixture::write(root, "d.md", "---\naliases: [Winner]\n---\n# D Note\n");
        Fixture::write(
            root,
            "source.md",
            "# Source\n\n[[Conflict]] 与 [[winner]]\n",
        );

        assert_eq!(index_vault(conn, &vault).unwrap(), 5);

        let a = title_of(conn, &vault.id, "A Note");
        let b = title_of(conn, &vault.id, "Conflict");
        let c = title_of(conn, &vault.id, "Winner");
        let d = title_of(conn, &vault.id, "D Note");

        let a_backlinks = note_backlinks(conn, &a.id).unwrap();
        assert_eq!(a_backlinks.len(), 1);
        assert_eq!(a_backlinks[0].target, "Conflict");
        assert!(
            note_backlinks(conn, &b.id).unwrap().is_empty(),
            "冲突时路径靠前的笔记胜出"
        );

        let c_backlinks = note_backlinks(conn, &c.id).unwrap();
        assert_eq!(c_backlinks.len(), 1);
        assert_eq!(c_backlinks[0].target, "winner");
        assert!(
            note_backlinks(conn, &d.id).unwrap().is_empty(),
            "冲突时路径靠前的笔记胜出"
        );
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

    #[test]
    fn tag_parsing_frontmatter_inline_and_code_blocks() {
        let content = "---\ntags: [Alpha, 中文标签, dev/work]\n---\n# 标题\n\n#正文标签 与 #dev/ops. 还有 #tag-name_1\n\n```rust\n#代码块\n```\n\nhttps://example.com/page#fragment 与 #real\n";
        assert_eq!(
            parse_tags(content),
            vec![
                "alpha",
                "中文标签",
                "dev/work",
                "正文标签",
                "dev/ops",
                "tag-name_1",
                "real"
            ]
        );

        let block_list = "---\ntags:\n  - One\n  - two/three\n---\n\n正文\n";
        assert_eq!(parse_tags(block_list), vec!["one", "two/three"]);

        let comma_string = "---\ntags: a, b\n---\n\n正文\n";
        assert_eq!(parse_tags(comma_string), vec!["a", "b"]);

        // 围栏内与 URL 片段都不算标签；重复标签去重。
        let fenced = "```\n#hidden #重复\n```\n#重复 #可见\n";
        assert_eq!(parse_tags(fenced), vec!["重复", "可见"]);
        assert!(normalize_tag("#Rust").is_some_and(|tag| tag == "rust"));
        assert!(normalize_tag("bad tag").is_none());
        assert!(normalize_tag("...").is_none());

        // 纯数字不是标签，混有字母/文字的数字标签仍有效。
        assert!(normalize_tag("#123").is_none());
        assert!(normalize_tag("2024/01").is_none());
        assert!(normalize_tag("#v2").is_some_and(|tag| tag == "v2"));
        assert!(normalize_tag("2024-notes").is_some_and(|tag| tag == "2024-notes"));
        assert_eq!(
            parse_tags("#123 #v2 与 #2024-notes\n"),
            vec!["v2", "2024-notes"]
        );

        // wikilink 标题与 markdown 锚点里的 # 不是行内标签。
        let anchors = "[[#标题]] 与 [文字](#锚点) 以及 #真标签\n";
        assert_eq!(parse_tags(anchors), vec!["真标签"]);
    }

    #[test]
    fn alias_parsing_flow_block_and_comma() {
        // flow 列表：去引号、trim、小写、去重。
        assert_eq!(
            parse_aliases("---\naliases: [Alpha, \"Beta Gamma\", alpha, 'Delta']\n---\n正文\n"),
            vec!["alpha", "beta gamma", "delta"]
        );
        // 块列表：`alias` 单数与缩进 `-` 行。
        assert_eq!(
            parse_aliases("---\nalias:\n  - One\n  - \"Two\"\n  - one\n---\n"),
            vec!["one", "two"]
        );
        // 逗号分隔。
        assert_eq!(
            parse_aliases("---\naliases: One, Two ,three\n---\n"),
            vec!["one", "two", "three"]
        );
        // 正文里的 aliases 字样不参与。
        assert!(parse_aliases("# 标题\n\naliases: [Fake]\n").is_empty());
        // 复杂嵌套与 flow mapping 忽略。
        assert!(parse_aliases("---\naliases:\n  nested:\n    - x\n---\n").is_empty());
        assert!(parse_aliases("---\nmeta:\n  aliases: [Fake]\n---\n").is_empty());
        assert!(parse_aliases("---\naliases: {a: b}\n---\n").is_empty());
        assert!(parse_aliases("# Plain\n").is_empty());
        // 其它 frontmatter 键不影响别名解析。
        assert_eq!(
            parse_aliases("---\ntitle: T\ntags: [a]\naliases:\n  - A\n---\n"),
            vec!["a"]
        );
    }

    #[test]
    fn tags_are_indexed_and_queryable() {
        let fixture = Fixture::new("tags");
        let vault = fixture.vault("tags");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        Fixture::write(
            root,
            "A.md",
            "---\ntags: [Rust, 中文]\n---\n\n# A\n\n#共享\n",
        );
        Fixture::write(root, "B.md", "# B\n\n#rust #共享 #dev/ops\n");
        assert_eq!(index_vault(conn, &vault).unwrap(), 2);

        let tags = vault_tags(conn, &vault.id).unwrap();
        let counts: HashMap<&str, i64> = tags
            .iter()
            .map(|tag| (tag.tag.as_str(), tag.count))
            .collect();
        assert_eq!(counts.get("rust"), Some(&2));
        assert_eq!(counts.get("共享"), Some(&2));
        assert_eq!(counts.get("中文"), Some(&1));
        assert_eq!(counts.get("dev/ops"), Some(&1));
        assert!(
            tags.windows(2).all(|pair| pair[0].count >= pair[1].count),
            "{tags:?}"
        );

        let rust = notes_by_tag(conn, &vault.id, "RUST").unwrap();
        assert_eq!(
            rust.iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert!(rust.iter().all(|entry| !entry.is_dir));
        assert_eq!(notes_by_tag(conn, &vault.id, "中文").unwrap().len(), 1);
        assert!(notes_by_tag(conn, &vault.id, " ").is_err());

        // 保存后标签随之重建。
        let b = title_of(conn, &vault.id, "B");
        save_vault_note(conn, &b.id, None, Some("# B\n\n没有标签\n"), None).unwrap();
        let counts: HashMap<String, i64> = vault_tags(conn, &vault.id)
            .unwrap()
            .into_iter()
            .map(|tag| (tag.tag, tag.count))
            .collect();
        assert_eq!(counts.get("rust"), Some(&1));
        assert_eq!(counts.get("共享"), Some(&1));
        assert_eq!(counts.get("dev/ops"), None);
    }

    #[test]
    fn attachments_sanitize_and_reject_escapes() {
        let fixture = Fixture::new("attachments");
        let vault = fixture.vault("attachments");
        let conn = fixture.db.conn();
        let note = create_vault_note(conn, &vault, "assets/base", None, Some("# Base")).unwrap();

        let relative =
            save_vault_attachment(conn, &vault.id, &note.id, "../../危险 文件.PNG", "aGVsbG8=")
                .unwrap();
        let name = relative
            .strip_prefix("attachments/")
            .expect("落在 attachments/");
        assert!(name.starts_with("base-"), "{relative}");
        assert!(name.ends_with(".png"), "{relative}");
        assert!(!name.contains("..") && !name.contains('/'), "{relative}");
        assert_eq!(name.matches('-').count(), 1, "{relative}");
        let disk = Path::new(&vault.root_path).join(&relative);
        assert_eq!(fs::read_to_string(&disk).unwrap(), "hello");

        let encoded = read_vault_attachment(conn, &vault.id, &relative).unwrap();
        assert_eq!(base64_decode(&encoded).unwrap(), b"hello".as_slice());
        assert_eq!(encoded, "aGVsbG8=");

        assert!(read_vault_attachment(conn, &vault.id, "../temli.db").is_err());
        assert!(read_vault_attachment(conn, &vault.id, "attachments/../temli.db").is_err());
        assert!(read_vault_attachment(conn, &vault.id, "notes/base.md").is_err());
        assert!(read_vault_attachment(conn, &vault.id, &format!("/{relative}")).is_err());
        assert!(save_vault_attachment(conn, &vault.id, &note.id, "a.txt", "***").is_err());

        let big = vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1];
        let encoded = base64_encode(&big);
        let error =
            save_vault_attachment(conn, &vault.id, &note.id, "big.bin", &encoded).unwrap_err();
        assert!(error.contains("25 MB"), "{error}");
    }

    #[test]
    fn templates_scan_and_create_from_content() {
        let fixture = Fixture::new("templates");
        let vault = fixture.vault("templates");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        Fixture::write(root, "templates/Daily.md", "# 日记模板\n\n- [ ] 待办\n");
        Fixture::write(root, "templates/nested/Deep.md", "# Deep\n");
        Fixture::write(root, "elsewhere/Other.md", "# Other\n");

        let templates = list_vault_templates(conn, &vault.id).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].path, "templates/Daily.md");
        assert_eq!(templates[0].title, "日记模板");
        assert!(!templates[0].is_dir);
        assert!(!templates[0].id.is_empty());

        let content = fs::read_to_string(Path::new(root).join("templates/Daily.md")).unwrap();
        let note =
            create_vault_note_with_content(conn, &vault, "Journal/2026-11-09", &content).unwrap();
        assert_eq!(note.path, "Journal/2026-11-09.md");
        assert_eq!(note.title, "日记模板");
        assert_eq!(
            fs::read_to_string(Path::new(root).join(&note.path)).unwrap(),
            content
        );
        assert!(
            create_vault_note_with_content(conn, &vault, "Journal/2026-11-09", "重复").is_err()
        );
        assert!(create_vault_note_with_content(conn, &vault, "../escape", "越界").is_err());
    }

    #[test]
    fn properties_merge_keep_body_and_reindex_tags() {
        let fixture = Fixture::new("properties");
        let vault = fixture.vault("properties");
        let conn = fixture.db.conn();
        let note = create_vault_note_with_content(
            conn,
            &vault,
            "props",
            "---\ntitle: 旧标题\ntags: [a, keep]\naliases: [x]\nstatus: draft\n---\n\n# 正文标题\n\n正文内容\n",
        )
        .unwrap();

        let mut properties = BTreeMap::new();
        properties.insert("status".to_string(), "done".to_string());
        properties.insert("tags".to_string(), "b/c, 中文".to_string());
        properties.insert("aliases".to_string(), String::new());
        properties.insert("owner".to_string(), "lhy".to_string());

        let updated = set_note_properties(conn, &note.id, &properties).unwrap();
        let body = &updated.body;
        assert!(body.contains("title: 旧标题"), "{body}");
        assert!(body.contains("status: done"), "{body}");
        assert!(body.contains("owner: lhy"), "{body}");
        assert!(body.contains("tags: \"b/c, 中文\""), "{body}");
        assert!(!body.contains("draft"), "{body}");
        assert!(!body.contains("aliases"), "{body}");
        assert!(body.contains("# 正文标题\n\n正文内容"), "{body}");
        assert_eq!(updated.title, "正文标题");

        let counts: HashMap<String, i64> = vault_tags(conn, &vault.id)
            .unwrap()
            .into_iter()
            .map(|tag| (tag.tag, tag.count))
            .collect();
        assert_eq!(counts.get("b/c"), Some(&1));
        assert_eq!(counts.get("中文"), Some(&1));
        assert_eq!(counts.get("a"), None);
        assert_eq!(counts.get("keep"), None);

        // 没有 frontmatter 的笔记：新建 frontmatter，正文完整保留。
        let plain =
            create_vault_note_with_content(conn, &vault, "plain", "# Plain\n\n正文\n").unwrap();
        let mut properties = BTreeMap::new();
        properties.insert("foo".to_string(), "bar".to_string());
        let updated = set_note_properties(conn, &plain.id, &properties).unwrap();
        assert_eq!(updated.body, "---\nfoo: bar\n---\n# Plain\n\n正文\n");

        let mut invalid = BTreeMap::new();
        invalid.insert("bad key".to_string(), "x".to_string());
        assert!(set_note_properties(conn, &plain.id, &invalid).is_err());
    }

    #[test]
    fn rename_recomputes_id_and_rewrites_wikilinks() {
        let fixture = Fixture::new("rename");
        let vault = fixture.vault("rename");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        let target = create_vault_note(conn, &vault, "Old", None, Some("# Old\n\n正文")).unwrap();
        let source = create_vault_note(
            conn,
            &vault,
            "Source",
            None,
            Some("# Source\n\n[[Old]]、[[Old|别名]]、[[Old#小节|带锚]]、[[Other]] 与 #标签\n"),
        )
        .unwrap();
        create_vault_note(conn, &vault, "Other", None, Some("# Other")).unwrap();

        let renamed = rename_vault_note(conn, &target.id, "Folder/New Name").unwrap();
        assert_eq!(renamed.id, note_id(&vault.id, "Folder/New Name.md"));
        assert_ne!(renamed.id, target.id, "重命名后按新路径重算 id");
        assert_eq!(renamed.path, "Folder/New Name.md");
        assert!(!Path::new(root).join("Old.md").exists());
        assert!(Path::new(root).join("Folder/New Name.md").exists());
        assert!(get_vault_note(conn, &target.id).is_err(), "旧 id 应失效");
        let old_fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE note_id = ?1",
                params![target.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_fts, 0, "旧 id 的 FTS 行应删除");
        let new_fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE note_id = ?1",
                params![renamed.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_fts, 1, "新 id 应写入 FTS");
        assert!(
            search_vault(conn, &vault.id, "正文")
                .unwrap()
                .iter()
                .any(|hit| hit.id == renamed.id),
            "重命名后新 id 应能被检索到"
        );

        let source_body = fs::read_to_string(Path::new(root).join(&source.path)).unwrap();
        assert!(source_body.contains("[[New Name]]"), "{source_body}");
        assert!(source_body.contains("[[New Name|别名]]"), "{source_body}");
        assert!(
            source_body.contains("[[New Name#小节|带锚]]"),
            "{source_body}"
        );
        assert!(source_body.contains("[[Other]]"), "{source_body}");
        assert!(source_body.contains("#标签"), "{source_body}");

        assert_eq!(get_vault_note(conn, &source.id).unwrap().body, source_body);
        let backlinks = note_backlinks(conn, &renamed.id).unwrap();
        assert_eq!(backlinks.len(), 1, "{backlinks:?}");
        assert_eq!(backlinks[0].target, "New Name");
        assert_eq!(
            vault_tree(conn, &vault.id)
                .unwrap()
                .iter()
                .filter(|entry| entry.title == "Old")
                .count(),
            1
        );

        assert!(rename_vault_note(conn, &renamed.id, "Source").is_err());
        assert!(rename_vault_note(conn, &renamed.id, "../escape").is_err());
    }

    #[test]
    fn rename_restores_files_when_db_write_fails() {
        let fixture = Fixture::new("rename-rollback");
        let vault = fixture.vault("rename-rollback");
        let conn = fixture.db.conn();
        let root = vault.root_path.as_str();

        let note = create_vault_note(conn, &vault, "Keep", None, Some("# Keep\n\n原正文")).unwrap();
        let blocked_id = note_id(&vault.id, "Broken/Moved.md");
        conn.execute_batch(&format!(
            "CREATE TRIGGER fail_rename BEFORE INSERT ON vault_notes
             WHEN NEW.id = '{blocked_id}'
             BEGIN SELECT RAISE(ABORT, 'boom'); END;"
        ))
        .unwrap();

        assert!(rename_vault_note(conn, &note.id, "Broken/Moved").is_err());

        let restored = Path::new(root).join("Keep.md");
        assert!(restored.exists(), "DB 失败应恢复旧文件");
        assert_eq!(fs::read_to_string(&restored).unwrap(), "# Keep\n\n原正文");
        assert!(
            !Path::new(root).join("Broken/Moved.md").exists(),
            "DB 失败应删除新文件"
        );
        assert_eq!(get_vault_note(conn, &note.id).unwrap().path, "Keep.md");
        let moved_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vault_notes WHERE id = ?1",
                params![blocked_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_rows, 0, "失败的事务不应留下新行");
    }

    #[test]
    fn wikilink_rewrite_skips_fenced_code() {
        let content = "前 [[Old]]\n```md\n[[Old]] 与 [[Old|别名]]\n```\n~~~\n[[Old#锚]]\n~~~\n后 [[Old#小节|带锚]]\n";
        let replacements = vec![("old".to_string(), "New".to_string())];
        assert_eq!(
            rewrite_wikilinks(content, &replacements),
            "前 [[New]]\n```md\n[[Old]] 与 [[Old|别名]]\n```\n~~~\n[[Old#锚]]\n~~~\n后 [[New#小节|带锚]]\n"
        );
        // 没有尾部换行的最后一行同样处理。
        assert_eq!(rewrite_wikilinks("[[Old]]", &replacements), "[[New]]");
        assert_eq!(
            rewrite_wikilinks("```\n[[Old]]\n```\n[[Old]]", &replacements),
            "```\n[[Old]]\n```\n[[New]]"
        );
    }
}
