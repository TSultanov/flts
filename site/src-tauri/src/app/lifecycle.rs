use super::{
    AppState, BackendState, config::Config, summary_generation_queue::SummaryGenerationQueue,
    task::ManagedTask, translation_queue::TranslationQueue,
};
use tokio::sync::{mpsc, oneshot};
type Reply<T> = oneshot::Sender<anyhow::Result<T>>;
pub(super) enum Command {
    Initialize(Reply<()>),
    Configure(Config, Reply<()>),
    Wake(Reply<()>),
    Rename(String, Reply<()>),
    Translation(Reply<TranslationQueue>),
    Summary(Reply<SummaryGenerationQueue>),
    #[cfg(feature = "e2e-bridge")]
    Configuration(Reply<(Config, usize)>),
    Gemini(Reply<library::translator::gemini_cache::GeminiPromptCache>),
}
#[derive(Clone)]
pub(super) struct Lifecycle {
    requests: mpsc::Sender<Command>,
    control: library::task::TaskControl,
}
impl Lifecycle {
    pub fn new(state: BackendState) -> (Self, ManagedTask) {
        let (requests, mut receiver) = mpsc::channel(256);
        let task = ManagedTask::spawn(move |mut stop| async move {
            loop {
                tokio::select! {
                    biased;
                    _ = stop.changed() => { state.perform_shutdown().await; return; }
                    command = receiver.recv() => {
                        let Some(command) = command else { state.perform_shutdown().await; return; };
                        match command {
                            Command::Initialize(reply) => { let result = state.initialize_session().await; let config = state.config.borrow().clone(); state.lyrics_state.configure(config).await; state.publish_ready(result.as_ref().map(|_| ()).map_err(|e| e.to_string())); let _ = reply.send(result); }
                            Command::Configure(config, reply) => {
                                state.gated.publish_reconfiguring();
                                let previous = state.config.borrow().clone();
                                let result = state.apply_config(config).await;
                                if result.is_err() { state.restore_configuration(previous).await; }
                                let config = state.config.borrow().clone();
                                state.lyrics_state.configure(config).await;
                                // Failed changes leave the previous working session usable.
                                let readiness = if state.gated.library_unchecked().is_some() {Ok(())} else {result.as_ref().map(|_| ()).map_err(|e| e.to_string())};
                                state.publish_ready(readiness); let _ = reply.send(result);
                            }
                            Command::Wake(reply) => { state.restart_sync_if_needed().await; let _ = reply.send(Ok(())); }
                            Command::Rename(name, reply) => { let _ = reply.send(state.rename_sync_device(name).await); }
                            Command::Translation(reply) => { let _ = reply.send(state.build_translation_queue().await); }
                            Command::Summary(reply) => { let _ = reply.send(state.build_summary_queue().await); }
                            Command::Gemini(reply) => { let _ = reply.send(state.get_gemini_prompt_cache().await); }
                            #[cfg(feature = "e2e-bridge")]
                            Command::Configuration(reply) => { let _ = reply.send(Ok((state.config.borrow().clone(), state.lyrics_state.generation()))); }
                        }
                    }
                }
            }
        });
        (
            Self {
                requests,
                control: task.control(),
            },
            task,
        )
    }
    pub async fn request<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> anyhow::Result<T> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(build(reply))
            .await
            .map_err(|_| anyhow::anyhow!("application lifecycle closed"))?;
        result
            .await
            .map_err(|_| anyhow::anyhow!("application lifecycle closed"))?
    }
    pub async fn shutdown(&self) {
        if let Err(error) = self.control.shutdown().await {
            log::warn!("application shutdown: {error}");
        }
    }
}
impl AppState {
    async fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }
    pub async fn eval_config(&self) -> anyhow::Result<()> {
        self.lifecycle().await.request(Command::Initialize).await
    }
    pub async fn update_config(&self, config: Config) -> anyhow::Result<()> {
        self.gated
            .await_settled()
            .await
            .map_err(anyhow::Error::msg)?;
        self.lifecycle()
            .await
            .request(|reply| Command::Configure(config, reply))
            .await
    }
    #[cfg(feature = "e2e-bridge")]
    pub(super) async fn configuration(&self) -> anyhow::Result<(Config, usize)> {
        self.lifecycle().await.request(Command::Configuration).await
    }
    pub async fn wake_sync(&self) {
        if let Err(error) = self.lifecycle().await.request(Command::Wake).await {
            log::warn!("sync wake: {error}");
        }
    }
    pub async fn set_sync_device_name(&self, name: String) -> anyhow::Result<()> {
        self.lifecycle()
            .await
            .request(|reply| Command::Rename(name, reply))
            .await
    }
    pub async fn get_or_init_summary_generation_queue(
        &self,
    ) -> anyhow::Result<SummaryGenerationQueue> {
        self.gated.await_ready().await.map_err(anyhow::Error::msg)?;
        self.lifecycle().await.request(Command::Summary).await
    }
    pub(super) async fn get_or_init_translation_queue(&self) -> anyhow::Result<TranslationQueue> {
        self.gated.await_ready().await.map_err(anyhow::Error::msg)?;
        self.lifecycle().await.request(Command::Translation).await
    }
    pub async fn shutdown(&self) {
        // The controller keeps draining after this deadline; ownership is never
        // released merely because the UI stopped waiting for shutdown.
        let deadline = super::EXIT_SAVE_ALL_TIMEOUT
            + super::EXIT_STOP_QUEUE_TIMEOUT * 3
            + std::time::Duration::from_secs(1);
        if tokio::time::timeout(deadline, self.lifecycle().await.shutdown())
            .await
            .is_err()
        {
            log::warn!("application shutdown is still draining after {deadline:?}");
        }
    }
}
