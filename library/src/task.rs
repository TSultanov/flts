use std::{future::Future, sync::Mutex};
use tokio::{sync::watch, task::JoinHandle};

/// Completion is shared, so every shutdown caller waits for the same drain.
pub struct TaskOwner {
    control: TaskControl,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct TaskControl {
    stop: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), String>>>,
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
impl TaskOwner {
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
            control: TaskControl {
                stop,
                done: completion,
            },
            join: Mutex::new(Some(join)),
        }
    }
    pub fn control(&self) -> TaskControl {
        self.control.clone()
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.control.shutdown().await?;
        let join = self.join.lock().unwrap().take();
        if let Some(join) = join {
            join.await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl TaskControl {
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.stop();
        self.wait().await
    }
    pub async fn wait(&self) -> Result<(), String> {
        let mut done = self.done.clone();
        loop {
            if let Some(result) = done.borrow_and_update().clone() {
                return result;
            }
            done.changed()
                .await
                .map_err(|_| "task completion channel closed".to_string())?;
        }
    }
}

impl Drop for TaskOwner {
    fn drop(&mut self) {
        self.control.stop();
    }
}

#[cfg(test)]

mod tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn every_shutdown_caller_waits_for_the_same_completion() {
        let (release, blocked) = tokio::sync::oneshot::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let task = Arc::new(TaskOwner::spawn(move |mut stop| async move {
            stop.changed().await.unwrap();
            entered.send(()).unwrap();
            let _ = blocked.await;
        }));
        let first = tokio::spawn({
            let task = task.clone();
            async move { task.shutdown().await }
        });
        started.await.unwrap();
        let second = tokio::spawn({
            let task = task.clone();
            async move { task.shutdown().await }
        });
        tokio::task::yield_now().await;
        assert!(!first.is_finished());
        assert!(!second.is_finished());
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        task.shutdown().await.unwrap();
    }
}
