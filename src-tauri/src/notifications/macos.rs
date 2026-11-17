//! Native macOS notification authorization, delivery, and settings.

use tauri::AppHandle;

use super::NotificationPermission;

pub(super) async fn permission_state(_app: &AppHandle) -> Result<NotificationPermission, String> {
    use mac_usernotifications::AuthorizationStatus;

    let settings = mac_usernotifications::get_notification_settings()
        .await
        .map_err(|error| error.to_string())?;
    log::info!(
        "macOS notification settings: authorization={:?}, alerts={:?}, notification_center={:?}",
        settings.authorization_status,
        settings.alert_enabled,
        settings.notification_center_enabled,
    );
    match settings.authorization_status {
        AuthorizationStatus::Authorized
        | AuthorizationStatus::Provisional
        | AuthorizationStatus::Ephemeral => Ok(NotificationPermission::Granted),
        AuthorizationStatus::NotDetermined => Ok(NotificationPermission::NotDetermined),
        _ => Ok(NotificationPermission::Denied),
    }
}

pub(super) async fn request_permission(_app: &AppHandle) -> Result<bool, String> {
    mac_usernotifications::request_auth()
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn show(_app: &AppHandle, title: String, body: String) -> Result<(), String> {
    // Use UNUserNotificationCenter directly. The Tauri 2.4 desktop backend uses
    // NSUserNotificationCenter and discards the eventual delivery error.
    mac_usernotifications::Notification::new()
        .title(title)
        .message(body)
        .send()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn open_settings() -> Result<(), String> {
    open::that_detached("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
        .map_err(|error| error.to_string())
}
