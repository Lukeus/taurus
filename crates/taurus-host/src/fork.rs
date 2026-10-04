//! Trying a turn again without losing the first attempt.
//!
//! A rewind undoes turns and throws them away. A fork keeps them: it starts a
//! new conversation from just before a turn, puts the workspace back the way
//! it was at that point, and leaves the original exactly as it was, so either
//! one can be picked up again later. That's the answer to the truncating edit
//! `docs/known-gaps.md` turned down: nothing is truncated, and a mis-click
//! costs a conversation in the rail, not an hour of work.
//!
//! # What a fork is
//!
//! A copy of the start of both logs: the transcript up to the turn, and the
//! checkpoint log's turns before it. A copy rather than a reference, so a fork
//! is an ordinary conversation in every reader there is. Its turns number,
//! list, diff and rewind as the original's did, back past the point where the
//! two part, and it opens in a build that has never heard of forks. Its header
//! says where it came from, which is what the rail shows.
//!
//! # One set of files
//!
//! A workspace has one copy of each file, so only one branch of a conversation
//! can be on disk at a time. The checkpoint log records what a file held
//! *before* each turn, which is enough to go back but not forward. So leaving
//! a branch keeps its files: a set-aside, in that branch's own log, of what
//! every path about to be written holds right then, hand edits included.
//! Entering a branch writes back what its set-aside kept, and for a path it
//! never kept, the first pre-image the branch being left has of it. That path
//! is one only the leaving branch changed, so before that change is what the
//! other branch has.
//!
//! That works for any tree of forks with no search for a common ancestor, and
//! costs nothing per turn: the price is paid when somebody switches. A file
//! that can't be set aside (not text, unreadable) is never written over, and
//! the plan says so. Ignored directories and `.git` are shared between
//! branches, the same as they're outside a rewind.
//!
//! A turn can't start in a conversation whose files are set aside. The model
//! would be reading another branch's files and taking them for its own, which
//! is never what anybody means. See [`ensure_on_disk`].

use std::path::Path;

use serde::Serialize;
use taurus_tools::checkpoint::{apply, Held, State};
use taurus_tools::{CheckpointStore, Restored};
use ts_rs::TS;

use crate::sessions::{self, ForkedFrom};

/// What a fork did, or would do.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Forked {
    /// The new conversation. `None` for a dry run, which makes nothing.
    #[ts(optional)]
    pub id: Option<String>,
    /// The turn it was forked before, as `taurus rewind` numbers it.
    pub turn: u32,
    /// What that turn asked, to be asked again or asked differently.
    pub prompt: String,
    /// What putting the files back did to each, as a rewind reports it.
    pub restored: Vec<Restored>,
    /// What it can't put back. The same sentences a rewind to the same turn
    /// gives, because the files it writes are the same.
    pub warnings: Vec<String>,
}

/// What a switch did, or would do.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Switched {
    /// The conversation whose files were set aside to make room. `None` when
    /// none was on disk, which is a switch finishing one a crash interrupted.
    #[ts(optional)]
    pub from: Option<String>,
    pub restored: Vec<Restored>,
}

/// Forks `source` at checkpointed turn `turn`: a new conversation holding
/// everything before it, with the workspace put back to that point and
/// `source`'s files set aside.
pub fn fork(
    store: &CheckpointStore,
    workspace: &Path,
    source: &str,
    turn: u32,
    dry_run: bool,
) -> Result<Forked, String> {
    ensure_on_disk(store, source)?;
    let ids = store.turn_ids(source)?;
    let listed = store.turns(source)?;
    let checkpoint = listed.get((turn as usize).wrapping_sub(1)).ok_or_else(|| {
        format!(
            "session '{source}' has {} checkpointed turn{}; {turn} is not one of them",
            listed.len(),
            if listed.len() == 1 { "" } else { "s" }
        )
    })?;
    // Every turn's id, not only this one's: the copied prefix is matched to
    // the transcript by them, and one missing would leave nothing to match.
    if ids.iter().any(Option::is_none) {
        return Err(format!(
            "session '{source}' was recorded before Taurus named its turns, so its \
             checkpoints can't be matched to its transcript and it can't be forked. \
             `taurus rewind` still works on it."
        ));
    }
    let turn_id = ids[turn as usize - 1].clone().expect("checked just above");

    let plan = store.rewind_plan(source, turn)?;
    let warnings = store.rewind(source, workspace, turn, true)?.warnings;

    if dry_run {
        let tips: Vec<Held> = plan
            .iter()
            .map(|held| Held {
                path: held.path.clone(),
                state: now(workspace, &held.path),
            })
            .collect();
        return Ok(Forked {
            id: None,
            turn,
            prompt: checkpoint.prompt.clone(),
            restored: write(workspace, &plan, &tips, true),
            warnings,
        });
    }

    // Harmless steps first: a new transcript and log nobody points at yet. If
    // either fails, nothing that existed has changed.
    let id = uuid::Uuid::new_v4().to_string();
    let prompt = sessions::fork_transcript(
        &id,
        &ForkedFrom {
            session: source.to_string(),
            turn: turn_id,
            checkpoint: turn,
        },
    )?;
    if let Err(e) = store.copy_prefix(source, &id, turn as usize - 1, workspace) {
        let _ = sessions::delete(&id);
        return Err(e);
    }

    // Then the one that matters: the source's files, kept, before anything is
    // written over them.
    let touched: Vec<String> = store
        .first_preimages(source)?
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let kept = match store.set_aside(source, workspace, &touched, &id) {
        Ok(kept) => kept,
        Err(e) => {
            let _ = sessions::delete(&id);
            let _ = store.forget(&id);
            return Err(e);
        }
    };

    crate::review::copy_reviews(workspace, source, &id);
    Ok(Forked {
        id: Some(id),
        turn,
        prompt,
        restored: write(workspace, &plan, &kept, false),
        warnings,
    })
}

