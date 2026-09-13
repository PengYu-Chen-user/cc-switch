//! DeepSeek Harness (`dsh`, `$DSH_HOME` or `~/.dsh`) live configuration support.
//!
//! DSH keeps two hot-reloaded documents under the harness home:
//!
//! - `settings.yaml` — user settings. cc-switch writes the provider route under
//!   `llm-pi-ai.providers.<id>` and selects it via `agent-default-model`.
//! - `.credentials.yaml` — credential store. The API key is written into
//!   `refs.<apiKeyEnv>`; `settings.yaml` only carries the reference name.
//!
//! cc-switch models DSH with a **switch-mode** provider whose `settings_config`
//! carries the structured fields below:
//!
//! ```json
//! {
//!   "providerId": "moonshotai",
//!   "displayName": "Moonshot AI (Kimi)",
//!   "api": "openai-completions",
//!   "baseURL": "https://api.moonshot.ai/v1",
//!   "apiKeyEnv": "MOONSHOT_API_KEY",
//!   "apiKey": "sk-...",
//!   "models": [{ "id": "kimi-k2", "name": "Kimi K2", "contextWindow": 262144 }],
//!   "defaultModel": "kimi-k2"
//! }
//! ```

use serde_yaml::{Mapping, Value as YamlValue};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::config::{get_home_dir, write_text_file};
use crate::error::AppError;
use crate::provider::Provider;

/// DeepSeek Harness home directory (`$DSH_HOME` or `~/.dsh`).
pub fn get_dsh_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_dsh_override_dir() {
        return custom;
    }
    if let Some(home) = std::env::var_os("DSH_HOME") {
        let raw = home.to_string_lossy();
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    get_home_dir().join(".dsh")
}

/// DSH settings document (`<dir>/settings.yaml`).
pub fn get_dsh_settings_path() -> PathBuf {
    get_dsh_dir().join("settings.yaml")
}

/// DSH credential store (`<dir>/.credentials.yaml`).
pub fn get_dsh_credentials_path() -> PathBuf {
    get_dsh_dir().join(".credentials.yaml")
}

fn ykey(key: &str) -> YamlValue {
    YamlValue::String(key.to_string())
}

fn as_non_empty_str(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

fn mapping_mut<'a>(parent: &'a mut Mapping, key: &str) -> &'a mut Mapping {
    if !parent.contains_key(ykey(key)) {
        parent.insert(ykey(key), YamlValue::Mapping(Mapping::new()));
    }
    parent
        .get_mut(ykey(key))
        .and_then(YamlValue::as_mapping_mut)
        .expect("nested value must be a mapping")
}

fn read_yaml_mapping(path: &std::path::Path, label: &str) -> Result<YamlValue, AppError> {
    let text = fs::read_to_string(path).map_err(|error| AppError::io(path, error))?;
    if text.trim().is_empty() {
        return Ok(YamlValue::Mapping(Mapping::new()));
    }
    let value: YamlValue = serde_yaml::from_str(&text).map_err(|error| {
        AppError::localized(
            "provider.dsh.config.invalid_yaml",
            format!("DeepSeek Harness {label} 格式错误: {error}"),
            format!("Invalid DeepSeek Harness {label}: {error}"),
        )
    })?;
    if !value.is_mapping() {
        return Err(AppError::localized(
            "provider.dsh.config.not_mapping",
            format!("DeepSeek Harness {label} 必须是 YAML 映射"),
            format!("DeepSeek Harness {label} must be a YAML mapping"),
        ));
    }
    Ok(value)
}

/// Validate the provider-owned structured DSH settings object.
pub fn validate_dsh_settings(settings: &Value) -> Result<(), AppError> {
    let object = settings.as_object().ok_or_else(|| {
        AppError::localized(
            "provider.dsh.settings.not_object",
            "DeepSeek Harness 配置必须是 JSON 对象",
            "DeepSeek Harness configuration must be a JSON object",
        )
    })?;
    for key in ["providerId", "defaultModel"] {
        if as_non_empty_str(object.get(key)).is_empty() {
            return Err(AppError::localized(
                "provider.dsh.field.missing",
                format!("DeepSeek Harness 配置缺少有效的 {key} 字段"),
                format!("DeepSeek Harness configuration is missing a valid {key} field"),
            ));
        }
    }
    Ok(())
}

fn read_credential_ref(api_key_env: &str) -> String {
    if api_key_env.is_empty() {
        return String::new();
    }
    let path = get_dsh_credentials_path();
    let Ok(value) = read_yaml_mapping(&path, ".credentials.yaml") else {
        return String::new();
    };
    value
        .get("refs")
        .and_then(YamlValue::as_mapping)
        .and_then(|refs| refs.get(ykey(api_key_env)))
        .and_then(YamlValue::as_str)
        .unwrap_or("")
        .to_string()
}

