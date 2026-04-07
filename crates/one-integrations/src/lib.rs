pub mod asana;
pub mod github;
pub mod notion;
pub mod slack;

/// Trait for external service integrations that produce notifications.
#[trait_variant::make(Send)]
pub trait Integration {
    /// Human-readable name of the integration
    fn name(&self) -> &str;

    /// Start polling/listening for events. Sends notifications via the event bus.
    async fn start(
        &mut self,
        event_tx: tokio::sync::broadcast::Sender<one_core::event::Event>,
    ) -> anyhow::Result<()>;

    /// Gracefully stop the integration
    async fn stop(&mut self) -> anyhow::Result<()>;
}
