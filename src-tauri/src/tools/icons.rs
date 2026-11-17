use base64::Engine;

macro_rules! connector_icon {
    ($file:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../webview/static/images/connectors/",
            $file
        ))
    };
}

fn bundled_icon_svg(connector_id: &str) -> Option<&'static [u8]> {
    let svg: &[u8] = match connector_id {
        "filesystem" => connector_icon!("filesystem.svg"),
        "browser" => connector_icon!("browser.svg"),
        "bash" => connector_icon!("bash.svg"),
        "mcp-chrome-devtools" => connector_icon!("chrome-devtools.svg"),
        "mcp-slack" => connector_icon!("slack.svg"),
        "mcp-atlassian" => connector_icon!("atlassian.svg"),
        "mcp-dropbox" => connector_icon!("dropbox.svg"),
        "mcp-google-drive" => connector_icon!("google-drive.svg"),
        "mcp-greenhouse" => connector_icon!("greenhouse.svg"),
        "mcp-figma" => connector_icon!("figma.svg"),
        "mcp-grafana" => connector_icon!("grafana.svg"),
        "mcp-zendesk" => connector_icon!("zendesk.svg"),
        "proton_mail" => connector_icon!("proton-mail.svg"),
        "luminance" => connector_icon!("luminance.svg"),
        _ => return None,
    };
    Some(svg)
}

/// Returns the bundled icon for a connector as a base64 `data:` URI, if one
/// exists.
pub fn bundled_icon_data_uri(connector_id: &str) -> Option<String> {
    let svg = bundled_icon_svg(connector_id)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg);
    Some(format!("data:image/svg+xml;base64,{b64}"))
}
