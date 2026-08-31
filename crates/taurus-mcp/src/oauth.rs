//! Signing in to an MCP server that wants OAuth.
//!
//! # Why this exists
//!
//! Taurus could send an HTTP server fixed headers and nothing else, which is
//! enough for a server that issues personal access tokens and enough for
//! nothing else. The ecosystem went the other way: Linear, Sentry, Stripe,
//! Notion, Cloudflare and Google's own Drive server are all hosted and all
//! secured with an OAuth consent flow, so "add an MCP server" meant "add one of
//! the few that still hand out tokens". This is the ceiling that lifts.
//!
//! # What is here and what is not
//!
//! Almost none of the protocol is. `rmcp`'s `auth` feature implements the parts
//! that are specification rather than judgement — protected-resource discovery
//! from the `WWW-Authenticate` challenge, authorization-server metadata, PKCE,
//! dynamic client registration, the RFC 8707 `resource` parameter, RFC 9207
//! issuer validation, and refresh. What is left is the four things it cannot
//! know:
//!
//! 1. **Where the browser comes back to.** A desktop application has no
//!    hosted redirect, so it listens on loopback for exactly one request. See
//!    [`Loopback`].
//! 2. **Where the tokens live.** A refresh token is a long-lived credential and
//!    belongs in the OS keychain, which this crate cannot reach on its own —
//!    see [`TokenVault`].
//! 3. **When to start.** Never on its own. A connection that opened a browser
//!    window because a server returned 401 would be the application taking over
//!    the screen in response to something the user did not do. A server that
//!    needs signing in says so and waits to be asked.
//! 4. **How the token reaches the transport.** See [`Authorized`], which mints
//!    one per request rather than once per connection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::auth::{
    AuthError, AuthorizationCallback, AuthorizationManager, CredentialStore, StoredCredentials,
};
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use rmcp::model::ClientJsonRpcMessage;
use reqwest::header::{HeaderName, HeaderValue};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// How long the browser has to come back before the listener gives up.
///
/// Long enough to find a password manager, create an account, and read a
/// consent screen properly; short enough that a window abandoned an hour ago is
/// not still holding a port open.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// What Taurus calls itself when registering with an authorization server.
///
/// Shown on the consent screen, so it has to be the name the user recognises
/// rather than a crate name.
const CLIENT_NAME: &str = "Taurus";

/// Somewhere to keep a refresh token.
///
/// A trait rather than a keychain call, because the platform matrix for that —
/// which backend on which OS, and what happens on a machine with none — is
/// already written down once in `taurus_host::secrets`, and this crate sits
/// below that one. Duplicating it would give the same question two answers that
/// drift.
pub trait TokenVault: Send + Sync {
    fn read(&self, key: &str) -> Option<String>;
    fn write(&self, key: &str, value: &str) -> Result<(), String>;
    fn erase(&self, key: &str) -> Result<(), String>;
}

/// The vault entry one server's credentials live under.
///
/// Namespaced, because the vault is shared with provider API keys and a server
/// called `anthropic` must not be able to overwrite one.
pub fn vault_key(server: &str) -> String {
    format!("mcp-oauth:{server}")
}

/// An `rmcp` credential store backed by a [`TokenVault`].
///
/// Everything `rmcp` wants to persist — the access token, the refresh token,
/// the client id it registered, the issuer — travels as one JSON blob under one
/// key. Storing the fields separately would mean a partial write leaving a
/// client id without the refresh token that goes with it, which fails at the
/// next launch rather than at the moment it happened.
struct VaultStore {
    vault: Arc<dyn TokenVault>,
    key: String,
}

#[async_trait::async_trait]
impl CredentialStore for VaultStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(raw) = self.vault.read(&self.key) else {
            return Ok(None);
        };
        // A blob that will not parse is treated as absent rather than fatal:
        // the format can change between versions, and the recovery — sign in
        // again — is one the user can perform and an error message is not.
        Ok(serde_json::from_str(&raw).ok())
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let raw = serde_json::to_string(&credentials)
            .map_err(|e| AuthError::InternalError(e.to_string()))?;
        self.vault
            .write(&self.key, &raw)
            .map_err(AuthError::InternalError)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.vault.erase(&self.key).map_err(AuthError::InternalError)
    }
}

