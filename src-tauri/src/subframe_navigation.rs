//! Let iframes navigate without asking the app's navigation handler.
//!
//! wry calls the handler for every frame but passes only the URL, so `webview::classify` sends
//! third-party iframes (Chargebee's card form, 3-D Secure challenges) to the browser. This
//! replaces wry's `webView:decidePolicyForNavigationAction:decisionHandler:` so that only main
//! frame navigations and new-window requests reach it.

use std::sync::OnceLock;

use log::warn;
use objc::runtime::{
    class_getInstanceMethod, method_setImplementation, Class, Imp, Object, Sel, BOOL, NO,
};
use objc::{msg_send, sel, sel_impl};

/// Clang block ABI header, enough to call a block taking a `WKNavigationActionPolicy`.
#[repr(C)]
struct PolicyBlock {
    isa: *const std::ffi::c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut PolicyBlock, isize),
}

const WK_NAVIGATION_ACTION_POLICY_CANCEL: isize = 0;
const WK_NAVIGATION_ACTION_POLICY_ALLOW: isize = 1;

type DecidePolicy = extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut PolicyBlock);

static WRY_DECIDE_POLICY: OnceLock<DecidePolicy> = OnceLock::new();

extern "C" fn decide_policy(
    this: &Object,
    sel: Sel,
    webview: *mut Object,
    action: *mut Object,
    handler: *mut PolicyBlock,
) {
    if is_subframe_navigation(action) {
        unsafe { ((*handler).invoke)(handler, WK_NAVIGATION_ACTION_POLICY_ALLOW) };
    } else if let Some(wry) = WRY_DECIDE_POLICY.get() {
        wry(this, sel, webview, action, handler);
    } else {
        unsafe { ((*handler).invoke)(handler, WK_NAVIGATION_ACTION_POLICY_CANCEL) };
    }
}

/// Downloads stay with wry, which routes them to the app's download handler.
fn is_subframe_navigation(action: *mut Object) -> bool {
    unsafe {
        let frame: *mut Object = msg_send![action, targetFrame];
        if frame.is_null() {
            return false;
        }
        let is_main: BOOL = msg_send![frame, isMainFrame];
        if is_main != NO {
            return false;
        }
        let can_download: BOOL = msg_send![action, respondsToSelector: sel!(shouldPerformDownload)];
        if can_download != NO {
            let download: BOOL = msg_send![action, shouldPerformDownload];
            if download != NO {
                return false;
            }
        }
        true
    }
}

/// Takes the class from the window's live delegate, since objc2 names it with wry's module path
/// and version.
pub fn install(window: &tauri::WebviewWindow) {
    if let Err(e) = window.with_webview(|webview| unsafe {
        let delegate: *mut Object = msg_send![webview.inner() as *mut Object, navigationDelegate];
        if delegate.is_null() {
            warn!("webview has no navigation delegate, iframes will go through the navigation handler");
            return;
        }
        replace_decide_policy((*delegate).class());
    }) {
        warn!("could not reach the webview, iframes will go through the navigation handler: {e}");
    }
}

fn replace_decide_policy(class: &Class) {
    unsafe {
        let method = class_getInstanceMethod(
            class,
            sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
        );
        if method.is_null() {
            warn!("{} has no navigation policy method, iframes will go through the navigation handler", class.name());
            return;
        }
        let imp: Imp = std::mem::transmute(decide_policy as DecidePolicy);
        let wry: DecidePolicy =
            std::mem::transmute(method_setImplementation(method as *mut _, imp));
        let _ = WRY_DECIDE_POLICY.set(wry);
    }
}
