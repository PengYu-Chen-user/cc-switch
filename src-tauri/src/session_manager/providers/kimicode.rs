//! Kimi Code CLI session scanning/loading.
//!
//! Layout: `$KIMI_CODE_HOME/sessions/<workDirKey>/<sessionId>/`
//! - `state.json`: session metadata (title/lastPrompt/timestamps)
//! - `agents/main/wire.jsonl`: main-agent message log (JSONL)

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session_manager::{SessionMessage, SessionMeta};

use super::utils::{extract_text, parse_timestamp_to_ms, truncate_summary, TITLE_MAX_CHARS};

const PROVIDER_ID: &str = "kimicode";

pub fn session_roots() -> Vec<PathBuf> {
    vec![crate::kimicode_config::get_kimicode_dir().join("sessions")]
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let root = crate::kimicode_config::get_kimicode_dir().join("sessions");
    if !root.is_dir() {
        return Vec::new();
    }

    let mut sessions = Vec::new();

    // sessions/<workDirKey>/<sessionId>/
    for work_dir_entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let work_dir = work_dir_entry.path();
        if !work_dir.is_dir() {
            continue;
        }
        for session_entry in std::fs::read_dir(&work_dir).into_iter().flatten().flatten() {
            let session_dir = session_entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            let wire = session_dir.join("agents").join("main").join("wire.jsonl");
            let state = session_dir.join("state.json");
            if !wire.exists() && !state.exists() {
                continue;
            }
            if let Some(meta) = build_session_meta(&session_dir, &state, &wire) {
                sessions.push(meta);
            }
        }
    }

    sessions
}

fn build_session_meta(
    session_dir: &Path,
    state_path: &Path,
    wire_path: &Path,
) -> Option<SessionMeta> {
    let session_id = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();

    let state = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());

    let title = state
        .as_ref()
        .and_then(|value| {
            value
                .get("title")
                .or_else(|| value.get("lastPrompt"))
                .and_then(Value::as_str)
        })
        .filter(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, TITLE_MAX_CHARS));

    let created_at = state
        .as_ref()
        .and_then(|value| {
            value
                .get("createdAt")
                .or_else(|| value.get("created_at"))
                .or_else(|| value.get("created"))
        })
        .and_then(parse_timestamp_to_ms);

    let last_active_at = state
        .as_ref()
        .and_then(|value| {
            value
                .get("updatedAt")
                .or_else(|| value.get("updated_at"))
                .or_else(|| value.get("lastActiveAt"))
        })
        .and_then(parse_timestamp_to_ms)
        .or_else(|| file_mtime_ms(wire_path))
        .or_else(|| file_mtime_ms(state_path));

    let source_path = if wire_path.exists() {
        wire_path.to_string_lossy().to_string()
    } else {
        state_path.to_string_lossy().to_string()
    };

    Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: title.clone(),
        summary: title,
        project_dir: None,
        created_at: created_at.or(last_active_at),
        last_active_at,
        source_path: Some(source_path),
        resume_command: Some(format!("kimi --session {session_id}")),
    })
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    if !path.exists() {
        return Err(format!("Session file not found: {}", path.display()));
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read session: {e}"))?;

    let mut messages = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(message) = message_from_wire(&value) {
            messages.push(message);
        }
    }
    Ok(messages)
}

fn message_from_wire(value: &Value) -> Option<SessionMessage> {
    let role_raw = value
        .get("role")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)?;

    let role = match role_raw {
        "user" | "human" => "user",
        "assistant" | "agent" | "model" | "ai" => "assistant",
        "tool" | "tool_result" => "tool",
        _ => return None,
    };

    let content_value = value
        .get("content")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("text"))
        .or_else(|| value.get("parts"));

    let content = content_value
        .map(|value| extract_text(value))
        .unwrap_or_default();

    if content.trim().is_empty() {
        return None;
    }

    let ts = value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .or_else(|| value.get("created_at"))
        .and_then(parse_timestamp_to_ms);

    Some(SessionMessage {
        role: role.to_string(),
        content,
        ts,
    })
}

pub fn delete_session(_root: &Path, path: &Path, session_id: &str) -> Result<bool, String> {
    // Prefer deleting the whole session directory when the path is inside it.
    let session_dir = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent());

    if let Some(dir) = session_dir {
        if dir.is_dir() {
            if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                if name == session_id {
                    std::fs::remove_dir_all(dir)
                        .map_err(|e| format!("Failed to delete session dir: {e}"))?;
                    return Ok(true);
                }
            }
        }
    }

    std::fs::remove_file(path).map_err(|e| format!("Failed to delete session file: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn loads_wire_jsonl_messages() {
        let temp = tempdir().expect("tempdir");
        let wire = temp.path().join("wire.jsonl");
        std::fs::write(
            &wire,
            "{\"role\":\"user\",\"content\":\"hello\",\"timestamp\":1771061953000}\n\
             {\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"world\"}]}\n",
        )
        .expect("write");

        let messages = load_messages(&wire).expect("load");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "world");
    }
}