/// Puts `target`'s files back on disk, setting aside whichever branch of the
/// same conversation is there now.
pub fn switch(
    store: &CheckpointStore,
    workspace: &Path,
    target: &str,
    dry_run: bool,
) -> Result<Switched, String> {
    let Some(aside) = store.aside(target)? else {
        return Err(format!(
            "session '{target}' already has its files in the workspace; there's nothing to \
             switch to"
        ));
    };

    let family = family(workspace, target);
    let mut on_disk = Vec::new();
    for member in family.iter().filter(|m| m.as_str() != target) {
        if store.away(member)?.is_none() {
            on_disk.push(member.clone());
        }
    }
    let current = match on_disk.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        many => {
            return Err(format!(
                "more than one branch of this conversation says its files are on disk ({}), \
                 so there's no telling which files to keep. Switch to one of them first.",
                many.join(", ")
            ))
        }
    };

    // Every path either side has something to say about.
    let firsts = match &current {
        Some(current) => store.first_preimages(current)?,
        None => Vec::new(),
    };
    let mut paths: Vec<String> = firsts.iter().map(|(path, _)| path.clone()).collect();
    for held in &aside {
        if !paths.contains(&held.path) {
            paths.push(held.path.clone());
        }
    }
    paths.sort();

    // What the target has at each: its own kept copy, or else what was there
    // before the branch being left first changed it.
    let wanted: Vec<Held> = paths
        .iter()
        .filter_map(|path| {
            aside
                .iter()
                .find(|held| held.path == *path)
                .map(|held| held.state.clone())
                .or_else(|| {
                    firsts
                        .iter()
                        .find(|(seen, _)| seen == path)
                        .map(|(_, state)| state.clone())
                })
                .map(|state| Held {
                    path: path.clone(),
                    state,
                })
        })
        .collect();

    let tips: Vec<Held> = match (&current, dry_run) {
        (Some(current), false) => store.set_aside(current, workspace, &paths, target)?,
        // A dry run reads what a set-aside would keep, without keeping it.
        _ => paths
            .iter()
            .map(|path| Held {
                path: path.clone(),
                state: now(workspace, path),
            })
            .collect(),
    };
    let restored = write(workspace, &wanted, &tips, dry_run);
    if !dry_run {
        store.mark_entered(target, workspace)?;
    }
    Ok(Switched {
        from: current,
        restored,
    })
}

/// Refuses a turn in a conversation whose files are set aside, naming the
/// way out.
pub fn ensure_on_disk(store: &CheckpointStore, session: &str) -> Result<(), String> {
    match store.away(session)? {
        None => Ok(()),
        Some(other) => Err(format!(
            "this conversation's files were set aside when another branch of it ({other}) \
             took the workspace, so its model would be reading files that aren't its own. \
             Switch to it first: `taurus fork --switch --id {session}`, or Switch in the app."
        )),
    }
}

/// Writes `wanted`, except over a file whose current contents couldn't be
/// kept. `tips` is what each path held, as the set-aside recorded it.
fn write(workspace: &Path, wanted: &[Held], tips: &[Held], dry_run: bool) -> Vec<Restored> {
    let mut writable = Vec::new();
    let mut skipped = Vec::new();
    for held in wanted {
        match tips
            .iter()
            .find(|tip| tip.path == held.path)
            .map(|tip| &tip.state)
        {
            Some(State::Opaque { reason }) => skipped.push(Restored::Skipped {
                path: held.path.clone(),
                reason: format!("{reason}, so it couldn't be kept and was left as it is"),
            }),
            _ => writable.push(held.clone()),
        }
    }
    let mut restored = apply(workspace, &writable, dry_run);
    restored.extend(skipped);
    restored.sort_by(|a, b| a.path().cmp(b.path()));
    restored
}

