use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 任务附件
// ---------------------------------------------------------------------------

/// 任务附件上限：20 MiB（base64 解码后的字节数）。
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// 磁盘文件名主干上限（UTF-8 字节数）。最坏情况完整文件名约
/// 8+1+120+1+16=146 字节，临时名再加 13 字节，远低于文件系统 255 字节上限。
const MAX_ATTACHMENT_STEM_BYTES: usize = 120;

/// 保存任务附件：base64 解码后写入
/// `<data_dir>/attachments/<task_id>/<uuid8>-<sanitized_name>`。
///
/// 文件名只保留最后一段并清洗，扩展名只保留 1–16 位 ASCII 字母数字（否则
/// 回退为 `bin`）；先写同目录临时文件再原子重命名。超过 20 MiB 拒绝。
pub fn add_task_attachment(
    conn: &Connection,
    data_dir: &Path,
    task_id: &str,
    file_name: &str,
    data_base64: &str,
) -> Result<Attachment, String> {
    ensure_task(conn, task_id)?;
    if data_base64.trim().is_empty() {
        return Err("附件内容为空".to_string());
    }
    let bytes = decode_attachment_base64(data_base64)?;
    if bytes.is_empty() {
        return Err("附件内容为空".to_string());
    }
    let (file_name, stored_file_name, extension) = clean_attachment_file_name(file_name)?;
    let mime = attachment_mime(&extension);

    let directory = attachment_task_dir(data_dir, task_id)?;
    fs::create_dir_all(&directory).map_err(|err| format!("创建附件目录失败：{err}"))?;
    let attachments_root = data_dir
        .join("attachments")
        .canonicalize()
        .map_err(|err| format!("附件目录不可用：{err}"))?;
    let directory = directory
        .canonicalize()
        .map_err(|err| format!("附件目录不可用：{err}"))?;
    if !directory.starts_with(&attachments_root) {
        return Err("附件目录超出数据目录".to_string());
    }

    let mut stored_name = String::new();
    for _ in 0..8 {
        let unique = Uuid::new_v4().simple().to_string();
        let candidate = format!("{}-{}", &unique[..8], stored_file_name);
        if !directory.join(&candidate).exists() {
            stored_name = candidate;
            break;
        }
    }
    if stored_name.is_empty() {
        return Err("生成附件文件名失败，请重试".to_string());
    }

    let target = directory.join(&stored_name);
    let temp = directory.join(format!(
        ".{stored_name}.{}.tmp",
        &Uuid::new_v4().simple().to_string()[..8]
    ));
    fs::write(&temp, &bytes).map_err(|err| format!("写入附件失败：{err}"))?;
    if let Err(err) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("保存附件失败：{err}"));
    }

    let rel_path = format!("attachments/{task_id}/{stored_name}");
    let id = Uuid::new_v4().to_string();
    let size = i64::try_from(bytes.len()).map_err(|_| "附件超过 20 MB".to_string())?;
    if let Err(err) = conn.execute(
        "INSERT INTO task_attachments (id, task_id, file_name, rel_path, mime, size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, task_id, file_name, rel_path, mime, size],
    ) {
        let _ = fs::remove_file(&target);
        return Err(db_err(err));
    }
    read_attachment(conn, &id)
}

/// 任务下的附件列表，按上传时间升序。
pub fn list_task_attachments(conn: &Connection, task_id: &str) -> Result<Vec<Attachment>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, file_name, mime, size, created_at
             FROM task_attachments WHERE task_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id], read_attachment_row)
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 读取附件内容（base64）。`rel_path` 必须是数据目录内 `attachments/` 下的
/// 相对路径，解析后再次校验真实路径，防止 `..` / 符号链接越界。
pub fn read_task_attachment(
    conn: &Connection,
    data_dir: &Path,
    attachment_id: &str,
) -> Result<String, String> {
    let rel_path = attachment_rel_path(conn, attachment_id)?;
    let target = resolve_attachment_path(data_dir, &rel_path)?;
    let meta = fs::metadata(&target).map_err(|err| format!("读取附件失败：{err}"))?;
    if !meta.is_file() {
        return Err("附件路径不是文件".to_string());
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err("附件超过 20 MB".to_string());
    }
    let bytes = fs::read(&target).map_err(|err| format!("读取附件失败：{err}"))?;
    Ok(encode_attachment_base64(&bytes))
}

/// 删除附件：先删数据库记录，再尽力删除文件；文件删除失败不报错。
pub fn delete_task_attachment(
    conn: &Connection,
    data_dir: &Path,
    attachment_id: &str,
) -> Result<(), String> {
    let rel_path = attachment_rel_path(conn, attachment_id)?;
    conn.execute(
        "DELETE FROM task_attachments WHERE id = ?1",
        params![attachment_id],
    )
    .map_err(db_err)?;
    if let Ok(target) = resolve_attachment_path(data_dir, &rel_path) {
        let _ = fs::remove_file(target);
    }
    Ok(())
}

