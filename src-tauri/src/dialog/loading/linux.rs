use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

pub(super) fn show(
    app: &AppHandle,
    title: &str,
    message: &str,
    done: Arc<AtomicBool>,
    closed: oneshot::Sender<()>,
) {
    let dialog = gtk::Dialog::builder()
        .title(title)
        .modal(true)
        .deletable(false)
        .resizable(false)
        .build();
    dialog.connect_delete_event(|_, _| glib::Propagation::Stop);
    if let Some(parent) = app.get_webview_window(crate::main_window::LABEL) {
        if parent.is_visible().unwrap_or(false) {
            if let Ok(parent) = parent.gtk_window() {
                dialog.set_transient_for(Some(&parent));
            }
        }
    }
    let content = dialog.content_area();
    content.set_border_width(20);
    content.set_spacing(16);
    let label = gtk::Label::new(Some(message));
    label.set_xalign(0.0);
    content.add(&label);
    let bar = gtk::ProgressBar::new();
    bar.set_size_request(280, -1);
    bar.set_pulse_step(0.1);
    content.add(&bar);

    let dialog_alive = Rc::new(Cell::new(true));
    let alive = dialog_alive.clone();
    let closed = std::cell::RefCell::new(Some(closed));
    dialog.connect_destroy(move |_| {
        alive.set(false);
        if let Some(closed) = closed.borrow_mut().take() {
            glib::idle_add_local_once(move || {
                let _ = closed.send(());
            });
        }
    });
    let bar_alive = Rc::new(Cell::new(true));
    let alive = bar_alive.clone();
    bar.connect_destroy(move |_| alive.set(false));
    let dialog_weak = dialog.downgrade();
    let bar_weak = bar.downgrade();

    dialog.show_all();
    dialog.present();
    glib::timeout_add_local(Duration::from_millis(80), move || {
        if !dialog_alive.get() {
            return ControlFlow::Break;
        }
        let Some(dialog) = dialog_weak.upgrade() else {
            return ControlFlow::Break;
        };
        if done.load(Ordering::Acquire) {
            unsafe { dialog.destroy() };
            ControlFlow::Break
        } else {
            // Completion remains independent of the optional animation widget.
            if bar_alive.get() {
                if let Some(bar) = bar_weak.upgrade() {
                    bar.pulse();
                }
            }
            ControlFlow::Continue
        }
    });
}
