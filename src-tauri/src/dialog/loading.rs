use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::oneshot;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub(crate) struct Loading {
    done: Arc<AtomicBool>,
    closed: Option<oneshot::Receiver<()>>,
}

impl Loading {
    pub(crate) fn show(app: &AppHandle, title: &str, message: &str) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let title = title.to_owned();
        let message = message.to_owned();
        let finished = done.clone();
        let (closed, completion) = oneshot::channel();

        #[cfg(target_os = "macos")]
        if let Err(error) = app.run_on_main_thread(move || {
            if !finished.load(Ordering::Acquire) {
                macos::show(&title, &message, &finished);
            }
            let _ = closed.send(());
        }) {
            log::warn!("Could not show loading dialog: {error}");
        }

        #[cfg(target_os = "linux")]
        {
            let app_handle = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                if !finished.load(Ordering::Acquire) {
                    linux::show(&app_handle, &title, &message, finished, closed);
                } else {
                    let _ = closed.send(());
                }
            }) {
                log::warn!("Could not show loading dialog: {error}");
            }
        }

        #[cfg(windows)]
        let owner = super::win32::owner(app);
        #[cfg(windows)]
        tauri::async_runtime::spawn_blocking(move || {
            if !finished.load(Ordering::Acquire) {
                if let Err(error) = windows::show(&title, &message, &finished, owner) {
                    log::warn!("Could not show loading dialog: {error}");
                }
            }
            let _ = closed.send(());
        });

        Self {
            done,
            closed: Some(completion),
        }
    }

    pub(crate) async fn finish(mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(closed) = self.closed.take() {
            if closed.await.is_err() {
                log::warn!("Loading dialog closed without a cleanup acknowledgement");
            }
        }
    }
}

impl Drop for Loading {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Poll;

    #[tokio::test]
    async fn finish_waits_for_native_cleanup() {
        let done = Arc::new(AtomicBool::new(false));
        let (closed, completion) = oneshot::channel();
        let loading = Loading {
            done: done.clone(),
            closed: Some(completion),
        };
        let mut finish = Box::pin(loading.finish());
        assert!(matches!(futures::poll!(&mut finish), Poll::Pending));
        assert!(done.load(Ordering::Acquire));
        closed.send(()).unwrap();
        finish.await;
    }

    #[tokio::test]
    async fn failed_creation_does_not_block_finish() {
        let (closed, completion) = oneshot::channel();
        drop(closed);
        Loading {
            done: Arc::new(AtomicBool::new(false)),
            closed: Some(completion),
        }
        .finish()
        .await;
    }

    #[test]
    fn dropping_before_creation_requests_cleanup() {
        let done = Arc::new(AtomicBool::new(false));
        let (_, completion) = oneshot::channel();
        drop(Loading {
            done: done.clone(),
            closed: Some(completion),
        });
        assert!(done.load(Ordering::Acquire));
    }
}