/// Whether this server has credentials stored.
pub fn signed_in(vault: &Arc<dyn TokenVault>, server: &str) -> bool {
    vault.read(&vault_key(server)).is_some()
}

/// Forgets one server's credentials.
///
/// Local only, and the distinction is worth stating where somebody reads it:
/// this removes Taurus's copy, it does not revoke the grant. The authorization
/// server still lists the application until the user removes it there, which is
/// what the panel says when it offers this.
pub fn sign_out(vault: &Arc<dyn TokenVault>, server: &str) -> Result<(), String> {
    vault.erase(&vault_key(server))
}

/// A one-shot HTTP listener on loopback, for the redirect to come back to.
///
/// A desktop application has nowhere hosted to send a browser, and loopback is
/// what OAuth 2.1 permits a native client instead. Bound before the
/// authorization URL is built, because the port is part of the redirect URI and
/// the redirect URI is registered with the authorization server.
///
/// Port zero: the OS picks. A fixed port would collide with a second window, and
/// worse, would let anything else on the machine claim it first and receive an
/// authorization code meant for this process.
struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    async fn bind() -> Result<Self, String> {
        // 127.0.0.1 rather than 0.0.0.0. The difference is whether the rest of
        // the network can reach the socket the authorization code arrives on.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("could not listen for the browser to come back: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("could not read the callback port: {e}"))?
            .port();
        Ok(Self { listener, port })
    }

    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// Waits for the browser, and answers it with something readable.
    ///
    /// Returns the full callback URL, which is what `rmcp` parses the code,
    /// state and issuer out of. Only the request line is read: everything
    /// needed is in it, and reading headers would mean deciding what to do
    /// about a client that sends none.
    async fn wait(self) -> Result<String, String> {
        let accept = tokio::time::timeout(CALLBACK_TIMEOUT, self.listener.accept());
        let (stream, _) = accept
            .await
            .map_err(|_| "the browser did not come back within five minutes".to_string())?
            .map_err(|e| format!("the browser could not be answered: {e}"))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("could not read the callback: {e}"))?;

        // `GET /callback?code=…&state=… HTTP/1.1`
        let target = line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| "the browser sent something that was not a request".to_string())?;
        let url = format!("http://127.0.0.1:{}{target}", self.port);

        // Answered before the token exchange rather than after it, so the tab
        // stops spinning while the exchange happens. What the exchange does
        // with the code is reported in the window that started this, which is
        // where the user is going next.
        let page = "<!doctype html><meta charset=utf-8>\
            <title>Signed in</title>\
            <body style=\"font:16px system-ui;padding:3rem;max-width:32rem\">\
            <h1>Signed in</h1>\
            <p>You can close this tab and go back to Taurus.</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let mut stream = reader.into_inner();
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;

        Ok(url)
    }
}

/// A sign-in part way through: the URL to send the browser to, and the wait.
///
/// Split in two because the middle of it belongs to a layer that can reach the
/// desktop. This crate knows how to talk to an authorization server and has no
/// way to open a browser window; the application layer has the opposite pair.
pub struct SignIn {
    pub authorization_url: String,
    manager: AuthorizationManager,
    loopback: Loopback,
}

