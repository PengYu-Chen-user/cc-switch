//! Kimi Code CLI (~/.kimi-code) live configuration support.
//!
//! Kimi Code CLI stores its runtime configuration in `config.toml` under the
//! data directory (`KIMI_CODE_HOME`, defaulting to `~/.kimi-code`). Providers
//! live in `[providers.<name>]` and models in `[models."<alias>"]`, with the
//! top-level `default_model` selecting the active alias. Credentials must be
//! written into the file itself — shell environment variables are ignored.
//!
//! cc-switch models Kimi with a **switch-mode** provider whose `settings_config`
//! carries the structured fields below. Writing a provider merges into the
//! existing document (other top-level sections such as `thinking`,
//! `loop_control` and any OAuth-managed providers are preserved):
//!
//! ```json
//! {
//!   "providerKey": "moonshot",
//!   "type": "kimi",
//!   "baseUrl": "https://api.moonshot.ai/v1",
//!   "apiKey": "sk-...",
//!   "modelAlias": "kimi-code/kimi-for-coding",
//!   "modelId": "kimi-for-coding",
//!   "maxContextSize": 262144
//! }
//! ```

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use toml_edit::{value as toml_value, DocumentMut, Item, Table};

use crate::config::{get_home_dir, write_text_file};
use crate::error::AppError;
use crate::provider::Provider;

pub const DEFAULT_MAX_CONTEXT_SIZE: i64 = 262_144;

/// Kimi Code data directory (`$KIMI_CODE_HOME` or `~/.kimi-code`).
pub fn get_kimicode_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_kimicode_override_dir() {
        return custom;
    }
    if let Some(home) = std::env::var_os("KIMI_CODE_HOME") {
        let raw = home.to_string_lossy();
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    get_home_dir().join(".kimi-code")
}

/// Kimi Code live configuration path (`<dir>/config.toml`).
pub fn get_kimicode_config_path() -> PathBuf {
    get_kimicode_dir().join("config.toml")
}

/// Kimi Code user-level MCP servers file (`<dir>/mcp.json`).
pub fn get_kimicode_mcp_path() -> PathBuf {
    get_kimicode_dir().join("mcp.json")
}

