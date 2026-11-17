//! The HTTP MCP servers the app offers in its catalog.
//!
//! A catalog server is an established service the app vouches for, so its read-only hints are
//! believed. A server the user added by hand can label any tool read-only, and can put
//! instructions for the model in its descriptions.

use super::config::{McpServerConfig, McpTransport};
use std::sync::LazyLock;

/// Read from the catalog the settings page and the web app are built from, so the two can't
/// drift apart.
static CATALOG_URLS: LazyLock<Vec<String>> = LazyLock::new(|| {
    include_str!("../../../../../webview/shared/connector-catalog.ts")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("url: '"))
        .filter_map(|rest| rest.split('\'').next())
        .map(normalize)
        .collect()
});

fn normalize(url: &str) -> String {
    url.trim_end_matches('/').to_ascii_lowercase()
}

/// Whether `config` is a catalog entry: an HTTP server at exactly a catalog URL. Stdio servers
/// and URLs the user typed in count as added by hand.
pub(crate) fn is_catalog_server(config: &McpServerConfig) -> bool {
    match &config.transport {
        McpTransport::Http { url, .. } => CATALOG_URLS.contains(&normalize(url)),
        McpTransport::Stdio { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_read_from_the_shared_list() {
        assert!(CATALOG_URLS.len() >= 20, "{:?}", *CATALOG_URLS);
        assert!(CATALOG_URLS.contains(&"https://mcp.slack.com/mcp".to_string()));
        assert!(CATALOG_URLS.iter().all(|url| url.starts_with("http")));
    }
}