impl SignIn {
    /// Discovers, registers, and works out where to send the browser.
    ///
    /// `challenge` is the `WWW-Authenticate` header from the 401 that started
    /// this, when there was one. It carries the protected-resource metadata URL
    /// and the scopes this particular resource wants, both of which are better
    /// than anything that can be guessed from the server's URL alone — see the
    /// scope selection strategy in the MCP authorization specification.
    pub async fn begin(
        url: &str,
        server: &str,
        challenge: Option<&str>,
        vault: Arc<dyn TokenVault>,
    ) -> Result<Self, String> {
        let loopback = Loopback::bind().await?;
        let redirect_uri = loopback.redirect_uri();

        let mut manager = AuthorizationManager::new(url)
            .await
            .map_err(|e| describe(e, url))?;

        // The challenge first, because it names the resource metadata document
        // directly. Falling back to discovery from the base URL covers a server
        // that is reachable but has not been asked for anything yet.
        // `None` falls through to discovery from the base URL, which covers a
        // server reachable but not yet asked for anything.
        let resolution = manager
            .resolve_metadata_from_challenge(challenge)
            .await
            .map_err(|e| describe(e, url))?;
        // Refused where the metadata was synthesized rather than published.
        // `rmcp` keeps a legacy fallback that guesses endpoints from the base
        // URL, and guessing where to send somebody's credentials is not a thing
        // to do quietly.
        if !resolution.source.is_discovered() {
            return Err(format!(
                "{url} does not advertise an authorization server, so there is \
                 nothing to sign in to. If it takes a token instead, add it as a header."
            ));
        }

        let scopes = manager.get_current_scopes().await;
        let borrowed: Vec<&str> = scopes.iter().map(String::as_str).collect();

        // Dynamic registration, which is the only route open to a desktop
        // application. The specification now prefers a Client ID Metadata
        // Document — an https URL the authorization server fetches — and
        // Taurus has nowhere to host one, so this is the fallback the
        // specification keeps for exactly that case.
        manager
            .register_client(CLIENT_NAME, &redirect_uri, &borrowed)
            .await
            .map_err(|e| match e {
                AuthError::RegistrationFailed(_) => format!(
                    "{url} does not offer dynamic client registration, so Taurus \
                     cannot register itself. A server that issues personal access \
                     tokens can be added with an Authorization header instead."
                ),
                other => describe(other, url),
            })?;

        let authorization_url = manager
            .get_authorization_url(&borrowed)
            .await
            .map_err(|e| describe(e, url))?;

        // Attached only now: everything above can fail, and a store wired in
        // earlier would have nothing to write but would exist.
        manager.set_credential_store(VaultStore {
            vault,
            key: vault_key(server),
        });

        Ok(Self {
            authorization_url,
            manager,
            loopback,
        })
    }

    /// Waits for the browser, exchanges the code, and stores what comes back.
    ///
    /// The issuer check, the state check and PKCE are all `rmcp`'s, applied
    /// inside `handle_callback_url`. What this adds is the reason a failure is
    /// readable: an authorization server's own `error_description` is the most
    /// useful thing available and is passed through rather than flattened.
    pub async fn finish(self) -> Result<(), String> {
        let url = self.loopback.wait().await?;
        let callback = AuthorizationCallback::from_redirect_url(&url).map_err(|e| match e {
            AuthError::AuthorizationFailed(why) => {
                format!("the authorization server refused: {why}")
            }
            other => other.to_string(),
        })?;

        self.manager
            .exchange_code_for_token_with_issuer(
                &callback.code,
                &callback.csrf_token,
                callback.issuer.as_deref(),
            )
            .await
            .map(|_| ())
            .map_err(|e| match e {
                AuthError::AuthorizationFailed(why) => {
                    format!("the authorization server refused: {why}")
                }
                other => other.to_string(),
            })
    }
}

/// Turns an `rmcp` auth error into something worth reading in a panel.
fn describe(error: AuthError, url: &str) -> String {
    match error {
        AuthError::NoAuthorizationSupport | AuthError::MetadataError(_) => format!(
            "{url} does not advertise an authorization server, so there is nothing \
             to sign in to. If it takes a token instead, add it as a header."
        ),
        AuthError::PkceUnsupported => format!(
            "{url}'s authorization server does not support PKCE, which OAuth 2.1 \
             requires and Taurus will not go without."
        ),
        other => other.to_string(),
    }
}

/// An HTTP client that puts a fresh access token on every request.
///
/// Per request rather than per connection, and that is the whole reason this
/// exists rather than a header set once at connect time. An access token
/// commonly lives an hour and a server connection lives as long as the window
/// does, so a token minted at connect is one that expires mid-session — and the
/// failure it produces is a tool call that returns 401 in the middle of a turn.
/// `get_access_token` refreshes when it needs to, so asking it on every request
/// makes that impossible rather than unlikely.
///
/// The `auth_header` the transport passes in is discarded. It carries whatever
/// was configured statically, and for an OAuth server that is nothing.
#[derive(Clone)]
pub struct Authorized<C> {
    inner: C,
    manager: Arc<tokio::sync::Mutex<AuthorizationManager>>,
}

impl<C> Authorized<C> {
    pub fn new(inner: C, manager: AuthorizationManager) -> Self {
        Self {
            inner,
            manager: Arc::new(tokio::sync::Mutex::new(manager)),
        }
    }

