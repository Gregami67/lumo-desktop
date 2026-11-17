use crate::account::AccountId;
use crate::tools::registry::ToolDefinition;
use log::warn;
use tauri::AppHandle;

/// Keeps a server's discovered tools between launches, so one with no live session still
/// advertises what it offers.
pub trait ToolCache: Send + Sync {
    fn load(&self) -> Vec<ToolDefinition>;
    fn save(&self, tools: &[ToolDefinition]);
}

pub struct AccountToolCache {
    pub(super) app_handle: AppHandle,
    pub(super) account: AccountId,
    pub(super) server_id: String,
}

impl AccountToolCache {
    fn path(&self) -> Result<std::path::PathBuf, String> {
        mcp_tools_path(&self.app_handle, &self.account, &self.server_id)
    }
}

pub fn mcp_tools_path(
    app_handle: &AppHandle,
    account: &AccountId,
    server_id: &str,
) -> Result<std::path::PathBuf, String> {
    super::validate_server_id(server_id)?;
    crate::settings::account_config_dir(app_handle, account)
        .map(|dir| dir.join("mcp_tools").join(format!("{server_id}.json")))
}

impl ToolCache for AccountToolCache {
    fn load(&self) -> Vec<ToolDefinition> {
        let Ok(path) = self.path() else {
            return Vec::new();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    fn save(&self, tools: &[ToolDefinition]) {
        let result = self.path().and_then(|path| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let content = serde_json::to_string_pretty(tools).map_err(|e| e.to_string())?;
            std::fs::write(path, content).map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            warn!("failed to cache MCP tools: {e}");
        }
    }
}

pub struct NoToolCache;

impl ToolCache for NoToolCache {
    fn load(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    fn save(&self, _tools: &[ToolDefinition]) {}
}
