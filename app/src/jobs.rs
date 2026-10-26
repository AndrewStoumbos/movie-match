//! Runs library calls on a background Tokio runtime and hands the results to the UI.

use std::future::Future;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;
use tokio::runtime::Runtime;

/// A background task's eventual result. Errors arrive already formatted for display.
pub struct Job<T> {
    rx: Receiver<Result<T, String>>,
}

impl<T: Send + 'static> Job<T> {
    /// Starts `task` on `runtime` and repaints the UI when it finishes.
    pub fn spawn<F>(runtime: &Runtime, ctx: &egui::Context, task: F) -> Self
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = task.await.map_err(|err| format!("{err:#}"));
            // The UI may have dropped the job (e.g. a newer one replaced it); that's fine.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self { rx }
    }

    /// The result once the task is done, `None` while it's still running.
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the task stopped unexpectedly".into())),
        }
    }
}
