use super::*;
use std::path::Path;

#[test]
fn a_turn_in_the_folder_the_conversation_belongs_to_is_allowed() {
    assert!(check_workspace(Path::new("/src/a"), Path::new("/src/a")).is_ok());
}

#[test]
fn a_turn_from_another_folder_is_refused_and_told_where_to_go() {
    // Two checkouts of one project is the case the path has to be spelled
    // out for: "wrong workspace" would not say which of them.
    let err = check_workspace(Path::new("/src/a/taurus"), Path::new("/work/taurus"))
        .expect_err("a conversation must not be continued from another folder");
    assert!(err.contains("/src/a/taurus"), "{err}");
    assert!(
        !err.contains("/work/taurus"),
        "names the way back, not the dead end: {err}"
    );
}

fn open(model: &str) -> SessionEntry {
    SessionEntry {
        session: Arc::new(Mutex::new(Session::new(model))),
        provider_id: Mutex::new("local".into()),
        model: Mutex::new(model.into()),
        workspace: std::path::PathBuf::from("/src/a"),
        cancel: Arc::new(Mutex::new(CancellationToken::new())),
        log: Arc::new(Mutex::new(SessionLog::disabled())),
        live: Mutex::new(None),
        unattended: Arc::new(AtomicBool::new(false)),
        released: AtomicBool::new(false),
        switches: Mutex::new(Vec::new()),
    }
}

#[tokio::test]
async fn a_conversation_mid_turn_still_says_which_model_it_is_on() {
    // The first thing a turn review asks. A turn holds the session for its
    // whole run, so an answer read out of the session waits for all of it
    // — and "Review this turn" sat on "Reading it over…" until it ended.
    let entry = open("small-model");
    let _turn = entry.session.lock().await;

    let model = tokio::time::timeout(std::time::Duration::from_secs(2), session_model(&entry))
        .await
        .expect("waited for the running turn to finish");

    assert_eq!(model, "small-model");
}

#[tokio::test]
async fn usage_mid_turn_is_left_to_the_transcript_rather_than_waited_for() {
    let entry = open("m");
    let fixed = usage::Fixed::new("", Vec::new());

    let turn = entry.session.lock().await;
    assert!(
        open_usage(&entry, &fixed).is_none(),
        "a running turn's session is not readable, and must not be waited on"
    );

    drop(turn);
    assert!(
        open_usage(&entry, &fixed).is_some(),
        "between turns it answers from memory"
    );
}

#[tokio::test]
async fn what_cannot_happen_mid_turn_is_refused_and_told_what_to_do() {
    let entry = open("m");

    let turn = entry.session.lock().await;
    let err = entry
        .idle("stop it before rewinding")
        .expect_err("a rewind must not run underneath a turn");
    assert_eq!(
        err,
        "this conversation is mid-turn; stop it before rewinding"
    );

    drop(turn);
    assert!(entry.idle("stop it before rewinding").is_ok());
}

/// Leaving a conversation is not stopping the turn in it.
///
/// The rule the whole of long-running work rests on. Closing is what happens
/// on every switch between conversations, and while it cancelled, a task you
/// started and then looked away from was a task you had killed.
#[tokio::test]
async fn letting_go_of_a_conversation_mid_turn_waits_for_the_turn() {
    let entry = open("m");
    *entry.live.lock().await = Some(Arc::new(crate::live::Live::new()));

    assert_eq!(entry.release().await, Release::WhenTheTurnEnds);
    assert!(
        entry.was_released(),
        "the turn has to find out, or the conversation is held for the life of the process"
    );
}

#[tokio::test]
async fn letting_go_of_an_idle_conversation_is_immediate() {
    let entry = open("m");

    assert_eq!(entry.release().await, Release::Now);
    assert!(
        !entry.was_released(),
        "nothing is owed to a turn that is not running"
    );
}

/// Coming back to a conversation before its turn ends must un-say the release,
/// or the turn ending reaps an entry the window is using.
#[tokio::test]
async fn reopening_a_released_conversation_keeps_it() {
    let entry = open("m");
    *entry.live.lock().await = Some(Arc::new(crate::live::Live::new()));
    entry.release().await;

    entry.keep();

    assert!(!entry.was_released());
}
