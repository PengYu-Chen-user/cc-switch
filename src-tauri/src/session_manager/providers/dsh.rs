//! DeepSeek Harness (`dsh`) session scanning/loading.
//!
//! Harness sessions live under `$DSH_HOME/sessions/` as append-only JSONL logs,
//! optionally compressed with checksummed Zstandard frames. This module is
//! deliberately tolerant about the surrounding directory layout (the developer
//! preview iterates quickly) and about the per-message schema.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session_manager::{SessionMessage, SessionMeta};

use super::utils::{extract_text, parse_timestamp_to_ms, truncate_summary, TITLE_MAX_CHARS};

const PROVIDER_ID: &str = "dsh";
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
const MAX_DEPTH: usize = 5;

pub fn session_roots() -> Vec<PathBuf> {
    vec![crate::dsh_config::get_dsh_dir().join("sessions")]
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let root = crate::dsh_config::get_dsh_dir().join("sessions");
    if !root.is_dir() {
        return Vec::new();
    }

    let mut sessions = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    walk(&root, 0, &mut |path| {
        if !looks_like_session_log(path) {
            return;
        }
        let session_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(|name| name.trim_end_matches(".jsonl").trim_end_matches(".json"))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                path.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "session".to_string());

        let key = format!("{session_id}:{}", path.display());
        if !seen_ids.insert(key) {
            return;
        }

        let mtime = file_mtime_ms(path);
        sessions.push(SessionMeta {
            provider_id: PROVIDER_ID.to_string(),
            session_id: session_id.clone(),
            title: None,
            summary: parent_project_dir(path).map(|dir| truncate_summary(&dir, TITLE_MAX_CHARS)),
            project_dir: parent_project_dir(path),
            created_at: mtime,
            last_active_at: mtime,
            source_path: Some(path.to_string_lossy().to_string()),
            resume_command: Some(format!("dsh --resume {session_id}")),
        });
    });

    sessions
}

fn walk(dir: &Path, depth: usize, visit: &mut impl FnMut(&Path)) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, visit);
        } else {
            visit(&path);
        }
    }
}

fn looks_like_session_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower.starts_with('.') {
        return false;
    }
    lower.contains(".jsonl") || lower.ends_with(".json") || lower.ends_with(".zst")
}

fn parent_project_dir(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

fn read_maybe_zstd(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read session: {e}"))?;

    let decompressed = if bytes.starts_with(&ZSTD_MAGIC) {
        let mut decoder = zstd::stream::Decoder::new(std::io::Cursor::new(&bytes))
            .map_err(|e| format!("Failed to open zstd session: {e}"))?;
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| format!("Failed to decompress zstd session: {e}"))?;
        out
    } else {
        bytes
    };

    Ok(String::from_utf8_lossy(&decompressed).into_owned())
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    if !path.exists() {
        return Err(format!("Session file not found: {}", path.display()));
    }
    let text = read_maybe_zstd(path)?;

    let mut messages = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(message) = message_from_log(&value) {
            messages.push(message);
        }
    }
    Ok(messages)
}

fn message_from_log(value: &Value) -> Option<SessionMessage> {
    // Session creation header / projections are not conversation messages.
    let role_raw = value
        .get("role")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)?;

    let role = match role_raw {
        "user" | "human" => "user",
        "assistant" | "agent" | "model" | "ai" | "message" => "assistant",
        "tool" | "tool_result" | "tool_call" => "tool",
        _ => return None,
    };

    // `message`-typed frames may nest the payload.
    let payload = value.get("message").unwrap_or(value);
    let content_value = payload
        .get("content")
        .or_else(|| payload.get("text"))
        .or_else(|| payload.get("parts"));

    let content = content_value.map(extract_text).unwrap_or_default();
    if content.trim().is_empty() {
        return None;
    }

    let ts = value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .or_else(|| value.get("created_at"))
        .or_else(|| value.get("time"))
        .and_then(parse_timestamp_to_ms);

    Some(SessionMessage {
        role: role.to_string(),
        content,
        ts,
    })
}

pub fn delete_session(_root: &Path, path: &Path, _session_id: &str) -> Result<bool, String> {
    std::fs::remove_file(path).map_err(|e| format!("Failed to delete session file: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn loads_plain_jsonl_messages() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"content\":\"hi\",\"ts\":1771061953000}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"yo\"}]}\n\
             {\"type\":\"session_meta\",\"id\":\"abc\"}\n",
        )
        .expect("write");

        let messages = load_messages(&path).expect("load");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hi");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "yo");
    }

    #[test]
    fn loads_zstd_jsonl_messages() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl.zst");
        let raw = "{\"type\":\"user\",\"content\":\"compressed hi\"}\n";
        let compressed =
            zstd::stream::encode_all(std::io::Cursor::new(raw.as_bytes()), 0).expect("compress");
        std::fs::write(&path, compressed).expect("write");

        let messages = load_messages(&path).expect("load");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "compressed hi");
    }
}
