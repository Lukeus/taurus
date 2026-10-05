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
use taurus_provider::TokenUsage;
use taurus_tools::checkpoint::{apply, Held, State};
use taurus_tools::{Checkpoint, CheckpointStore, Restored, TurnChange};
use ts_rs::TS;

use crate::review::ReviewReport;
use crate::sessions::{self, ForkedFrom};

/// What a fork did, or would do.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Forked {
    /// The new conversation. `None` for a dry run, which makes nothing.
    #[ts(optional)]
    pub id: Option<String>,
    /// The checkpointed turn whose files it went back to before, as `taurus
    /// rewind` numbers it. One past the last when no turn from the fork point
    /// on changed files.
    pub turn: u32,
    /// The turn it was forked at changed no files, so `turn` is the next one
    /// that did. See [`ForkedFrom::place`].
    pub read_only: bool,
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
///
/// The numbering `taurus rewind` uses. The turn is matched to its transcript
/// by the id both logs record, and forked from there with [`fork_before`].
pub fn fork(
    store: &CheckpointStore,
    workspace: &Path,
    source: &str,
    turn: u32,
    dry_run: bool,
) -> Result<Forked, String> {
    let ids = store.turn_ids(source)?;
    let id = ids.get((turn as usize).wrapping_sub(1)).ok_or_else(|| {
        format!(
            "session '{source}' has {} checkpointed turn{}; {turn} is not one of them",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        )
    })?;
    let Some(id) = id else {
        return Err(unnamed(source));
    };
    fork_before(store, workspace, source, &id.clone(), dry_run)
}

/// Forks `source` before the transcript turn `turn`, which may be one that
/// changed no files: the question that's to be asked differently is often
/// one that only read.
///
/// The checkpoint log is cut where the transcript is. Its turns before the
/// fork point come along, and the files go back to before the first one at or
/// after it. When no turn from there on changed anything, the files are
/// already right and none are written, but `source` is still set aside: two
/// branches never both have the workspace, even for a moment when their files
/// agree.
pub fn fork_before(
    store: &CheckpointStore,
    workspace: &Path,
    source: &str,
    turn: &str,
    dry_run: bool,
) -> Result<Forked, String> {
    ensure_on_disk(store, source)?;
    let point = sessions::fork_point(source, turn)?;

    // Every checkpoint's id, not only the ones after the cut: the copied
    // prefix is matched to the transcript by them, and one missing would leave
    // nothing to match.
    let ids = store.turn_ids(source)?;
    if ids.iter().any(Option::is_none) {
        return Err(unnamed(source));
    }
    let ids: Vec<String> = ids.into_iter().flatten().collect();
    let kept = ids
        .iter()
        .take_while(|id| point.before.contains(id))
        .count();
    if ids[kept..].iter().any(|id| point.before.contains(id)) {
        return Err(format!(
            "session '{source}' has checkpoints out of step with its transcript, so there's no \
             telling which files go with the fork. `taurus rewind` still works on it."
        ));
    }
    let checkpoint = kept as u32 + 1;
    let read_only = ids.get(kept) != Some(&point.turn);

    let (plan, warnings) = if kept < ids.len() {
        (
            store.rewind_plan(source, checkpoint)?,
            store.rewind(source, workspace, checkpoint, true)?.warnings,
        )
    } else {
        (Vec::new(), Vec::new())
    };

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
            turn: checkpoint,
            read_only,
            prompt: point.prompt,
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
            turn: point.turn,
            checkpoint,
            read_only: read_only.then_some(true),
        },
    )?;
    if let Err(e) = store.copy_prefix(source, &id, kept, workspace) {
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

    crate::review::copy_reviews(workspace, source, &id, checkpoint);
    Ok(Forked {
        id: Some(id),
        turn: checkpoint,
        read_only,
        prompt,
        restored: write(workspace, &plan, &kept, false),
        warnings,
    })
}

