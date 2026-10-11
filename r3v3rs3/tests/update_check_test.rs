use futures::FutureExt;
use r3v3rs3::{command::ServerCommand, server::rpc::auth::GetUpdateAvailable};
use r3v3rs3_api::event::ServerEvent;

mod common;
use common::{TestStorage, with_server};

#[tokio::test]
async fn an_update_check_updates_the_state_and_broadcasts_the_change() -> anyhow::Result<()> {
    with_server(TestStorage::builder().build(), |mut channels| async move {
        let mut events = channels.event.subscribe();
        for tag in [Some("v9.9.9".to_string()), None] {
            channels
                .command
                .send(ServerCommand::SetUpdateAvailable { tag: tag.clone() })
                .await?;
            let ServerEvent::UpdateAvailable { tag: broadcast } = next_event(&mut events).await?
            else {
                panic!("the update check sent another event");
            };
            assert_eq!(broadcast.as_deref(), tag.as_deref());
            let available = common::call(&mut channels, GetUpdateAvailable).await??;
            assert_eq!(available, tag);
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_repeated_tag_broadcasts_nothing() -> anyhow::Result<()> {
    with_server(TestStorage::builder().build(), |channels| async move {
        let mut events = channels.event.subscribe();
        for _ in 0..2 {
            channels
                .command
                .send(ServerCommand::SetUpdateAvailable {
                    tag: Some("v9.9.9".into()),
                })
                .await?;
        }
        // The commands run in order, so the drain of one event proves that the second
        // identical tag stayed silent.
        let ServerEvent::UpdateAvailable { tag: Some(tag) } = next_event(&mut events).await? else {
            panic!("the first change sent another event");
        };
        assert_eq!(tag, "v9.9.9");
        assert!(
            events.recv().now_or_never().is_none(),
            "the identical tag sent a second event"
        );
        Ok(())
    })
    .await
}

/// The next event of the stream, or an error when the server closed it.
async fn next_event(
    events: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
) -> anyhow::Result<ServerEvent> {
    events
        .recv()
        .await
        .map_err(|err| anyhow::anyhow!("event channel closed: {err}"))
}
