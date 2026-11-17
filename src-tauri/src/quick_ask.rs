//! Quick-ask handoff from the search window to the web app.
//!
//! The question typed in the quick-ask window stays here. The main window is sent to
//! `LUMO_URL/#quickask=<nonce>`, and the web app trades the nonce for the question with
//! [`take_quick_ask`]. The question never appears in a URL, so a link followed inside the webview
//! cannot make the web app auto-send a prompt: it can neither guess the nonce nor fill this slot.

pub mod window;

use base64::Engine;
use log::warn;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::State;

/// Fragment key the web app reads the nonce from. Must match `QUICK_ASK_PARAM` in clients'
/// `packages/lumo-api-client/core/desktop-quick-ask.ts`.
pub const FRAGMENT_KEY: &str = "quickask";

/// How long a question waits for the web app to reload and redeem it.
const TTL: Duration = Duration::from_secs(60);

struct Pending {
    nonce: String,
    query: String,
    created: Instant,
}

/// The one question waiting for the web app, if any. A newer question replaces it.
#[derive(Default)]
pub struct PendingQuickAsk(Mutex<Option<Pending>>);

impl PendingQuickAsk {
    fn slot(&self) -> MutexGuard<'_, Option<Pending>> {
        // The slot holds plain data, so a panic elsewhere cannot leave it inconsistent.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Hold `query` and return the nonce that redeems it.
    pub fn store(&self, query: String) -> String {
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(super::browser_login::random_bytes::<16>());
        self.store_at(query, nonce.clone(), Instant::now());
        nonce
    }

    fn store_at(&self, query: String, nonce: String, now: Instant) {
        *self.slot() = Some(Pending {
            nonce,
            query,
            created: now,
        });
    }

    /// Drop the waiting question without handing it out: `take_quick_ask` returns `None` until the
    /// next `store`.
    ///
    /// `swap_account` calls it so a question typed under one account is never sent under another,
    /// and `inject_search` calls it when the main window could not be sent to the nonce URL.
    pub fn clear(&self) {
        self.slot().take();
    }

    /// Hand the question out once, if `nonce` is the current one and it has not expired.
    ///
    /// A wrong nonce leaves the question in place, so a stray call cannot discard it before the
    /// web app redeems it.
    fn take_at(&self, nonce: &str, now: Instant) -> Option<String> {
        let mut slot = self.slot();
        let pending = slot.as_ref()?;
        if now.saturating_duration_since(pending.created) > TTL {
            *slot = None;
            return None;
        }
        if pending.nonce != nonce {
            return None;
        }
        slot.take().map(|pending| pending.query)
    }
}

/// Redeem the quick-ask nonce from `#quickask=<nonce>` for the question it stands for.
#[tauri::command]
pub fn take_quick_ask(
    nonce: String,
    webview: tauri::Webview,
    state: State<'_, PendingQuickAsk>,
) -> Result<Option<String>, String> {
    if webview.label() != crate::main_window::LABEL {
        warn!("quick-ask take refused from webview {}", webview.label());
        return Err("Quick ask is only available to the main window".into());
    }
    Ok(state.take_at(&nonce, Instant::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(query: &str, nonce: &str, now: Instant) -> PendingQuickAsk {
        let state = PendingQuickAsk::default();
        state.store_at(query.into(), nonce.into(), now);
        state
    }

    #[test]
    fn the_matching_nonce_redeems_the_question_once() {
        let now = Instant::now();
        let state = pending("what is the weather?", "abc", now);
        assert_eq!(
            state.take_at("abc", now).as_deref(),
            Some("what is the weather?")
        );
        assert_eq!(state.take_at("abc", now), None);
    }

    #[test]
    fn a_wrong_nonce_gets_nothing_and_leaves_the_question() {
        let now = Instant::now();
        let state = pending("hello", "abc", now);
        assert_eq!(state.take_at("guess", now), None);
        assert_eq!(state.take_at("", now), None);
        assert_eq!(state.take_at("abc", now).as_deref(), Some("hello"));
    }

    #[test]
    fn an_expired_question_is_dropped() {
        let now = Instant::now();
        let state = pending("hello", "abc", now);
        assert_eq!(
            state.take_at("abc", now + TTL + Duration::from_secs(1)),
            None
        );
        assert_eq!(state.take_at("abc", now), None);
    }

    #[test]
    fn a_newer_question_replaces_the_older_one() {
        let now = Instant::now();
        let state = pending("first", "one", now);
        state.store_at("second".into(), "two".into(), now);
        assert_eq!(state.take_at("one", now), None);
        assert_eq!(state.take_at("two", now).as_deref(), Some("second"));
    }

    #[test]
    fn clearing_drops_the_question() {
        let now = Instant::now();
        let state = pending("hello", "abc", now);
        state.clear();
        assert_eq!(state.take_at("abc", now), None);
    }

    #[test]
    fn stored_nonces_are_unguessable_and_url_safe() {
        let state = PendingQuickAsk::default();
        let first = state.store("a".into());
        let second = state.store("b".into());
        assert_ne!(first, second);
        // 16 random bytes, unpadded base64url.
        assert_eq!(first.len(), 22);
        assert!(first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
