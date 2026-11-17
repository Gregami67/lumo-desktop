use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};

/// Emitted to every webview when the hosted web app switches between light and dark.
pub const WEB_THEME_CHANGED_EVENT: &str = "web-theme:changed";

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}

/// The theme the hosted web app last rendered with, resolved (never "auto"). `None` until the
/// web app has reported, in which case the bundled pages fall back to the OS preference.
#[derive(Default)]
pub struct WebTheme(Mutex<Option<Theme>>);

#[tauri::command]
pub fn report_web_theme(theme: Theme, app_handle: AppHandle, state: State<'_, WebTheme>) {
    let Ok(mut current) = state.0.lock() else {
        return;
    };
    if *current == Some(theme) {
        return;
    }
    *current = Some(theme);
    drop(current);
    let _ = app_handle.emit(WEB_THEME_CHANGED_EVENT, theme);
}

#[tauri::command]
pub fn get_web_theme(state: State<'_, WebTheme>) -> Option<Theme> {
    state.0.lock().ok().and_then(|current| *current)
}