fn as_non_empty_str(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// Validate the provider-owned structured Kimi Code settings object.
pub fn validate_kimicode_settings(settings: &Value) -> Result<(), AppError> {
    let object = settings.as_object().ok_or_else(|| {
        AppError::localized(
            "provider.kimicode.settings.not_object",
            "Kimi Code 配置必须是 JSON 对象",
            "Kimi Code configuration must be a JSON object",
        )
    })?;

    for key in ["providerKey", "type", "modelAlias", "modelId"] {
        if as_non_empty_str(object.get(key)).is_empty() {
            return Err(AppError::localized(
                "provider.kimicode.field.missing",
                format!("Kimi Code 配置缺少有效的 {key} 字段"),
                format!("Kimi Code configuration is missing a valid {key} field"),
            ));
        }
    }

    Ok(())
}

fn parse_toml(text: &str) -> Result<toml::Value, AppError> {
    text.parse::<toml::Value>().map_err(|error| {
        AppError::localized(
            "provider.kimicode.config.invalid_toml",
            format!("Kimi Code config.toml 格式错误: {error}"),
            format!("Invalid Kimi Code config.toml: {error}"),
        )
    })
}

/// Read the live `config.toml` as a provider settings snapshot.
pub fn read_kimicode_live_settings() -> Result<Value, AppError> {
    let path = get_kimicode_config_path();
    if !path.exists() {
        return Err(AppError::localized(
            "kimicode.config.missing",
            "Kimi Code 配置文件不存在",
            "Kimi Code configuration file not found",
        ));
    }

    let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    let document = parse_toml(&text)?;
    let root = document.as_table().ok_or_else(|| {
        AppError::localized(
            "provider.kimicode.config.not_table",
            "Kimi Code 配置必须是 TOML 表结构",
            "Kimi Code configuration must be a TOML table",
        )
    })?;

    let default_model = root
        .get("default_model")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if default_model.is_empty() {
        return Err(AppError::localized(
            "provider.kimicode.default_model.missing",
            "Kimi Code 配置缺少 default_model",
            "Kimi Code configuration is missing default_model",
        ));
    }

    let models = root.get("models").and_then(toml::Value::as_table);
    let model_entry = models
        .and_then(|models| models.get(default_model))
        .and_then(toml::Value::as_table);

    let provider_key = model_entry
        .and_then(|entry| entry.get("provider"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let model_id = model_entry
        .and_then(|entry| entry.get("model"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let max_context_size = model_entry
        .and_then(|entry| entry.get("max_context_size"))
        .and_then(toml::Value::as_integer)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE);

    let providers = root.get("providers").and_then(toml::Value::as_table);
    let provider_entry = providers
        .and_then(|providers| providers.get(provider_key))
        .and_then(toml::Value::as_table);

    let provider_type = provider_entry
        .and_then(|entry| entry.get("type"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("kimi");
    let base_url = provider_entry
        .and_then(|entry| entry.get("base_url"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let api_key = provider_entry
        .and_then(|entry| entry.get("api_key"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    Ok(json!({
        "providerKey": provider_key,
        "type": provider_type,
        "baseUrl": base_url,
        "apiKey": api_key,
        "modelAlias": default_model,
        "modelId": model_id,
        "maxContextSize": max_context_size,
    }))
}

fn ensure_table<'a>(document: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    let root = document.as_table_mut();
    if !root.contains_key(key) {
        root.insert(key, Item::Table(Table::new()));
    }
    root.get_mut(key)
        .and_then(Item::as_table_mut)
        .expect("table was just ensured")
}

/// Write a provider into the live `config.toml`, preserving unrelated sections.
pub fn write_kimicode_provider_live(provider: &Provider) -> Result<(), AppError> {
    validate_kimicode_settings(&provider.settings_config)?;
    let object = provider
        .settings_config
        .as_object()
        .expect("validated as object");

    let provider_key = as_non_empty_str(object.get("providerKey"));
    let provider_type = as_non_empty_str(object.get("type"));
    let base_url = as_non_empty_str(object.get("baseUrl"));
    let api_key = as_non_empty_str(object.get("apiKey"));
    let model_alias = as_non_empty_str(object.get("modelAlias"));
    let model_id = as_non_empty_str(object.get("modelId"));
    let max_context_size = object
        .get("maxContextSize")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE);

    let path = get_kimicode_config_path();
    let mut document = if path.exists() {
        let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
        text.parse::<DocumentMut>().map_err(|error| {
            AppError::localized(
                "provider.kimicode.config.invalid_toml",
                format!("Kimi Code config.toml 格式错误: {error}"),
                format!("Invalid Kimi Code config.toml: {error}"),
            )
        })?
    } else {
        DocumentMut::new()
    };

    {
        let providers = ensure_table(&mut document, "providers");
        let mut entry = Table::new();
        entry.insert("type", toml_value(&provider_type));
        if !base_url.is_empty() {
            entry.insert("base_url", toml_value(&base_url));
        }
        if !api_key.is_empty() {
            entry.insert("api_key", toml_value(&api_key));
        }
        providers.insert(&provider_key, Item::Table(entry));
    }

    {
        let models = ensure_table(&mut document, "models");
        let mut entry = Table::new();
        entry.insert("provider", toml_value(&provider_key));
        entry.insert("model", toml_value(&model_id));
        entry.insert("max_context_size", toml_value(max_context_size));
        models.insert(&model_alias, Item::Table(entry));
    }

    document
        .as_table_mut()
        .insert("default_model", toml_value(&model_alias));

    write_text_file(&path, &document.to_string())
}

/// Read the user-level MCP servers map from `mcp.json` (`mcpServers`).
pub fn read_mcp_servers_map() -> Result<serde_json::Map<String, Value>, AppError> {
    let path = get_kimicode_mcp_path();
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        AppError::localized(
            "provider.kimicode.mcp.invalid_json",
            format!("Kimi Code mcp.json 格式错误: {error}"),
            format!("Invalid Kimi Code mcp.json: {error}"),
        )
    })?;
    Ok(value
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

/// Write the user-level MCP servers map to `mcp.json`, preserving other keys.
pub fn set_mcp_servers_map(servers: &serde_json::Map<String, Value>) -> Result<(), AppError> {
    let path = get_kimicode_mcp_path();
    let mut root = if path.exists() {
        let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&text).map_err(|error| {
                AppError::localized(
                    "provider.kimicode.mcp.invalid_json",
                    format!("Kimi Code mcp.json 格式错误: {error}"),
                    format!("Invalid Kimi Code mcp.json: {error}"),
                )
            })?
        }
    } else {
        json!({})
    };

    let object = root.as_object_mut().ok_or_else(|| {
        AppError::localized(
            "provider.kimicode.mcp.not_object",
            "Kimi Code mcp.json 必须是 JSON 对象",
            "Kimi Code mcp.json must be a JSON object",
        )
    })?;
    object.insert("mcpServers".to_string(), Value::Object(servers.clone()));

    write_text_file(
        &path,
        &serde_json::to_string_pretty(&root).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let temp = TempDir::new().expect("temp dir");
        let original = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        f();
        match original {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    fn sample_settings() -> Value {
        json!({
            "providerKey": "moonshot",
            "type": "kimi",
            "baseUrl": "https://api.moonshot.ai/v1",
            "apiKey": "sk-test",
            "modelAlias": "kimi-code/kimi-for-coding",
            "modelId": "kimi-for-coding",
            "maxContextSize": 262144
        })
    }

    #[test]
    #[serial]
    fn writes_then_reads_roundtrip() {
        with_temp_home(|| {
            let provider = Provider::with_id(
                "kimi".to_string(),
                "Moonshot".to_string(),
                sample_settings(),
                None,
            );
            write_kimicode_provider_live(&provider).expect("write live config");

            let path = get_kimicode_config_path();
            assert!(path.exists());
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.parse::<toml::Value>().is_ok(),
                "output must be valid TOML: {text}"
            );

            let read_back = read_kimicode_live_settings().expect("read back");
            assert_eq!(read_back["providerKey"], "moonshot");
            assert_eq!(read_back["modelAlias"], "kimi-code/kimi-for-coding");
            assert_eq!(read_back["modelId"], "kimi-for-coding");
            assert_eq!(read_back["maxContextSize"], 262144);
        });
    }

    #[test]
    #[serial]
    fn preserves_unrelated_sections() {
        with_temp_home(|| {
            let path = get_kimicode_config_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                "[thinking]\nenabled = true\neffort = \"high\"\n\n[providers.managed]\ntype = \"kimi\"\napi_key = \"oauth\"\n",
            )
            .unwrap();

            let provider = Provider::with_id(
                "kimi".to_string(),
                "Moonshot".to_string(),
                sample_settings(),
                None,
            );
            write_kimicode_provider_live(&provider).expect("write live config");

            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("[thinking]"),
                "unrelated section preserved: {text}"
            );
            assert!(text.contains("managed"), "oauth provider preserved: {text}");
            let read_back = read_kimicode_live_settings().expect("read back");
            assert_eq!(read_back["providerKey"], "moonshot");
        });
    }

    #[test]
    #[serial]
    fn rejects_settings_without_required_fields() {
        with_temp_home(|| {
            let bad = json!({ "providerKey": "", "type": "kimi" });
            assert!(validate_kimicode_settings(&bad).is_err());
        });
    }
}