/// Why a conversation can't be forked: its checkpoints have no ids to match
/// them to its transcript by.
fn unnamed(source: &str) -> String {
    format!(
        "session '{source}' was recorded before Taurus named its turns, so its \
         checkpoints can't be matched to its transcript and it can't be forked. \
         `taurus rewind` still works on it."
    )
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

/// Two branches of one conversation, side by side.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Comparison {
    /// Questions both asked before they parted.
    pub shared: u32,
    pub a: Side,
    pub b: Side,
    /// Every file whose latest state differs between the two, as a diff from
    /// `a`'s to `b`'s. A file both left the same isn't here.
    pub files: Vec<TurnChange>,
}

/// One branch, from where it parted from the other.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Side {
    pub id: String,
    /// Its files are the ones in the workspace.
    pub on_disk: bool,
    /// What it asked since they parted, oldest first.
    pub asked: Vec<String>,
    /// Its turns since they parted that changed files, numbered as its own
    /// Changes panel numbers them.
    pub turns: Vec<Checkpoint>,
    /// What it spent since they parted: its running total now, less the total
    /// both had at the point they parted.
    pub usage: TokenUsage,
    /// The latest review of each of `turns` that has one.
    pub reviews: Vec<ReviewReport>,
}

/// Compares two branches of one conversation without switching to either:
/// what each did since they parted, and how their files differ now.
///
/// Each branch's files are read where they are. The one in the workspace is
/// on disk. One set aside is in its set-aside, and for a path it never kept,
/// it has what a switch to it would write: the first pre-image of the branch
/// on disk, or, for a path nobody's branch touched, what's on disk. That's
/// the same rule [`switch`] follows, so a comparison shows exactly what
/// switching would put in front of you.
pub fn compare(
    store: &CheckpointStore,
    workspace: &Path,
    a: &str,
    b: &str,
) -> Result<Comparison, String> {
    if a == b {
        return Err(format!(
            "'{a}' is the same conversation twice; name another branch of it"
        ));
    }
    let family = family(workspace, a);
    if !family.iter().any(|member| member == b) {
        return Err(format!(
            "'{a}' and '{b}' aren't branches of the same conversation, so there's no point \
             where they parted to compare from. `taurus sessions` marks which conversations \
             are forks of which."
        ));
    }
    let (outline_a, outline_b) = (sessions::outline(a)?, sessions::outline(b)?);
    let shared = outline_a
        .turns
        .iter()
        .zip(&outline_b.turns)
        .take_while(|(x, y)| x.id == y.id)
        .count();
    let shared_ids: Vec<&str> = outline_a.turns[..shared]
        .iter()
        .map(|turn| turn.id.as_str())
        .collect();
    let parted_usage = outline_a
        .turns
        .get(shared)
        .or(outline_b.turns.get(shared))
        .map(|turn| turn.usage_before)
        .unwrap_or(outline_a.usage);

    // The branch whose files are on disk, which may be neither of these two.
    let mut current = None;
    for member in &family {
        if store.away(member)?.is_none() {
            current = Some(member.clone());
            break;
        }
    }
    let current_firsts = match &current {
        Some(current) => store.first_preimages(current)?,
        None => Vec::new(),
    };

    let side = |id: &str, outline: &sessions::Outline| -> Result<(Side, Vec<Held>), String> {
        let ids = store.turn_ids(id)?;
        let turns: Vec<Checkpoint> = store
            .turns(id)?
            .into_iter()
            .zip(ids)
            .filter(|(_, turn_id)| {
                !turn_id
                    .as_deref()
                    .is_some_and(|turn_id| shared_ids.contains(&turn_id))
            })
            .map(|(checkpoint, _)| checkpoint)
            .collect();
        let aside = store.aside(id)?;
        let on_disk = aside.is_none();
        let mut paths: Vec<String> = store
            .first_preimages(id)?
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        for held in aside.iter().flatten() {
            if !paths.contains(&held.path) {
                paths.push(held.path.clone());
            }
        }
        let tip = |path: &String| -> State {
            if on_disk {
                return now(workspace, path);
            }
            aside
                .iter()
                .flatten()
                .find(|held| held.path == *path)
                .map(|held| held.state.clone())
                .or_else(|| {
                    current_firsts
                        .iter()
                        .find(|(seen, _)| seen == path)
                        .map(|(_, state)| state.clone())
                })
                .unwrap_or_else(|| now(workspace, path))
        };
        let held = paths
            .iter()
            .map(|path| Held {
                path: path.clone(),
                state: tip(path),
            })
            .collect();
        Ok((
            Side {
                id: id.to_string(),
                on_disk,
                asked: outline.turns[shared.min(outline.turns.len())..]
                    .iter()
                    .filter(|turn| !turn.continues && !turn.asked.is_empty())
                    .map(|turn| turn.asked.clone())
                    .collect(),
                reviews: crate::review::kept(workspace, id, &turns),
                turns,
                usage: spent(outline.usage, parted_usage),
            },
            held,
        ))
    };
    let (side_a, mut held_a) = side(a, &outline_a)?;
    let (side_b, mut held_b) = side(b, &outline_b)?;

    // A path only one side has a word about is what the other side would get
    // from the same rule, read for it.
    let mut paths: Vec<String> = held_a
        .iter()
        .chain(&held_b)
        .map(|h| h.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    for path in &paths {
        let state_of = |held: &mut Vec<Held>, side: &Side| -> State {
            if let Some(found) = held.iter().find(|h| h.path == *path) {
                return found.state.clone();
            }
            let state = if side.on_disk {
                now(workspace, path)
            } else {
                current_firsts
                    .iter()
                    .find(|(seen, _)| seen == path)
                    .map(|(_, state)| state.clone())
                    .unwrap_or_else(|| now(workspace, path))
            };
            held.push(Held {
                path: path.clone(),
                state: state.clone(),
            });
            state
        };
        let (from, to) = (
            state_of(&mut held_a, &side_a),
            state_of(&mut held_b, &side_b),
        );
        if from != to {
            files.push(CheckpointStore::compare(path, &from, &to));
        }
    }

    Ok(Comparison {
        shared: outline_a.turns[..shared]
            .iter()
            .filter(|turn| !turn.continues)
            .count() as u32,
        a: side_a,
        b: side_b,
        files,
    })
}

/// What was spent between two running totals.
fn spent(now: TokenUsage, then: TokenUsage) -> TokenUsage {
    let less =
        |now: Option<u32>, then: Option<u32>| now.map(|n| n.saturating_sub(then.unwrap_or(0)));
    TokenUsage {
        input_tokens: now.input_tokens.saturating_sub(then.input_tokens),
        output_tokens: now.output_tokens.saturating_sub(then.output_tokens),
        cache_read_input_tokens: less(now.cache_read_input_tokens, then.cache_read_input_tokens),
        cache_creation_input_tokens: less(
            now.cache_creation_input_tokens,
            then.cache_creation_input_tokens,
        ),
        reasoning_tokens: less(now.reasoning_tokens, then.reasoning_tokens),
    }
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
            self.turn_as(world, id, false, prompt, writes).await;
        }

        /// [`Talk::turn`], continuing the one before when `continues`.
        async fn turn_as(
            &mut self,
            world: &World,
            id: &str,
            continues: bool,
            prompt: &str,
            writes: &[(&str, Option<&str>)],
        ) {
            self.log.start_turn(id, continues);
            // A hundred in and ten out per turn, so what a branch spent can
            // be counted from the running totals.
            self.session.usage.input_tokens += 100;
            self.session.usage.output_tokens += 10;
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
                checkpoint: 2,
                read_only: None,
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
    async fn a_fork_at_a_turn_that_changed_nothing_puts_back_the_files_from_after_it() {
        let world = World::new();
        let mut talk = Talk::start(&world);
        talk.turn(&world, "t1", "make a", &[("a.txt", Some("one"))])
            .await;
        talk.turn(&world, "t2", "what does a say?", &[]).await;
        talk.turn(&world, "t3", "change a", &[("a.txt", Some("three"))])
            .await;
        let original = talk.session.id.clone();

        let forked = fork_before(&world.store, &world.ws, &original, "t2", false).unwrap();
        let id = forked.id.clone().unwrap();
        assert_eq!(forked.prompt, "what does a say?");
        assert_eq!((forked.turn, forked.read_only), (2, true));
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));

        let said: Vec<String> = sessions::load(&id)
            .unwrap()
            .session
            .messages
            .iter()
            .map(|m| m.text())
            .collect();
        assert_eq!(said, ["make a", "done"]);
        assert_eq!(world.store.turns(&id).unwrap().len(), 1);
        let from = sessions::meta(&id).unwrap().forked_from.unwrap();
        assert_eq!(from.place(), "after turn 1");
        assert_eq!(world.store.away(&original).unwrap(), Some(id));
    }

    #[tokio::test]
    async fn a_fork_after_the_last_change_writes_nothing_but_still_sets_the_original_aside() {
        let world = World::new();
        let mut talk = Talk::start(&world);
        talk.turn(&world, "t1", "make a", &[("a.txt", Some("one"))])
            .await;
        talk.turn(&world, "t2", "explain it", &[]).await;
        let original = talk.session.id.clone();

        let forked = fork_before(&world.store, &world.ws, &original, "t2", false).unwrap();
        let id = forked.id.clone().unwrap();
        assert!(forked.restored.is_empty(), "{:?}", forked.restored);
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));
        assert_eq!(forked.turn, 2);
        assert_eq!(
            sessions::meta(&id).unwrap().forked_from.unwrap().place(),
            "after turn 1"
        );

        // One branch has the workspace, even when the two agree on every file.
        assert_eq!(world.store.away(&original).unwrap(), Some(id.clone()));
        switch(&world.store, &world.ws, &original, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));
        assert_eq!(world.store.away(&id).unwrap(), Some(original));
    }

    #[tokio::test]
    async fn a_fork_inside_a_continued_turn_cuts_both_logs_where_the_request_began() {
        let world = World::new();
        let mut talk = Talk::start(&world);
        talk.turn(&world, "t1", "make a", &[("a.txt", Some("one"))])
            .await;
        talk.turn(&world, "t2", "change a", &[("a.txt", Some("two"))])
            .await;
        talk.turn_as(&world, "t3", true, "carry on", &[("a.txt", Some("three"))])
            .await;
        let original = talk.session.id.clone();

        // Checkpoint 3 is the continuation. The request it belongs to began
        // at checkpoint 2, so that's where the transcript is cut, and the
        // files and the copied checkpoints have to agree with it.
        let forked = fork(&world.store, &world.ws, &original, 3, false).unwrap();
        let id = forked.id.clone().unwrap();
        assert_eq!(forked.prompt, "change a");
        assert_eq!((forked.turn, forked.read_only), (2, false));
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));
        assert_eq!(world.store.turns(&id).unwrap().len(), 1);
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
    async fn a_set_aside_branch_diffs_against_its_own_files_not_the_workspace() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        fork(&world.store, &world.ws, &original, 2, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("one"));

        // Turn 3 of the original left a.txt as "three". The workspace has the
        // fork's "one" now, and the diff must not say turn 3 wrote that.
        let changes = world.store.changes(&original, &world.ws, 3).unwrap();
        let TurnChange::Diff { diff } = &changes[0] else {
            panic!("{changes:?}")
        };
        let added: Vec<&str> = diff
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == taurus_tools::diff::DiffLineKind::Added)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(added, ["three"]);
    }

    #[tokio::test]
    async fn comparing_shows_what_each_branch_did_since_they_parted() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        let id = fork(&world.store, &world.ws, &original, 2, false)
            .unwrap()
            .id
            .unwrap();
        let mut branch = Talk::resume(&id);
        branch
            .turn(
                &world,
                "f2",
                "try it differently",
                &[("a.txt", Some("other")), ("b.txt", Some("b"))],
            )
            .await;

        let compared = compare(&world.store, &world.ws, &id, &original).unwrap();
        assert_eq!(compared.shared, 1);
        assert!(compared.a.on_disk && !compared.b.on_disk);
        assert_eq!(compared.a.asked, ["try it differently"]);
        assert_eq!(compared.b.asked, ["change a, add x", "change a again"]);
        assert_eq!(
            compared.a.turns.iter().map(|t| t.turn).collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(
            compared.b.turns.iter().map(|t| t.turn).collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(
            (
                compared.a.usage.input_tokens,
                compared.a.usage.output_tokens
            ),
            (100, 10)
        );
        assert_eq!(compared.b.usage.input_tokens, 200);

        let paths: Vec<&str> = compared.files.iter().map(TurnChange::path).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "x.txt"]);
        // Read where each branch's files are, and nothing written.
        assert_eq!(world.read("a.txt").as_deref(), Some("other"));
        assert_eq!(world.store.away(&original).unwrap(), Some(id));
    }

    #[tokio::test]
    async fn two_set_aside_branches_compare_while_a_third_has_the_workspace() {
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
        fork(&world.store, &world.ws, &first, 3, false).unwrap();
        assert_eq!(world.read("a.txt").as_deref(), Some("two"));

        let compared = compare(&world.store, &world.ws, &original, &first).unwrap();
        assert!(!compared.a.on_disk && !compared.b.on_disk);
        assert_eq!(compared.shared, 2);
        let [TurnChange::Diff { diff }] = compared.files.as_slice() else {
            panic!("{:?}", compared.files)
        };
        assert_eq!(diff.path, "a.txt");
        let text = |kind| -> Vec<String> {
            diff.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == kind)
                .map(|line| line.text.clone())
                .collect()
        };
        assert_eq!(text(taurus_tools::diff::DiffLineKind::Removed), ["three"]);
        assert_eq!(text(taurus_tools::diff::DiffLineKind::Added), ["f"]);
    }

    #[tokio::test]
    async fn only_branches_of_one_conversation_compare() {
        let world = World::new();
        let one = three_turns(&world).await.session.id;
        let mut other = Talk::start(&world);
        other
            .turn(&world, "o1", "unrelated", &[("z.txt", Some("z"))])
            .await;
        let err = compare(&world.store, &world.ws, &one, &other.session.id).unwrap_err();
        assert!(
            err.contains("aren't branches of the same conversation"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_review_carried_into_a_fork_isnt_shown_as_the_forks_own() {
        let world = World::new();
        let talk = three_turns(&world).await;
        let original = talk.session.id.clone();
        let report = |turn: u32, at: u64| ReviewReport {
            turn,
            files: 1,
            model: "m".into(),
            text: format!("review of turn {turn}"),
            omitted: Vec::new(),
            read_claims: false,
            fingerprint: format!("f{turn}"),
            at,
            cached: false,
        };
        let made = world.store.turns(&original).unwrap()[1].at;
        crate::review::store(&world.ws, &original, &report(2, made));

        let id = fork(&world.store, &world.ws, &original, 2, false)
            .unwrap()
            .id
            .unwrap();
        let mut branch = Talk::resume(&id);
        branch
            .turn(&world, "f2", "again", &[("a.txt", Some("other"))])
            .await;

        // The original's turn 2 was reviewed. The fork has a turn 2 of its
        // own, so that review stays with the original.
        let compared = compare(&world.store, &world.ws, &id, &original).unwrap();
        assert!(compared.a.reviews.is_empty(), "{:?}", compared.a.reviews);
        assert_eq!(compared.b.reviews.len(), 1);

        let own = world.store.turns(&id).unwrap()[1].at;
        crate::review::store(&world.ws, &id, &report(2, own + 1));
        let compared = compare(&world.store, &world.ws, &id, &original).unwrap();
        assert_eq!(compared.a.reviews[0].at, own + 1);
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
