//! One multiplexed agent connection. A yamux connection runs inside the TLS stream. The master
//! opens a new stream for every request and every tunnel; the agent only accepts streams.

use super::frame::{read_frame, write_frame, write_payload};
use super::protocol::{AgentOutput, AgentReply, AgentRequest};
use anyhow::anyhow;
use futures::future::poll_fn;
use std::task::Poll;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use yamux::{Config, Connection, ConnectionError, Mode, Stream};

/// A request of a new outbound stream, answered by the task that drives the connection.
type OpenRequest = oneshot::Sender<Result<Stream, ConnectionError>>;

/// The stream of one request or one tunnel, as a tokio byte stream.
pub type LinkStream = Compat<Stream>;

/// The handle of the master to one agent connection. A clone opens streams on the same
/// connection.
#[derive(Clone)]
pub struct Link {
    opens: mpsc::Sender<OpenRequest>,
}

impl Link {
    /// Starts the task that drives the connection on `io`. The task ends when the connection
    /// closes or fails; aborting it closes the connection.
    pub fn spawn<T>(io: T) -> (Self, JoinHandle<()>)
    where
        T: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let (opens, requests) = mpsc::channel(64);
        let connection = Connection::new(io, Config::default(), Mode::Client);
        let task = tokio::spawn(async move {
            // The master opens every stream, so an inbound stream is a fault of the agent.
            let result = drive(connection, requests, drop).await;
            if let Err(err) = result {
                tracing::debug!(%err, "the agent connection ended");
            }
        });
        (Self { opens }, task)
    }

    /// Whether the task of the connection has ended.
    pub fn is_closed(&self) -> bool {
        self.opens.is_closed()
    }

    pub async fn open(&self) -> anyhow::Result<LinkStream> {
        let (answer, stream) = oneshot::channel();
        self.opens
            .send(answer)
            .await
            .map_err(|_| anyhow!("the agent is not connected"))?;
        let stream = stream
            .await
            .map_err(|_| anyhow!("the agent is not connected"))??;
        Ok(stream.compat())
    }

    /// Sends one request on a new stream and reads its answer. An error answer of the agent
    /// becomes an error.
    pub async fn request(&self, request: &AgentRequest) -> anyhow::Result<AgentOutput> {
        let mut stream = self.open().await?;
        write_frame(&mut stream, request).await?;
        read_reply(&mut stream).await
    }

    /// Sends one request with its payload on a new stream and reads its answer.
    pub async fn request_with_payload(
        &self,
        request: &AgentRequest,
        payload: &[u8],
    ) -> anyhow::Result<AgentOutput> {
        let mut stream = self.open().await?;
        write_frame(&mut stream, request).await?;
        write_payload(&mut stream, payload).await?;
        read_reply(&mut stream).await
    }

    /// Opens a stream that the agent connects to a port, and returns it once the agent confirmed
    /// the connection.
    pub async fn tunnel(&self, request: &AgentRequest) -> anyhow::Result<LinkStream> {
        let mut stream = self.open().await?;
        write_frame(&mut stream, request).await?;
        match read_reply(&mut stream).await? {
            AgentOutput::Tunnel => Ok(stream),
            other => Err(anyhow!("the agent answered a tunnel with {other:?}")),
        }
    }
}

async fn read_reply(stream: &mut LinkStream) -> anyhow::Result<AgentOutput> {
    match read_frame(stream).await? {
        AgentReply::Ok(output) => Ok(output),
        AgentReply::Error { message } => Err(anyhow!(message)),
    }
}

/// Serves the connection of an agent: every stream that the master opens goes to `on_stream`.
/// Returns when the connection closes.
pub async fn serve<T>(io: T, on_stream: impl FnMut(LinkStream)) -> Result<(), ConnectionError>
where
    T: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    let connection = Connection::new(io, Config::default(), Mode::Server);
    // The agent opens no stream, so the sender side is dropped at once.
    let (_, requests) = mpsc::channel(1);
    let mut on_stream = on_stream;
    drive(connection, requests, |stream| on_stream(stream.compat())).await
}

/// Drives the connection: answers the outbound stream requests and hands every inbound stream to
/// `on_inbound`. Polling for inbound streams is what moves the bytes of every stream.
async fn drive<T>(
    mut connection: Connection<T>,
    mut requests: mpsc::Receiver<OpenRequest>,
    mut on_inbound: impl FnMut(Stream),
) -> Result<(), ConnectionError>
where
    T: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    let mut waiting: Option<OpenRequest> = None;
    poll_fn(|cx| {
        loop {
            if waiting.is_none()
                && let Poll::Ready(Some(request)) = requests.poll_recv(cx)
            {
                waiting = Some(request);
            }
            if let Some(request) = waiting.take() {
                match connection.poll_new_outbound(cx) {
                    Poll::Ready(result) => {
                        // A requester that stopped waiting drops the new stream, which closes it.
                        let _ = request.send(result);
                        continue;
                    }
                    Poll::Pending => waiting = Some(request),
                }
            }
            match connection.poll_next_inbound(cx) {
                Poll::Ready(Some(Ok(stream))) => on_inbound(stream),
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Err(err)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::frame::{read_frame, write_frame};
    use tokio::io::duplex;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    /// An agent that answers every ping with its version.
    async fn agent(io: tokio::io::DuplexStream) {
        let result = serve(io.compat(), |mut stream| {
            tokio::spawn(async move {
                let request: AgentRequest = read_frame(&mut stream).await?;
                assert_eq!(request, AgentRequest::Ping);
                let reply = AgentReply::Ok(AgentOutput::Pong {
                    version: "9.9.9".into(),
                });
                write_frame(&mut stream, &reply).await
            });
        })
        .await;
        assert!(result.is_ok() || matches!(result, Err(ConnectionError::Closed)));
    }

    #[tokio::test]
    async fn requests_run_side_by_side_on_one_connection() -> anyhow::Result<()> {
        let (master_io, agent_io) = duplex(1 << 16);
        tokio::spawn(agent(agent_io));
        let (link, task) = Link::spawn(master_io.compat());
        let pings = (0..20).map(|_| link.request(&AgentRequest::Ping));
        for pong in futures::future::join_all(pings).await {
            assert_eq!(
                pong?,
                AgentOutput::Pong {
                    version: "9.9.9".into()
                }
            );
        }
        task.abort();
        let _ = task.await;
        assert!(link.is_closed());
        assert!(link.request(&AgentRequest::Ping).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn an_error_answer_becomes_an_error() -> anyhow::Result<()> {
        let (master_io, agent_io) = duplex(1 << 16);
        tokio::spawn(async move {
            serve(agent_io.compat(), |mut stream| {
                tokio::spawn(async move {
                    let _: AgentRequest = read_frame(&mut stream).await?;
                    let reply = AgentReply::Error {
                        message: "no such container".into(),
                    };
                    write_frame(&mut stream, &reply).await
                });
            })
            .await
        });
        let (link, _task) = Link::spawn(master_io.compat());
        let err = link.request(&AgentRequest::Ping).await.unwrap_err();
        assert_eq!(err.to_string(), "no such container");
        Ok(())
    }
}