    async fn bearer(&self) -> Option<String> {
        self.manager
            .lock()
            .await
            .get_access_token()
            .await
            .ok()
            .map(|token| format!("Bearer {token}"))
    }
}

impl<C: StreamableHttpClient + Clone + Send + Sync + 'static> StreamableHttpClient
    for Authorized<C>
{
    type Error = C::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let auth = self.bearer().await;
        self.inner
            .post_message(uri, message, session_id, auth, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let auth = self.bearer().await;
        self.inner
            .delete_session(uri, session_id, auth, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, StreamableHttpError<Self::Error>> {
        let auth = self.bearer().await;
        self.inner
            .get_stream(uri, session_id, last_event_id, auth, custom_headers)
            .await
    }
}

/// The manager a connection uses, rebuilt from what is in the vault.
///
/// Returns `None` when nothing is stored, which is the ordinary state of a
/// server that has never been signed in to and is what makes the panel say so
/// rather than fail the connection with a 401 nobody can act on.
pub async fn manager_for(
    url: &str,
    server: &str,
    vault: Arc<dyn TokenVault>,
) -> Option<AuthorizationManager> {
    if !signed_in(&vault, server) {
        return None;
    }
    let mut manager = AuthorizationManager::new(url).await.ok()?;
    manager.set_credential_store(VaultStore {
        vault,
        key: vault_key(server),
    });
    // Loads the stored client id and tokens. False means the store had nothing
    // usable, which after the check above means a blob that would not parse.
    manager.initialize_from_store().await.ok()?;
    Some(manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);

    impl TokenVault for Memory {
        fn read(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn write(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn erase(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn one_servers_credentials_cannot_overwrite_another_secret() {
        // The vault is shared with provider API keys, so an MCP server called
        // `anthropic` must not land on the key for the Anthropic provider.
        assert_eq!(vault_key("anthropic"), "mcp-oauth:anthropic");
        assert_ne!(vault_key("anthropic"), "anthropic");
    }

    #[test]
    fn signing_out_forgets_the_server_and_nothing_else() {
        let vault: Arc<dyn TokenVault> = Arc::new(Memory::default());
        vault.write(&vault_key("linear"), "{}").unwrap();
        vault.write(&vault_key("sentry"), "{}").unwrap();

        sign_out(&vault, "linear").unwrap();
        assert!(!signed_in(&vault, "linear"));
        assert!(signed_in(&vault, "sentry"));
    }

    #[test]
    fn signing_out_of_something_never_signed_in_to_succeeds() {
        // The caller asked for it to be gone, and it is.
        let vault: Arc<dyn TokenVault> = Arc::new(Memory::default());
        assert!(sign_out(&vault, "linear").is_ok());
    }

    #[tokio::test]
    async fn a_blob_that_will_not_parse_reads_as_nothing_stored() {
        // The format can change between versions, and the recovery — sign in
        // again — is one the user can perform where an error message is not.
        let vault: Arc<dyn TokenVault> = Arc::new(Memory::default());
        vault.write(&vault_key("linear"), "not json").unwrap();
        let store = VaultStore {
            vault: vault.clone(),
            key: vault_key("linear"),
        };
        assert!(store.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_callback_listens_only_on_loopback() {
        // The difference between this and 0.0.0.0 is whether the rest of the
        // network can reach the socket an authorization code arrives on.
        let loopback = Loopback::bind().await.unwrap();
        let addr = loopback.listener.local_addr().unwrap();
        assert!(addr.ip().is_loopback(), "{addr}");
        assert_ne!(addr.port(), 0, "the OS picks a real port");
        assert!(loopback.redirect_uri().starts_with("http://127.0.0.1:"));
    }

    #[tokio::test]
    async fn the_callback_url_is_rebuilt_from_the_request_line() {
        let loopback = Loopback::bind().await.unwrap();
        let port = loopback.port;
        let waiting = tokio::spawn(loopback.wait());

        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client
            .write_all(b"GET /callback?code=abc&state=xyz&iss=https%3A%2F%2Fa HTTP/1.1\r\n\r\n")
            .await
            .unwrap();

        let url = waiting.await.unwrap().unwrap();
        assert_eq!(
            url,
            format!("http://127.0.0.1:{port}/callback?code=abc&state=xyz&iss=https%3A%2F%2Fa")
        );
    }
}