/// Read the live `settings.yaml` (+ credential ref) as a provider settings snapshot.
pub fn read_dsh_live_settings() -> Result<Value, AppError> {
    let path = get_dsh_settings_path();
    if !path.exists() {
        return Err(AppError::localized(
            "dsh.config.missing",
            "DeepSeek Harness 配置文件不存在",
            "DeepSeek Harness configuration file not found",
        ));
    }

    let value = read_yaml_mapping(&path, "settings.yaml")?;
    let provider_id = value
        .get("agent-default-model")
        .and_then(YamlValue::as_mapping)
        .and_then(|map| map.get(ykey("provider")))
        .and_then(YamlValue::as_str)
        .map(str::trim)
        .unwrap_or("");
    if provider_id.is_empty() {
        return Err(AppError::localized(
            "provider.dsh.default_model.missing",
            "DeepSeek Harness 配置缺少 agent-default-model.provider",
            "DeepSeek Harness configuration is missing agent-default-model.provider",
        ));
    }

    let default_model = value
        .get("agent-default-model")
        .and_then(YamlValue::as_mapping)
        .and_then(|map| map.get(ykey("model")))
        .and_then(YamlValue::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    let route = value
        .get("llm-pi-ai")
        .and_then(YamlValue::as_mapping)
        .and_then(|map| map.get(ykey("providers")))
        .and_then(YamlValue::as_mapping)
        .and_then(|providers| providers.get(ykey(provider_id)))
        .and_then(YamlValue::as_mapping);

    let api = route
        .and_then(|route| route.get(ykey("api")))
        .and_then(YamlValue::as_str)
        .unwrap_or("")
        .to_string();
    let base_url = route
        .and_then(|route| route.get(ykey("baseURL")))
        .and_then(YamlValue::as_str)
        .unwrap_or("")
        .to_string();
    let display_name = route
        .and_then(|route| route.get(ykey("displayName")))
        .and_then(YamlValue::as_str)
        .unwrap_or("")
        .to_string();
    let api_key_env = route
        .and_then(|route| route.get(ykey("apiKeyEnv")))
        .and_then(YamlValue::as_str)
        .unwrap_or("")
        .to_string();

    let models: Value = route
        .and_then(|route| route.get(ykey("models")))
        .map(|models| serde_json::to_value(models).unwrap_or(Value::Array(Vec::new())))
        .unwrap_or(Value::Array(Vec::new()));

    let api_key = read_credential_ref(&api_key_env);

    Ok(json!({
        "providerId": provider_id,
        "displayName": display_name,
        "api": api,
        "baseURL": base_url,
        "apiKeyEnv": api_key_env,
        "apiKey": api_key,
        "models": models,
        "defaultModel": default_model,
    }))
}

/// Write a provider route into `settings.yaml` and its key into `.credentials.yaml`.
pub fn write_dsh_provider_live(provider: &Provider) -> Result<(), AppError> {
    validate_dsh_settings(&provider.settings_config)?;
    let object = provider
        .settings_config
        .as_object()
        .expect("validated as object");

    let provider_id = as_non_empty_str(object.get("providerId"));
    let display_name = as_non_empty_str(object.get("displayName"));
    let api = as_non_empty_str(object.get("api"));
    let base_url = as_non_empty_str(object.get("baseURL"));
    let api_key_env = as_non_empty_str(object.get("apiKeyEnv"));
    let api_key = as_non_empty_str(object.get("apiKey"));
    let default_model = as_non_empty_str(object.get("defaultModel"));
    let models = object
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // ── settings.yaml ──
    let settings_path = get_dsh_settings_path();
    let mut root = if settings_path.exists() {
        read_yaml_mapping(&settings_path, "settings.yaml")?
    } else {
        YamlValue::Mapping(Mapping::new())
    };

    {
        let root_map = root
            .as_mapping_mut()
            .expect("settings root must be a mapping");
        let llm = mapping_mut(root_map, "llm-pi-ai");
        let providers = mapping_mut(llm, "providers");
        let route = mapping_mut(providers, &provider_id);

        if !display_name.is_empty() {
            route.insert(ykey("displayName"), YamlValue::String(display_name.clone()));
        }
        if !api.is_empty() {
            route.insert(ykey("api"), YamlValue::String(api.clone()));
        }
        if !base_url.is_empty() {
            route.insert(ykey("baseURL"), YamlValue::String(base_url.clone()));
        }
        if !api_key_env.is_empty() {
            route.insert(ykey("apiKeyEnv"), YamlValue::String(api_key_env.clone()));
        }
        if !models.is_empty() {
            let models_value = serde_yaml::to_value(&Value::Array(models.clone())).map_err(
                |error| {
                    AppError::localized(
                        "provider.dsh.models.invalid",
                        format!("DeepSeek Harness 模型列表无法序列化: {error}"),
                        format!("DeepSeek Harness models could not be serialized: {error}"),
                    )
                },
            )?;
            route.insert(ykey("models"), models_value);
        }
    }

    {
        let root_map = root
            .as_mapping_mut()
            .expect("settings root must be a mapping");
        let default_entry = mapping_mut(root_map, "agent-default-model");
        default_entry.insert(ykey("provider"), YamlValue::String(provider_id.clone()));
        default_entry.insert(ykey("model"), YamlValue::String(default_model.clone()));
    }

    let settings_text = serde_yaml::to_string(&root).map_err(|error| {
        AppError::localized(
            "provider.dsh.config.serialize_failed",
            format!("DeepSeek Harness settings.yaml 序列化失败: {error}"),
            format!("Failed to serialize DeepSeek Harness settings.yaml: {error}"),
        )
    })?;
    write_text_file(&settings_path, &settings_text)?;

    // ── .credentials.yaml ──
    if !api_key.is_empty() && !api_key_env.is_empty() {
        write_dsh_credential(&api_key_env, &api_key)?;
    }

    Ok(())
}

fn write_dsh_credential(api_key_env: &str, api_key: &str) -> Result<(), AppError> {
    let path = get_dsh_credentials_path();
    let mut root = if path.exists() {
        read_yaml_mapping(&path, ".credentials.yaml")?
    } else {
        YamlValue::Mapping(Mapping::new())
    };

    {
        let map = root.as_mapping_mut().expect("root mapping");
        if !map.contains_key(ykey("version")) {
            map.insert(ykey("version"), YamlValue::Number(1u64.into()));
        }
    }
    let refs = mapping_mut(root.as_mapping_mut().expect("credentials root"), "refs");
    refs.insert(
        ykey(api_key_env),
        YamlValue::String(api_key.to_string()),
    );

    let text = serde_yaml::to_string(&root).map_err(|error| {
        AppError::localized(
            "provider.dsh.credentials.serialize_failed",
            format!("DeepSeek Harness .credentials.yaml 序列化失败: {error}"),
            format!("Failed to serialize DeepSeek Harness .credentials.yaml: {error}"),
        )
    })?;
    crate::config::atomic_write_private(&path, text.as_bytes())
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
            "providerId": "moonshotai",
            "displayName": "Moonshot AI (Kimi)",
            "api": "openai-completions",
            "baseURL": "https://api.moonshot.ai/v1",
            "apiKeyEnv": "MOONSHOT_API_KEY",
            "apiKey": "sk-test",
            "models": [{ "id": "kimi-k2", "name": "Kimi K2", "contextWindow": 262144 }],
            "defaultModel": "kimi-k2"
        })
    }

    #[test]
    #[serial]
    fn writes_then_reads_roundtrip() {
        with_temp_home(|| {
            let provider = Provider::with_id(
                "moonshot".to_string(),
                "Moonshot".to_string(),
                sample_settings(),
                None,
            );
            write_dsh_provider_live(&provider).expect("write live config");

            let settings_text =
                fs::read_to_string(get_dsh_settings_path()).expect("settings.yaml written");
            assert!(settings_text.contains("agent-default-model"));
            assert!(settings_text.contains("moonshotai"));
            let creds_text =
                fs::read_to_string(get_dsh_credentials_path()).expect(".credentials.yaml written");
            assert!(creds_text.contains("MOONSHOT_API_KEY"));
            assert!(creds_text.contains("sk-test"));

            let read_back = read_dsh_live_settings().expect("read back");
            assert_eq!(read_back["providerId"], "moonshotai");
            assert_eq!(read_back["defaultModel"], "kimi-k2");
            assert_eq!(read_back["apiKey"], "sk-test");
            assert_eq!(read_back["baseURL"], "https://api.moonshot.ai/v1");
        });
    }

    #[test]
    #[serial]
    fn preserves_unrelated_settings_namespaces() {
        with_temp_home(|| {
            let path = get_dsh_settings_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "web-search-deepseek:\n  model: qwen3\n").unwrap();

            let provider = Provider::with_id(
                "moonshot".to_string(),
                "Moonshot".to_string(),
                sample_settings(),
                None,
            );
            write_dsh_provider_live(&provider).expect("write live config");

            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains("web-search-deepseek"), "unrelated namespace preserved");
            let read_back = read_dsh_live_settings().expect("read back");
            assert_eq!(read_back["providerId"], "moonshotai");
        });
    }

    #[test]
    #[serial]
    fn rejects_settings_without_required_fields() {
        with_temp_home(|| {
            let bad = json!({ "providerId": "", "defaultModel": "" });
            assert!(validate_dsh_settings(&bad).is_err());
        });
    }
}