fn read_attachment(conn: &Connection, attachment_id: &str) -> Result<Attachment, String> {
    conn.query_row(
        "SELECT id, task_id, file_name, mime, size, created_at
         FROM task_attachments WHERE id = ?1",
        params![attachment_id],
        read_attachment_row,
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("附件不存在：{attachment_id}"))
}

fn read_attachment_row(row: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        task_id: row.get(1)?,
        file_name: row.get(2)?,
        mime: row.get(3)?,
        size: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn attachment_rel_path(conn: &Connection, attachment_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT rel_path FROM task_attachments WHERE id = ?1",
        params![attachment_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("附件不存在：{attachment_id}"))
}

fn attachment_task_dir(data_dir: &Path, task_id: &str) -> Result<PathBuf, String> {
    if task_id.is_empty()
        || task_id == "."
        || task_id == ".."
        || task_id.contains(['/', '\\', '\0'])
    {
        return Err("任务 id 非法".to_string());
    }
    Ok(data_dir.join("attachments").join(task_id))
}

/// 只接受数据目录内 `attachments/<task>/<file>` 相对路径；拒绝绝对路径、
/// `..` 与空组件，并通过 canonicalize 校验目标未越出附件根目录。
fn resolve_attachment_path(data_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    let normalized = rel_path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("附件路径不能为空".to_string());
    }
    if normalized.starts_with('/') {
        return Err("附件路径必须是相对数据目录的路径".to_string());
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
    if parts.len() < 3 || !parts[0].eq_ignore_ascii_case("attachments") {
        return Err("只允许读取 attachments/ 目录下的附件".to_string());
    }
    let root = data_dir.join("attachments");
    let real_root = root
        .canonicalize()
        .map_err(|err| format!("附件目录不存在：{err}"))?;
    let target = parts[1..].iter().fold(root, |path, part| path.join(part));
    let real_target = target
        .canonicalize()
        .map_err(|err| format!("附件不存在：{err}"))?;
    if !real_target.starts_with(&real_root) {
        return Err("附件路径超出 attachments 目录".to_string());
    }
    Ok(real_target)
}

/// 文件名清洗：只取最后一段。展示名去掉控制字符并截断；磁盘名的主干只保留
/// 字母数字（含中文）、`_`、`-`，其它字符丢弃，空白折叠为 `-`；扩展名只保留
/// ASCII 字母数字，否则回退为 `bin`。
fn clean_attachment_file_name(input: &str) -> Result<(String, String, String), String> {
    let normalized = input.trim().replace('\\', "/");
    let basename = normalized.rsplit('/').next().unwrap_or_default().trim();
    if basename.is_empty() {
        return Err("文件名不能为空".to_string());
    }
    // 扩展名必须从未截断的原始文件名提取：展示名按 120 字符截断，
    // 长文件名截断后可能丢掉 `.png` 之类后缀，退化成 bin/octet-stream。
    let (stem, extension) = match basename.rsplit_once('.') {
        Some((stem, extension)) if !extension.is_empty() => {
            (stem, sanitize_attachment_extension(extension))
        }
        _ => (basename, "bin".to_string()),
    };
    let stem = sanitize_attachment_stem(stem);
    let display = sanitize_attachment_display_name(basename);
    Ok((display, format!("{stem}.{extension}"), extension))
}

fn sanitize_attachment_display_name(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let trimmed = cleaned.trim();
    let limited: String = trimmed.chars().take(120).collect();
    let limited = limited.trim_end();
    if limited.is_empty() || limited == "." || limited == ".." {
        "file".to_string()
    } else {
        limited.to_string()
    }
}

fn sanitize_attachment_stem(input: &str) -> String {
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
    // 按 UTF-8 字节截断（而非字符数）：CJK 每字 3 字节，80 字符会超过
    // 文件系统 255 字节的单段上限。120 字节留足 uuid 前缀、扩展名与临时
    // 名后缀的余量。
    let mut limited = String::new();
    for character in out.trim_matches('-').chars() {
        if limited.len() + character.len_utf8() > MAX_ATTACHMENT_STEM_BYTES {
            break;
        }
        limited.push(character);
    }
    let limited = limited.trim_matches('-').to_string();
    if limited.is_empty() {
        "file".to_string()
    } else {
        limited
    }
}

fn sanitize_attachment_extension(input: &str) -> String {
    let extension = input.trim().to_lowercase();
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

fn attachment_mime(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "yaml" | "yml" => "text/yaml",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// 标准 base64（兼容 URL-safe 与 data URL 前缀）；解码超过 20 MiB 报错。
fn decode_attachment_base64(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim();
    let payload = if let Some(rest) = input.strip_prefix("data:") {
        rest.split_once(";base64,")
            .map(|(_, payload)| payload)
            .unwrap_or(input)
    } else {
        input
    };
    if payload.len() as u64 > MAX_ATTACHMENT_BYTES * 2 {
        return Err("附件超过 20 MB".to_string());
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
            return Err("附件超过 20 MB".to_string());
        }
    }
    if bits == 6 {
        return Err("附件数据不是合法 base64（长度无效）".to_string());
    }
    Ok(out)
}

fn encode_attachment_base64(bytes: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn task_attachments_roundtrip_and_delete() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let data_dir = temp_dir();

        let content = b"hello attachment";
        let encoded = encode_attachment_base64(content);
        let attachment =
            add_task_attachment(conn, &data_dir, &task.id, "../../报告 v1.TXT", &encoded).unwrap();
        assert_eq!(attachment.task_id, task.id);
        assert_eq!(attachment.file_name, "报告 v1.TXT");
        assert_eq!(attachment.mime, "text/plain");
        assert_eq!(attachment.size, content.len() as i64);
        assert!(!attachment.created_at.is_empty());

        let rel_path: String = conn
            .query_row(
                "SELECT rel_path FROM task_attachments WHERE id = ?1",
                params![attachment.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            rel_path.starts_with(&format!("attachments/{}/", task.id)),
            "{rel_path}"
        );
        assert!(rel_path.ends_with("-报告-v1.txt"), "{rel_path}");
        assert!(!rel_path.contains(".."), "{rel_path}");
        let stored = data_dir.join(&rel_path);
        assert_eq!(fs::read(&stored).unwrap(), content);

        let listed = list_task_attachments(conn, &task.id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, attachment.id);
        assert!(list_task_attachments(conn, "missing").unwrap().is_empty());

        assert_eq!(
            read_task_attachment(conn, &data_dir, &attachment.id).unwrap(),
            encoded
        );
        assert!(read_task_attachment(conn, &data_dir, "missing").is_err());

        delete_task_attachment(conn, &data_dir, &attachment.id).unwrap();
        assert!(!stored.exists(), "删除记录时应同时删除文件");
        assert!(list_task_attachments(conn, &task.id).unwrap().is_empty());
        assert!(
            delete_task_attachment(conn, &data_dir, &attachment.id).is_err(),
            "重复删除应报附件不存在"
        );

        assert!(
            add_task_attachment(conn, &data_dir, "missing", "a.txt", &encoded).is_err(),
            "未知任务不能上传附件"
        );
    }

    #[test]
    fn task_attachment_sanitizes_names_and_rejects_oversize_and_traversal() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let data_dir = temp_dir();
        let encoded = encode_attachment_base64(b"x");

        let cleaned = add_task_attachment(
            conn,
            &data_dir,
            &task.id,
            "..\\..\\evil name?.PNG",
            &encoded,
        )
        .unwrap();
        assert_eq!(cleaned.file_name, "evil name?.PNG");
        assert_eq!(cleaned.mime, "image/png");
        let rel_path: String = conn
            .query_row(
                "SELECT rel_path FROM task_attachments WHERE id = ?1",
                params![cleaned.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            rel_path.starts_with(&format!("attachments/{}/", task.id)),
            "{rel_path}"
        );
        assert!(rel_path.ends_with("-evil-name.png"), "{rel_path}");
        assert!(
            !rel_path.contains("..") && !rel_path.contains('\\'),
            "{rel_path}"
        );

        let fallback = add_task_attachment(conn, &data_dir, &task.id, "..", &encoded).unwrap();
        assert_eq!(fallback.file_name, "file");
        assert_eq!(fallback.mime, "application/octet-stream");

        let err = add_task_attachment(conn, &data_dir, &task.id, "   ", &encoded).unwrap_err();
        assert!(err.contains("文件名"), "{err}");
        let err = add_task_attachment(conn, &data_dir, &task.id, "a.txt", "   ").unwrap_err();
        assert!(err.contains("为空"), "{err}");
        assert!(add_task_attachment(conn, &data_dir, &task.id, "a.txt", "***").is_err());

        let big = vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1];
        let err = add_task_attachment(
            conn,
            &data_dir,
            &task.id,
            "big.bin",
            &encode_attachment_base64(&big),
        )
        .unwrap_err();
        assert!(err.contains("20 MB"), "{err}");
        assert_eq!(
            list_task_attachments(conn, &task.id).unwrap().len(),
            2,
            "失败的写入不应留下记录"
        );

        // 手工篡改 rel_path 模拟越界：读取拒绝，删除不触碰越界文件。
        let outside = data_dir.join("outside.txt");
        fs::write(&outside, b"outside").unwrap();
        for rel_path in [
            "../outside.txt",
            "attachments/../outside.txt",
            "/etc/passwd",
        ] {
            conn.execute(
                "UPDATE task_attachments SET rel_path = ?2 WHERE id = ?1",
                params![cleaned.id, rel_path],
            )
            .unwrap();
            assert!(
                read_task_attachment(conn, &data_dir, &cleaned.id).is_err(),
                "越界路径必须拒绝：{rel_path}"
            );
        }
        conn.execute(
            "UPDATE task_attachments SET rel_path = '../outside.txt' WHERE id = ?1",
            params![cleaned.id],
        )
        .unwrap();
        delete_task_attachment(conn, &data_dir, &cleaned.id).unwrap();
        assert!(outside.exists(), "越界路径的文件不能被删除");
        assert_eq!(
            list_task_attachments(conn, &task.id).unwrap().len(),
            1,
            "删除只移除目标记录"
        );
    }
}
