use std::{future::Future, sync::Mutex};
use tokio::{sync::watch, task::JoinHandle};

/// Completion is shared, so every shutdown caller waits for the same drain.
pub struct ManagedTask {
    stop: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), String>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}
struct Completion(watch::Sender<Option<Result<(), String>>>);
impl Drop for Completion {
    fn drop(&mut self) {
        if self.0.borrow().is_none() {
            self.0
                .send_replace(Some(Err("background task exited unexpectedly".into())));
        }
    }
}
impl ManagedTask {
    pub fn spawn<F: Future<Output = ()> + Send + 'static>(
        build: impl FnOnce(watch::Receiver<bool>) -> F,
    ) -> Self {
        let (stop, receiver) = watch::channel(false);
        let (done, completion) = watch::channel(None);
        let future = build(receiver);
        let join = tokio::spawn(async move {
            let guard = Completion(done);
            future.await;
            guard.0.send_replace(Some(Ok(())));
        });
        Self {
            stop,
            done: completion,
            join: Mutex::new(Some(join)),
        }
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.stop.send_replace(true);
        let mut done = self.done.clone();
        loop {
            let result = done.borrow_and_update().clone();
            if let Some(result) = result {
                let join = self.join.lock().unwrap().take();
                if let Some(join) = join {
                    join.await.map_err(|e| e.to_string())?;
                }
                return result;
            }
            done.changed()
                .await
                .map_err(|_| "task completion channel closed".to_string())?;
        }
    }
}
impl Drop for ManagedTask {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
