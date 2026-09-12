//! The span a model request runs in, checked in a binary of its own.
//!
//! Alone because the subscriber here is thread-local and the span's callsite
//! is shared. With the other agent tests running beside it, a thread with no
//! subscriber can register the chat span's callsite at the moment this one
//! installs its own, and tracing caches that callsite as uninteresting: the
//! span is then disabled for this test and every event reads as outside it.
//! `taurus-host`'s `tests/spans.rs` is alone for the same reason.

use std::sync::Arc;

use taurus_core::testing::{FakeProvider, ScriptedTurn};
use taurus_core::{Agent, AgentConfig, Session};
use taurus_provider::{Message, StopReason, StreamEvent};
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A backend that logs while it streams, the way an adapter does about a
/// record it cannot parse, and then waits for `go` before answering from the
/// script it wraps.
struct Logging {
    inner: Arc<FakeProvider>,
    started: Arc<tokio::sync::Notify>,
    go: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl taurus_provider::Provider for Logging {
    fn id(&self) -> &str {
        self.inner.id()
    }

    async fn models(&self) -> taurus_provider::Result<Vec<taurus_provider::ModelInfo>> {
        self.inner.models().await
    }

    async fn capabilities(
        &self,
        model: &str,
    ) -> taurus_provider::Result<taurus_provider::Capabilities> {
        self.inner.capabilities(model).await
    }

    async fn stream(
        &self,
        request: taurus_provider::ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> taurus_provider::Result<StopReason> {
        tracing::warn!(target: "adapter", "streaming");
        self.started.notify_one();
        self.go.notified().await;
        self.inner.stream(request, tx, cancel).await
    }
}

/// Each test event's target, and the name of the span it was logged in.
type Seen = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// The span each test event was logged inside, by the event's target.
#[derive(Clone, Default)]
struct SpanOfEvent(Seen);

impl<S> tracing_subscriber::Layer<S> for SpanOfEvent
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let target = event.metadata().target();
        if target == "adapter" || target == "bystander" {
            let span = ctx.event_span(event).map(|s| s.name().to_string());
            self.0.lock().unwrap().push((target.to_string(), span));
        }
    }
}

#[tokio::test]
async fn a_request_span_covers_its_own_work_and_nothing_else() {
    // The chat span was entered with a guard held across every await of the
    // request. On a thread shared with other tasks that leaves it entered
    // while they run, so a task with nothing to do with the model logged
    // inside it — and the provider's own task, spawned without it, did not.
    use tracing_subscriber::layer::SubscriberExt;
    let seen = SpanOfEvent::default();
    let _default =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(seen.clone()));

    let started = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    // Runs while the request waits on the backend, and belongs to no request.
    let bystander = tokio::spawn({
        let (started, go) = (started.clone(), go.clone());
        async move {
            started.notified().await;
            tracing::warn!(target: "bystander", "unrelated work");
            go.notify_one();
        }
    });

    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let agent = Agent::new(
        Arc::new(Logging {
            inner: FakeProvider::new(vec![ScriptedTurn::text("Hello.")]),
            started,
            go,
        }),
        ToolRegistry::with_builtins(),
        ToolContext::new(workspace, permissions, CancellationToken::new()),
        AgentConfig::default(),
    );
    let (tx, mut rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let mut session = Session::new("fake");
    agent
        .run_turn(&mut session, Message::user("hi"), tx)
        .await
        .expect("the turn finishes");
    drain.await.unwrap();
    bystander.await.unwrap();

    let seen = seen.0.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            ("adapter".to_string(), Some("chat".to_string())),
            ("bystander".to_string(), None),
        ]
    );
}
