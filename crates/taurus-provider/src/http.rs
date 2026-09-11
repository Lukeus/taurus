//! The HTTP client every adapter talks to its backend through, and the rules
//! it applies to each request.
//!
//! Only what has to behave the same for every backend lives here: how long a
//! request may wait, how a transport failure is named, and how a provider's
//! request to be left alone is read. The wire formats stay in their adapters.

use std::io;
use std::ops::Deref;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::HeaderMap;

use crate::ProviderError;

/// How long a connection may take to open.
///
/// A backend that has not accepted a connection and finished TLS in this long
/// is not going to. A slow one takes a second or two.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a response may go without sending a single byte.
///
/// Per read, not for the whole request: a deadline on the whole request would
/// cut off a long answer that is streaming perfectly well. Ten minutes is
/// longer than any silence a working backend produces — a reasoning model
/// thinking before its first token, or a local model evaluating a long prompt
/// on a CPU — and it still ends a connection that has gone quiet for good,
/// without anybody having to press Stop.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(600);

/// TCP keepalive: probe an idle connection after a minute, then every fifteen
/// seconds, and drop it after four probes go unanswered.
///
/// This is what finds a peer that has vanished — a laptop that changed
/// networks, a NAT that forgot the mapping — in about two minutes rather than
/// at [`STALL_TIMEOUT`]. A backend that is alive but thinking answers the
/// probes, so this never cuts off a slow model.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_RETRIES: u32 = 4;

/// A reqwest client with the timeouts above, carrying the one fact a failure
/// message needs about them.
///
/// Derefs to [`reqwest::Client`], so an adapter builds requests on it exactly
/// as it would on a bare one.
#[derive(Clone, Debug)]
pub struct Client {
    inner: reqwest::Client,
    stall: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// The process's shared client.
    ///
    /// One connection pool for every provider. A provider is rebuilt whenever
    /// its settings change, and building a client each time would throw away
    /// every kept-alive connection and load the TLS roots again.
    pub fn new() -> Self {
        static SHARED: OnceLock<reqwest::Client> = OnceLock::new();
        Self {
            inner: SHARED.get_or_init(|| build(STALL_TIMEOUT)).clone(),
            stall: STALL_TIMEOUT,
        }
    }

    /// A client that gives up after `stall` of silence rather than
    /// [`STALL_TIMEOUT`].
    ///
    /// For a test of a backend that hangs, which cannot wait ten minutes to
    /// watch one.
    pub fn stalling_after(stall: Duration) -> Self {
        Self {
            inner: build(stall),
            stall,
        }
    }

    /// Names a failure to get a response, or to finish reading one.
    ///
    /// reqwest's own `Display` for a timeout is a wrapper naming the URL, which
    /// says a request failed and not that the clock ran out. Both timeouts are
    /// stated here in the words a user can act on.
    pub fn failure(&self, provider: &str, base_url: &str, error: reqwest::Error) -> ProviderError {
        if error.is_timeout() && !error.is_connect() {
            return ProviderError::Stalled {
                provider: provider.to_string(),
                after: self.stall,
            };
        }
        let source: Box<dyn std::error::Error + Send + Sync> = if error.is_timeout() {
            Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no connection within {} seconds", CONNECT_TIMEOUT.as_secs()),
            ))
        } else {
            Box::new(error)
        };
        ProviderError::Unreachable {
            provider: provider.to_string(),
            base_url: base_url.to_string(),
            source,
        }
    }
}

impl Deref for Client {
    type Target = reqwest::Client;

    fn deref(&self) -> &reqwest::Client {
        &self.inner
    }
}

fn build(stall: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(stall)
        .tcp_keepalive(KEEPALIVE_IDLE)
        .tcp_keepalive_interval(KEEPALIVE_INTERVAL)
        .tcp_keepalive_retries(KEEPALIVE_RETRIES)
        .build()
        // Fails only if the TLS backend cannot initialize. A default client
        // cannot reach an https backend either, but it still reaches a local
        // one, and it fails each request with a message rather than taking
        // the provider down at construction.
        .unwrap_or_default()
}

/// How long a response asks the caller to wait before trying again.
///
/// Reads `retry-after-ms` first, which OpenAI sends with a finer grain, then
/// `retry-after` as a number of seconds. The HTTP-date form of `retry-after`
/// reads as absent: none of the backends this talks to send it, and a caller
/// without a hint falls back to its own backoff, which is what it would do
/// with no header at all.
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let number = |name: &str| -> Option<f64> {
        let value = headers
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?;
        (value.is_finite() && value >= 0.0).then_some(value)
    };
    if let Some(ms) = number("retry-after-ms") {
        return Some(Duration::from_secs_f64(ms / 1000.0));
    }
    number("retry-after").map(Duration::from_secs_f64)
}

/// Backends that misbehave on purpose, for the adapters' own tests.
pub mod testing {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server that takes one connection, reads the request, sends `prefix`,
    /// and then says nothing more. Returns its base URL.
    ///
    /// The socket is held open for a minute, so a client sees silence rather
    /// than a close. An empty `prefix` is a backend that never answers at all.
    pub async fn silent_after(prefix: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("a bound address");
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 16 * 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(prefix).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
            drop(socket);
        });
        format!("http://{address}")
    }
}

#[cfg(test)]
mod tests {
    use super::testing::silent_after;
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn retry_after_reads_seconds_and_prefers_milliseconds() {
        assert_eq!(
            retry_after(&headers(&[("retry-after", "20")])),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            retry_after(&headers(&[
                ("retry-after", "20"),
                ("retry-after-ms", "1500")
            ])),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn retry_after_that_is_not_a_number_reads_as_absent() {
        assert_eq!(retry_after(&HeaderMap::new()), None);
        assert_eq!(
            retry_after(&headers(&[(
                "retry-after",
                "Wed, 21 Oct 2026 07:28:00 GMT"
            )])),
            None
        );
        assert_eq!(retry_after(&headers(&[("retry-after", "-3")])), None);
    }

    #[tokio::test]
    async fn a_backend_that_never_answers_is_named_as_stalled() {
        let base = silent_after(b"").await;
        let client = Client::stalling_after(Duration::from_millis(200));
        let error = tokio::time::timeout(Duration::from_secs(5), client.get(&base).send())
            .await
            .expect("the read timeout must end the wait, not the test's own")
            .expect_err("nothing was sent back");
        let failure = client.failure("anthropic", &base, error);
        assert!(
            matches!(failure, ProviderError::Stalled { after, .. } if after == Duration::from_millis(200)),
            "{failure:?}"
        );
        assert!(failure.is_transient());
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_part_way_is_named_as_stalled() {
        let base = silent_after(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n6\r\ndata:\n\r\n",
        )
        .await;
        let client = Client::stalling_after(Duration::from_millis(200));
        let mut response = client.get(&base).send().await.expect("headers arrive");
        let first = response.chunk().await.expect("the first chunk arrives");
        assert!(first.is_some());
        let error = tokio::time::timeout(Duration::from_secs(5), response.chunk())
            .await
            .expect("the read timeout must end the wait, not the test's own")
            .expect_err("the stream went quiet");
        assert!(matches!(
            client.failure("openai", &base, error),
            ProviderError::Stalled { .. }
        ));
    }
}