/// What a path holds right now, as a set-aside would record it.
fn now(workspace: &Path, path: &str) -> State {
    match taurus_tools::path_guard::resolve(workspace, path) {
        Ok(full) => match std::fs::read_to_string(&full) {
            Ok(content) => State::Text { content },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::Absent,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => State::Opaque {
                reason: "isn't text".into(),
            },
            Err(e) => State::Opaque {
                reason: format!("can't be read ({e})"),
            },
        },
        Err(e) => State::Opaque {
            reason: e.to_string(),
        },
    }
}

/// Every conversation in `workspace` that shares a root with `id`: the one
/// everything was forked from, and every fork of it, however deep.
fn family(workspace: &Path, id: &str) -> Vec<String> {
    let metas = sessions::list(Some(workspace));
    let parent_of = |id: &str| -> Option<String> {
        metas
            .iter()
            .find(|meta| meta.id == id)
            .and_then(|meta| meta.forked_from.as_ref())
            .map(|from| from.session.clone())
    };
    let root_of = |start: &str| -> String {
        let mut at = start.to_string();
        // Bounded, so a hand-edited loop of headers can't hang a switch.
        for _ in 0..metas.len() + 1 {
            match parent_of(&at) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        at
    };
    let root = root_of(id);
    let mut members: Vec<String> = metas
        .iter()
        .filter(|meta| root_of(&meta.id) == root)
        .map(|meta| meta.id.clone())
        .collect();
    if !members.contains(&root) {
        members.push(root);
    }
    members
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionLog;
    use crate::testing::{isolated_home, HomeGuard};
    use std::path::PathBuf;
    use taurus_core::Session;
    use taurus_provider::Message;
    use tempfile::TempDir;

    /// A workspace, a conversation in it, and its checkpoint log, under a
    /// home of its own.
    struct World {
        _home: HomeGuard,
        _dir: TempDir,
        ws: PathBuf,
        store: CheckpointStore,
    }

    impl World {
        fn new() -> Self {
            let home = isolated_home();
            let dir = TempDir::new().unwrap();
            let ws = dir.path().canonicalize().unwrap();
            Self {
                store: CheckpointStore::new(sessions::checkpoints_dir(&ws)),
                _home: home,
                _dir: dir,
                ws,
            }
        }

        fn read(&self, file: &str) -> Option<String> {
            std::fs::read_to_string(self.ws.join(file)).ok()
        }

        fn write(&self, file: &str, content: &str) {
            std::fs::write(self.ws.join(file), content).unwrap();
        }
    }

    /// One conversation's transcript, written as the app writes it.
    struct Talk {
        session: Session,
        log: SessionLog,
    }

    impl Talk {
        fn start(world: &World) -> Self {
            let session = Session::new("test-model");
            let log = SessionLog::create(&session, &world.ws, None);
            Self { session, log }
        }

        fn resume(id: &str) -> Self {
            let loaded = sessions::load(id).unwrap();
            Self {
                log: SessionLog::resume(&loaded),
                session: loaded.session,
            }
        }

        /// A turn that asks `prompt` and sets each file to its content, or
        /// deletes it for `None`.
        async fn turn(
            &mut self,
            world: &World,
            id: &str,
            prompt: &str,
            writes: &[(&str, Option<&str>)],
        ) {
            self.log.start_turn(id, false);
            self.session.messages.push(Message::user(prompt));
            self.session.messages.push(Message::assistant("done"));
            self.log.record(&self.session);
            self.log.end_turn(id, "finished");
            let recorder = world.store.begin_turn(&self.session.id, &world.ws, prompt);
            recorder.name(id).await;
            for (file, content) in writes {
                let path = world.ws.join(file);
                recorder.capture(&path).await;
                match content {
                    Some(text) => std::fs::write(&path, text).unwrap(),
                    None => {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }

    async fn three_turns(world: &World) -> Talk {
        let mut talk = Talk::start(world);
        talk.turn(world, "t1", "make a", &[("a.txt", Some("one"))])
            .await;
        talk.turn(
            world,
            "t2",
            "change a, add x",
            &[("a.txt", Some("two")), ("x.txt", Some("x2"))],
        )
        .await;
        talk.turn(world, "t3", "change a again", &[("a.txt", Some("three"))])
            .await;
        talk
    }

    #[tokio::test]
    async fn a_fork_keeps_the_original_and_puts_the_files_back_to_before_the_turn() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();

        let forked = fork(&world.store, &world.ws, &original, 2, false).unwrap();
        let id = forked.id.clone().unwrap();
        assert_eq!(forked.prompt, "change a, add x");
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));
        assert_eq!(world.read("x.txt"), None, "x.txt was made by turn 2");

        // The fork holds turn 1 and nothing after it, and says where it came from.
        let loaded = sessions::load(&id).unwrap();
        let said: Vec<String> = loaded.session.messages.iter().map(|m| m.text()).collect();
        assert_eq!(said, ["make a", "done"]);
        let meta = sessions::meta(&id).unwrap();
        assert_eq!(
            meta.forked_from,
            Some(ForkedFrom {
                session: original.clone(),
                turn: "t2".into(),
                checkpoint: 2
            })
        );
        assert_eq!(
            world.store.turns(&id).unwrap().len(),
            1,
            "turn 1's checkpoint came too"
        );

        // The original is untouched, and set aside.
        assert_eq!(sessions::load(&original).unwrap().session.messages.len(), 6);
        assert_eq!(world.store.turns(&original).unwrap().len(), 3);
        assert_eq!(world.store.away(&original).unwrap(), Some(id.clone()));
        assert!(ensure_on_disk(&world.store, &id).is_ok());
        let refused = ensure_on_disk(&world.store, &original).unwrap_err();
        assert!(
            refused.contains(&format!("taurus fork --switch --id {original}")),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn switching_puts_each_branch_back_with_its_own_files_and_your_edits() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        let id = fork(&world.store, &world.ws, &original, 2, false)
            .unwrap()
            .id
            .unwrap();

        // The fork goes its own way: a different a.txt, and a file of its own.
        let mut branch = Talk::resume(&id);
        branch
            .turn(
                &world,
                "f2",
                "try it differently",
                &[("a.txt", Some("other")), ("b.txt", Some("b"))],
            )
            .await;
        // And a hand edit to a file only the original's later turns made.
        world.write("x.txt", "typed by hand");

        let back = switch(&world.store, &world.ws, &original, false).unwrap();
        assert_eq!(back.from.as_deref(), Some(id.as_str()));
        assert_eq!(world.read("a.txt").as_deref(), Some("three"));
        assert_eq!(world.read("x.txt").as_deref(), Some("x2"));
        assert_eq!(
            world.read("b.txt"),
            None,
            "the fork's own file goes with it"
        );
        assert_eq!(world.store.away(&original).unwrap(), None);
        assert_eq!(world.store.away(&id).unwrap(), Some(original.clone()));

        // And back again: the fork's files, and the hand edit that was made on it.
        switch(&world.store, &world.ws, &id, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("other"));
        assert_eq!(world.read("b.txt").as_deref(), Some("b"));
        assert_eq!(world.read("x.txt").as_deref(), Some("typed by hand"));
    }

    #[tokio::test]
    async fn a_fork_of_a_fork_switches_back_to_the_first_one_directly() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        let first = fork(&world.store, &world.ws, &original, 3, false)
            .unwrap()
            .id
            .unwrap();
        let mut branch = Talk::resume(&first);
        branch
            .turn(&world, "f3", "first fork's turn", &[("a.txt", Some("f"))])
            .await;
        let second = fork(&world.store, &world.ws, &first, 3, false)
            .unwrap()
            .id
            .unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("two"));

        switch(&world.store, &world.ws, &original, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("three"));
        switch(&world.store, &world.ws, &first, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("f"));
        switch(&world.store, &world.ws, &second, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("two"));
    }

    #[tokio::test]
    async fn a_dry_run_says_what_would_change_and_changes_nothing() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        let before = sessions::list(Some(&world.ws)).len();

        let plan = fork(&world.store, &world.ws, &original, 2, true).unwrap();
        assert!(plan.id.is_none());
        assert_eq!(plan.restored.len(), 2, "{:?}", plan.restored);
        assert_eq!(world.read("a.txt").as_deref(), Some("three"));
        assert_eq!(sessions::list(Some(&world.ws)).len(), before);
        assert_eq!(world.store.away(&original).unwrap(), None);
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_kept_is_left_alone() {
        let world = World::new();
        let mut talk = Talk::start(&world);
        talk.turn(&world, "t1", "make a", &[("a.txt", Some("one"))])
            .await;
        talk.turn(&world, "t2", "change a", &[("a.txt", Some("two"))])
            .await;
        // Not text any more, so a set-aside can't hold it.
        std::fs::write(world.ws.join("a.txt"), [0xff, 0xfe, 0x00]).unwrap();

        let forked = fork(&world.store, &world.ws, &talk.session.id, 2, false).unwrap();
        assert!(
            matches!(&forked.restored[0], Restored::Skipped { reason, .. } if reason.contains("left as it is"))
        );
        assert_eq!(
            std::fs::read(world.ws.join("a.txt")).unwrap(),
            [0xff, 0xfe, 0x00]
        );
    }

    #[tokio::test]
    async fn switching_to_a_conversation_already_on_disk_says_so() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let err = switch(&world.store, &world.ws, &talk.session.id, false).unwrap_err();
        assert!(
            err.contains("already has its files in the workspace"),
            "{err}"
        );
    }
}
