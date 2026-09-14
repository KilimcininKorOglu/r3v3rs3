//! Starts an accepted TCP connection, which can first be an ACME HTTP-01 challenge request.

use hyper::service::service_fn;
use hyper::Response;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufStream};
use tokio::net::TcpStream;
use tracing::error;

/// The ACME HTTP-01 tokens and their key authorizations.
pub type HttpChallenges = Arc<HashMap<String, String>>;

/// The time that a client has to send its first bytes while a challenge is active.
const CHALLENGE_PEEK_TIMEOUT: Duration = Duration::from_secs(10);

const HTTP_CHALLENGE_HEADER: &[u8] = b"GET /.well-known/acme-challenge/";

/// Starts the connection without waiting for the client. While a challenge is active, a new task
/// reads the first bytes: a challenge request receives its key authorization, and every other
/// connection goes to `start` with the bytes that it sent.
pub fn accept<F>(stream: TcpStream, challenges: &HttpChallenges, start: F)
where
    F: FnOnce(BufStream<TcpStream>) + Send + 'static,
{
    let mut stream = BufStream::new(stream);
    if challenges.is_empty() {
        start(stream);
        return;
    }
    let challenges = challenges.clone();
    tokio::spawn(async move {
        match challenge_body(&challenges, &mut stream).await {
            Some(body) => serve_challenge(stream, body).await,
            None => start(stream),
        }
    });
}

/// The key authorization of a challenge request. `None` for another request, and for a client
/// that sends nothing in time.
async fn challenge_body(
    challenges: &HashMap<String, String>,
    stream: &mut BufStream<TcpStream>,
) -> Option<String> {
    let buf = tokio::time::timeout(CHALLENGE_PEEK_TIMEOUT, stream.fill_buf())
        .await
        .ok()?
        .ok()?;
    let token = buf
        .strip_prefix(HTTP_CHALLENGE_HEADER)?
        .split(|&byte| byte == b' ')
        .next()?;
    challenges.get(std::str::from_utf8(token).ok()?).cloned()
}

async fn serve_challenge(stream: BufStream<TcpStream>, body: String) {
    let service = service_fn(move |_| {
        let body = body.clone();
        async move { Ok::<_, Infallible>(Response::new(body)) }
    });
    if let Err(err) = auto::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), service)
        .await
    {
        error!("Error serving connection: {:?}", err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    fn challenges() -> HttpChallenges {
        Arc::new(HashMap::from([(
            "token".to_string(),
            "token.key".to_string(),
        )]))
    }

    /// Connects a client, and returns the client stream and the accepted stream.
    async fn connect(listener: &TcpListener) -> (TcpStream, TcpStream) {
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        (client, accepted)
    }

    #[tokio::test]
    async fn an_idle_client_does_not_delay_a_challenge_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let started = Arc::new(AtomicBool::new(false));

        let (_idle, accepted) = connect(&listener).await;
        let flag = started.clone();
        accept(accepted, &challenges(), move |_| {
            flag.store(true, Ordering::Relaxed)
        });

        let (mut client, accepted) = connect(&listener).await;
        let flag = started.clone();
        accept(accepted, &challenges(), move |_| {
            flag.store(true, Ordering::Relaxed)
        });
        let request = b"GET /.well-known/acme-challenge/token HTTP/1.1\r\nHost: a.test\r\nConnection: close\r\n\r\n";
        let exchange = async {
            client.write_all(request).await?;
            let mut response = String::new();
            client.read_to_string(&mut response).await?;
            std::io::Result::Ok(response)
        };
        let response = tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("token.key"), "{response}");
        assert!(!started.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn other_connections_reach_the_port_with_their_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        for challenges in [challenges(), HttpChallenges::default()] {
            let (mut client, accepted) = connect(&listener).await;
            client
                .write_all(b"GET /other HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let (sender, receiver) = oneshot::channel();
            accept(accepted, &challenges, move |stream| {
                let _ = sender.send(stream);
            });
            let mut stream = receiver.await.unwrap();
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line, "GET /other HTTP/1.1\r\n");
        }
    }
}
