use crate::account::AccountId;
use crate::reporting::config_failure;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::AppHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    #[default]
    AutoApprove,
    Ask,
    Disable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectorState {
    #[serde(default)]
    pub disabled_connectors: HashSet<String>,
    /// connector_id -> tool_name -> policy
    #[serde(default)]
    pub tool_policies: HashMap<String, HashMap<String, ToolPolicy>>,
    /// connector_id -> tool_name -> [`fingerprint`](super::registry::ToolDefinition::fingerprint)
    /// of an MCP tool as it was when the user chose to auto-approve it. The choice lapses back
    /// to asking when the server changes what it says the tool does.
    #[serde(default)]
    pub approved_tools: HashMap<String, HashMap<String, String>>,
    /// connector_id -> tool_name -> fingerprint (empty for built-in tools): tools the user lets
    /// run even after the conversation has read untrusted content, scheduled runs included.
    /// Like an approval, it lapses when an MCP server changes the tool.
    #[serde(default)]
    pub always_allowed: HashMap<String, HashMap<String, String>>,
    /// Connectors that have reached their remote at least once.
    ///
    /// Outlives the app, unlike the per-run activity: it separates a connector that is
    /// temporarily broken from one that has never functioned.
    #[serde(default)]
    pub succeeded_once: HashSet<String>,
    /// Conversations that have read private data or untrusted content, by conversation id.
    /// Kept so a conversation resumed after a restart is still gated; see `tools::exposure`.
    #[serde(default)]
    pub exposed_conversations: HashMap<String, super::exposure::Exposure>,
}

pub fn connector_state_path(
    app_handle: &AppHandle,
    account: &AccountId,
) -> Result<std::path::PathBuf, String> {
    crate::settings::account_config_dir(app_handle, account)
        .map(|dir| dir.join("connector_state.json"))
}

pub fn load_connector_state(
    app_handle: &AppHandle,
    account: &AccountId,
) -> Result<ConnectorState, String> {
    let path = connector_state_path(app_handle, account)?;
    if !path.exists() {
        return Ok(ConnectorState::default());
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content)
        .inspect_err(|_| config_failure("Connector state load failed"))
        .map_err(|e| e.to_string())
}

pub fn save_connector_state(
    app_handle: &AppHandle,
    account: &AccountId,
    state: &ConnectorState,
) -> Result<(), String> {
    let path = connector_state_path(app_handle, account)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(state)
        .inspect_err(|_| config_failure("Connector state save failed"))
        .map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())
}
