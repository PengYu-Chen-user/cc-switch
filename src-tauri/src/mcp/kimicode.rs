//! Kimi Code CLI MCP 同步和导入模块
//!
//! Kimi Code CLI 在用户级 `~/.kimi-code/mcp.json` 中以 `mcpServers` 映射
//! 声明 MCP 服务器（与 Claude 的 `~/.claude.json` 形状接近）。stdio 服务器
//! 使用 `command`/`args`/`env`/`cwd`，HTTP 使用 `url`（可选 `transport: "sse"`
//! 与 `headers`）。

use serde_json::Value;
use std::collections::HashMap;

use crate::app_config::{McpApps, McpConfig, McpServer, MultiAppConfig};
use crate::error::AppError;

use super::validation::{extract_server_spec, validate_server_spec};

fn should_sync_kimicode_mcp() -> bool {
    // Kimi 未安装/未初始化时：目录与 mcp.json 都不存在，跳过写入/删除，
    // 不创建任何文件或目录。
    crate::kimicode_config::get_kimicode_dir().exists()
        || crate::kimicode_config::get_kimicode_mcp_path().exists()
}

/// 返回已启用的 MCP 服务器（过滤 enabled==true）
fn collect_enabled_servers(cfg: &McpConfig) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for (id, entry) in cfg.servers.iter() {
        let enabled = entry
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            continue;
        }
        match extract_server_spec(entry) {
            Ok(spec) => {
                out.insert(id.clone(), spec);
            }
            Err(err) => {
                log::warn!("跳过无效的 Kimi Code MCP 条目 '{id}': {err}");
            }
        }
    }
    out
}

/// 将 config.json 中 enabled==true 的项投影写入 `~/.kimi-code/mcp.json`。
#[allow(dead_code)]
pub fn sync_enabled_to_kimicode(config: &MultiAppConfig) -> Result<(), AppError> {
    if !should_sync_kimicode_mcp() {
        return Ok(());
    }
    let enabled = collect_enabled_servers(&config.mcp.claude);
    crate::kimicode_config::set_mcp_servers_map(&enabled)
}

/// 从 `~/.kimi-code/mcp.json` 导入 mcpServers 到统一结构。
pub fn import_from_kimicode(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let map = crate::kimicode_config::read_mcp_servers_map()?;
    if map.is_empty() {
        return Ok(0);
    }

    let servers = config.mcp.servers.get_or_insert_with(HashMap::new);
    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in map.iter() {
        if let Err(e) = validate_server_spec(spec) {
            log::warn!("跳过无效 Kimi Code MCP 服务器 '{id}': {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id) {
            if !existing.apps.kimicode {
                existing.apps.kimicode = true;
                changed += 1;
                log::info!("MCP 服务器 '{id}' 已启用 Kimi Code 应用");
            }
        } else {
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: spec.clone(),
                    apps: McpApps {
                        claude: false,
                        codex: false,
                        gemini: false,
                        grokbuild: false,
                        opencode: false,
                        hermes: false,
                        kimicode: true,
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
            log::info!("导入新 Kimi Code MCP 服务器 '{id}'");
        }
    }

    if !errors.is_empty() {
        log::warn!("Kimi Code MCP 导入完成，{} 项失败: {:?}", errors.len(), errors);
    }

    Ok(changed)
}

/// 将单个 MCP 服务器同步到 Kimi Code live 配置。
pub fn sync_single_server_to_kimicode(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !should_sync_kimicode_mcp() {
        return Ok(());
    }
    let mut current = crate::kimicode_config::read_mcp_servers_map()?;
    current.insert(id.to_string(), server_spec.clone());
    crate::kimicode_config::set_mcp_servers_map(&current)
}

/// 从 Kimi Code live 配置中移除单个 MCP 服务器。
pub fn remove_server_from_kimicode(id: &str) -> Result<(), AppError> {
    if !should_sync_kimicode_mcp() {
        return Ok(());
    }
    let mut current = crate::kimicode_config::read_mcp_servers_map()?;
    current.remove(id);
    crate::kimicode_config::set_mcp_servers_map(&current)
}
