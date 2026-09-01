//! The engine driven the way the interface drives it: commands in, events out.

use std::sync::Arc;
use std::time::Duration;

use camion_core::RemotePath;
use camion_engine::engine::Command;
use camion_engine::event::Outcome;
use camion_engine::secrets::{InMemory, SecretStore};
use camion_engine::transfer::Place;
use camion_engine::{
    Answer, Attempt, Concern, Connection, Emitter, Engine, Event, SessionId, Vault,
};
use camion_providers::Destination;

/// Collects events off the engine's thread and hands them back one at a time, so a test can
/// wait for the thing it cares about without sleeping.
struct Watcher {
    events: std::sync::mpsc::Receiver<Event>,
}

impl Watcher {
    fn new() -> (Self, Emitter) {
        let (sender, events) = std::sync::mpsc::channel();

        let emit: Emitter = Arc::new(move |event| {
            let _ = sender.send(event);
        });

        (Self { events }, emit)
    }

    /// Waits for the first event the predicate accepts, giving up rather than hanging forever.
    ///
    /// Generous on purpose. The deadline is here so a broken engine fails the suite instead of
    /// hanging it — it is not a claim about how fast any of this is, and a tight one only fails
    /// on a machine that happens to be busy.
    fn wait_for<T>(&self, mut accept: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);

        while std::time::Instant::now() < deadline {
            match self.events.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => {
                    if let Some(found) = accept(event) {
                        return found;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("the engine stopped sending events: {error}"),
            }
        }

        panic!("the engine never sent the event this test was waiting for");
    }

    fn wait_for_session(&self) -> SessionId {
        self.wait_for(|event| match event {
            Event::Connected { session, .. } => Some(session),
            Event::ConnectionFailed { reason, .. } => panic!("could not connect: {reason}"),
            _ => None,
        })
    }

    fn wait_for_listing(&self, at: &str) -> Vec<String> {
        let wanted = RemotePath::parse(at).unwrap();

        self.wait_for(|event| match event {
            Event::Listing { path, entries, .. } if path == wanted => {
                Some(entries.into_iter().map(|entry| entry.name).collect())
            }
            _ => None,
        })
    }
}

fn started() -> (Engine, Watcher, SessionId) {
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new(
            "Scratch",
            Destination::Memory,
        )),
    });

    let session = watcher.wait_for_session();

    (engine, watcher, session)
}

#[test]
fn connecting_opens_the_root_folder() {
    let (_engine, watcher, _session) = started();

    let mut names = watcher.wait_for_listing("/");
    names.sort();

    assert_eq!(names, vec!["README.md", "documents", "photos"]);
}

#[test]
fn opening_a_folder_lists_it() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Open { session, path: RemotePath::parse("/documents").unwrap() });

    let mut names = watcher.wait_for_listing("/documents");
    names.sort();

    assert_eq!(names, vec!["invoices", "notes.txt"]);
}

#[test]
fn creating_a_folder_shows_it_without_asking_again() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::CreateFolder { session, name: "uploads".to_owned() });

    // Waited for by path rather than by contents, so the assertion below is the thing being
    // tested rather than a restatement of what was waited for.
    let mut names = watcher.wait_for_listing("/");
    names.sort();

    assert_eq!(names, vec!["README.md", "documents", "photos", "uploads"]);
}

#[test]
fn renaming_and_deleting_are_reflected_in_the_listing() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Rename {
        session,
        from: "README.md".to_owned(),
        to: "READ-ME.md".to_owned(),
    });

    watcher.wait_for(|event| match event {
        Event::Listing { entries, .. } if entries.iter().any(|e| e.name == "READ-ME.md") => Some(()),
        _ => None,
    });

    engine.send(Command::Delete { session, names: vec!["photos".to_owned()] });

    watcher.wait_for(|event| match event {
        Event::Listing { entries, .. } if !entries.iter().any(|e| e.name == "photos") => Some(()),
        _ => None,
    });
}

#[test]
fn a_dropped_file_is_queued_reports_progress_and_arrives() {
    let contents = "a file dragged in from the file manager";
    let directory = tempfile::tempdir().unwrap();
    let dropped = directory.path().join("harbour.txt");
    std::fs::write(&dropped, contents).unwrap();

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Upload {
        session,
        into: RemotePath::root(),
        sources: vec![dropped],
    });

    let queued = watcher.wait_for(|event| match event {
        Event::TransferAdded(transfer) => Some(transfer),
        _ => None,
    });
    assert_eq!(queued.name, "harbour.txt");

    let transferred = watcher.wait_for(|event| match event {
        Event::TransferProgress { transferred, .. } => Some(transferred),
        _ => None,
    });
    assert_eq!(transferred, contents.len() as u64);

    let outcome = watcher.wait_for(|event| match event {
        Event::TransferFinished { outcome, .. } => Some(outcome),
        _ => None,
    });
    assert_eq!(outcome, Outcome::Done);

    // The listing arrives on its own. Having to press refresh to see a file you just dropped
    // is exactly the kind of thing this application exists not to do.
    assert!(watcher.wait_for_listing("/").contains(&"harbour.txt".to_owned()));
}

#[test]
fn dropping_several_files_redraws_the_list_once_they_have_all_landed() {
    let directory = tempfile::tempdir().unwrap();
    let dropped = ["one.txt", "two.txt", "three.txt"]
        .iter()
        .map(|name| {
            let path = directory.path().join(name);
            std::fs::write(&path, *name).unwrap();
            path
        })
        .collect::<Vec<_>>();

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Upload {
        session,
        into: RemotePath::root(),
        sources: dropped,
    });

    let mut arrived = 0;
    let names = watcher.wait_for(|event| match event {
        Event::TransferFinished { .. } => {
            arrived += 1;
            None
        }
        Event::Listing { entries, .. } if arrived == 3 => {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        _ => None,
    });

    for name in ["one.txt", "two.txt", "three.txt"] {
        assert!(names.contains(&name.to_owned()), "{name} is missing from the listing");
    }
}

#[test]
fn a_downloaded_file_lands_on_disk() {
    let directory = tempfile::tempdir().unwrap();

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Download {
        session,
        names: vec!["README.md".to_owned()],
        into: directory.path().to_path_buf(),
    });

    let outcome = watcher.wait_for(|event| match event {
        Event::TransferFinished { outcome, .. } => Some(outcome),
        _ => None,
    });

    assert_eq!(outcome, Outcome::Done);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("README.md")).unwrap(),
        "Camion\n======\n"
    );
}

/// Dragging a folder out means the folder, with everything under it — not a refusal, and not a
/// flat pile of its files next to each other.
#[test]
fn a_downloaded_folder_arrives_with_its_tree_intact() {
    let directory = tempfile::tempdir().unwrap();

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Download {
        session,
        names: vec!["documents".to_owned()],
        into: directory.path().to_path_buf(),
    });

    // One transfer per file, and the folder holds two of them.
    for _ in 0..2 {
        let outcome = watcher.wait_for(|event| match event {
            Event::TransferFinished { outcome, .. } => Some(outcome),
            _ => None,
        });

        assert_eq!(outcome, Outcome::Done);
    }

    let here = directory.path().join("documents");

    assert_eq!(
        std::fs::read_to_string(here.join("notes.txt")).unwrap(),
        "remember the milk"
    );
    assert_eq!(
        std::fs::metadata(here.join("invoices/2026-08.pdf")).unwrap().len(),
        4096
    );
}

#[test]
fn moving_a_file_into_a_folder_takes_it_out_of_this_one() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Move {
        from: Place::new(session, RemotePath::root()),
        names: vec!["README.md".to_owned()],
        into: Place::new(session, RemotePath::parse("/documents").unwrap()),
    });

    let here = watcher.wait_for(|event| match event {
        Event::Listing { path, entries, .. } if path == RemotePath::root() => {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        _ => None,
    });
    assert!(!here.contains(&"README.md".to_owned()));

    engine.send(Command::Open { session, path: RemotePath::parse("/documents").unwrap() });
    assert!(watcher.wait_for_listing("/documents").contains(&"README.md".to_owned()));
}

#[test]
fn a_folder_cannot_be_moved_into_itself() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Move {
        from: Place::new(session, RemotePath::root()),
        names: vec!["documents".to_owned()],
        into: Place::new(session, RemotePath::parse("/documents/invoices").unwrap()),
    });

    let message = watcher.wait_for(|event| match event {
        Event::Failed { message, .. } => Some(message),
        _ => None,
    });

    assert!(message.contains("cannot be moved inside itself"), "{message}");
}

/// Something picked up in one folder and dropped in another is still moved from where it was
/// found, not from wherever the listing happens to be by the time the drop lands.
#[test]
fn files_can_be_moved_after_navigating_away_from_them() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    // Pick something up in the root, then go somewhere else entirely.
    engine.send(Command::Open { session, path: RemotePath::parse("/photos").unwrap() });
    watcher.wait_for_listing("/photos");

    engine.send(Command::Move {
        from: Place::new(session, RemotePath::root()),
        names: vec!["README.md".to_owned()],
        into: Place::new(session, RemotePath::parse("/photos").unwrap()),
    });

    let arrived = watcher.wait_for(|event| match event {
        Event::Listing { path, entries, .. } if path == RemotePath::parse("/photos").unwrap() => {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        _ => None,
    });

    assert!(arrived.contains(&"README.md".to_owned()), "{arrived:?}");
}

#[test]
fn a_command_for_a_connection_that_is_not_open_is_reported_not_ignored() {
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    engine.send(Command::Refresh(SessionId(404)));

    let message = watcher.wait_for(|event| match event {
        Event::Failed { message, .. } => Some(message),
        _ => None,
    });

    assert_eq!(message, "that connection is not open");
}

#[test]
fn an_object_store_asks_for_both_halves_of_its_key() {
    use camion_engine::Question;
    use camion_providers::destination::{S3Preset, S3};

    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new(
            "Assets",
            Destination::S3(S3 {
                bucket: "assets".to_owned(),
                preset: S3Preset::Other,
                region: "us-east-1".to_owned(),
                // Nothing listens here, so the connection fails at once — after the keys have
                // been asked for and accepted, which is what this test is about.
                endpoint: "http://127.0.0.1:1".to_owned(),
                root: String::new(),
            }),
        )),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert!(matches!(asked.question, Question::KeyPair { .. }));

    asked.answer(Answer::Pair {
        id: "AKIA".to_owned(),
        secret: "shh".to_owned(),
    });

    watcher.wait_for(|event| match event {
        Event::ConnectionFailed { reason, .. } => Some(reason),
        _ => None,
    });
}

#[test]
fn a_destination_that_needs_a_password_asks_for_one() {
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new(
            "Files",
            // A port nothing listens on, so the connection fails at once and without the test
            // depending on a name that has to be resolved.
            Destination::Ftp(camion_providers::destination::Ftp {
                host: "127.0.0.1".to_owned(),
                port: 1,
                username: "camion".to_owned(),
                encrypted: true,
                passive: true,
                home: String::new(),
            }),
        )),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert!(matches!(
        asked.question,
        camion_engine::Question::Password { .. }
    ));

    asked.answer(Answer::Text("hunter2".to_owned()));

    // The connection then fails, as it must — but it fails having been given the password,
    // which is what this test is about.
    watcher.wait_for(|event| match event {
        Event::ConnectionFailed { reason, .. } => Some(reason),
        _ => None,
    });
}

/// Cancelling has to reach the transfer itself. Removing the row and leaving the upload running
/// would keep writing to the server with nothing on screen to say so.
#[test]
fn a_cancelled_transfer_stops_and_says_it_was_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let dropped = directory.path().join("endless.bin");

    // A pipe that is open but never written to, so the upload is still going when the cancel
    // lands. A real file would race: a few megabytes into memory can finish before the next
    // command is read, and the test would be measuring who won rather than what cancelling does.
    assert!(
        std::process::Command::new("mkfifo").arg(&dropped).status().unwrap().success(),
        "mkfifo"
    );

    // Held open from here for reading *and* writing. Opening a pipe either way alone waits for
    // the other end: without this the engine blocks inside `open` on a thread it never gets
    // back, and opening it write-only from a thread of our own only moves the problem — a
    // cancel that lands before the upload opens its end leaves that thread waiting for a reader
    // that is never coming, and the test hangs instead of failing. Read-write never blocks, and
    // holding it is what keeps the upload from finishing on its own.
    let far_end = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&dropped)
        .expect("the far end of the pipe");

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Upload { session, into: RemotePath::root(), sources: vec![dropped] });

    let queued = watcher.wait_for(|event| match event {
        Event::TransferAdded(transfer) => Some(transfer),
        _ => None,
    });

    engine.send(Command::CancelTransfer(queued.id));

    let outcome = watcher.wait_for(|event| match event {
        Event::TransferFinished { outcome, .. } => Some(outcome),
        _ => None,
    });

    assert_eq!(outcome, Outcome::Cancelled);

    drop(far_end);
}

/// A machine with no keyring keeps its secrets in a passphrase-encrypted file, and the
/// passphrase has to be asked for. Opening that file under an empty one would leave every
/// password on disk locked with a key anybody can guess.
#[test]
fn a_locked_secrets_file_is_asked_about_before_anything_is_read_from_it() {
    use camion_engine::Question;

    let directory = tempfile::tempdir().unwrap();
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::locked(directory.path().join("secrets"))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new(
            "Files",
            Destination::Ftp(camion_providers::destination::Ftp {
                host: "127.0.0.1".to_owned(),
                port: 1,
                username: "camion".to_owned(),
                encrypted: false,
                passive: true,
                home: String::new(),
            }),
        )),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert_eq!(asked.question, Question::Passphrase);

    asked.answer(Answer::Text("open sesame".to_owned()));

    // Only once it is open does the connection get as far as wanting the password itself.
    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert!(matches!(asked.question, Question::Password { .. }));
}

/// Declining leaves the file shut rather than carrying on without it, because carrying on means
/// asking for every password again and saving them nowhere.
#[test]
fn refusing_the_passphrase_abandons_the_connection() {
    let directory = tempfile::tempdir().unwrap();
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::locked(directory.path().join("secrets"))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new(
            "Files",
            Destination::Ftp(camion_providers::destination::Ftp {
                host: "127.0.0.1".to_owned(),
                port: 1,
                username: "camion".to_owned(),
                encrypted: false,
                passive: true,
                home: String::new(),
            }),
        )),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    asked.answer(Answer::Decline);

    let reason = watcher.wait_for(|event| match event {
        Event::ConnectionFailed { reason, .. } => Some(reason),
        _ => None,
    });

    assert_eq!(reason, "cancelled");
}

/// Dropping a file onto a folder that already has one by that name asks first. Overwriting
/// silently is how an afternoon's work disappears under something from a downloads folder.
fn dropping_onto_an_existing_file(
    answer: Answer,
) -> (Engine, Watcher, SessionId, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let dropped = directory.path().join("README.md");
    std::fs::write(&dropped, "from the desktop").unwrap();

    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Upload {
        session,
        into: RemotePath::root(),
        sources: vec![dropped],
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert_eq!(
        asked.question,
        camion_engine::Question::Overwrite { name: "README.md".to_owned() }
    );

    asked.answer(answer);

    // The engine goes back to the caller: dropping it here would stop the thread mid-upload
    // and the test would see the channel close rather than the answer it is waiting for.
    (engine, watcher, session, directory)
}

#[test]
fn replacing_an_existing_file_leaves_one_of_it() {
    let (_engine, watcher, _session, _directory) = dropping_onto_an_existing_file(Answer::Accept);

    let queued = watcher.wait_for(|event| match event {
        Event::TransferAdded(transfer) => Some(transfer),
        _ => None,
    });

    assert_eq!(queued.name, "README.md");

    let mut names = watcher.wait_for_listing("/");
    names.sort();

    assert_eq!(names, vec!["README.md", "documents", "photos"]);
}

#[test]
fn keeping_both_puts_the_new_one_beside_the_old() {
    let (_engine, watcher, _session, _directory) = dropping_onto_an_existing_file(Answer::KeepBoth);

    let queued = watcher.wait_for(|event| match event {
        Event::TransferAdded(transfer) => Some(transfer),
        _ => None,
    });

    assert_eq!(queued.name, "README (2).md");
}

#[test]
fn declining_uploads_nothing_at_all() {
    let (engine, watcher, session, _directory) = dropping_onto_an_existing_file(Answer::Decline);

    // Declining queues nothing, so there is no transfer to wait on. Asking for the folder
    // again gives the engine something to answer, and the answer says what is really there.
    engine.send(Command::Refresh(session));

    let names = watcher.wait_for(|event| match event {
        Event::Listing { entries, .. } => {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        Event::TransferAdded(transfer) => panic!("queued {} after declining", transfer.name),
        _ => None,
    });

    assert!(!names.contains(&"README (2).md".to_owned()), "{names:?}");
    drop(engine);
}

/// Connecting asks for a credential only when none is stored, so replacing a mistyped one needs
/// its own way in — otherwise a wrong access key is permanent short of editing the keyring.
#[test]
fn changing_credentials_replaces_what_was_stored() {
    use camion_engine::Question;
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::Secret;

    let store = Arc::new(InMemory::default());
    let connection = Connection::new(
        "Assets",
        Destination::S3(S3 {
            bucket: "assets".to_owned(),
            preset: S3Preset::Other,
            region: "us-east-1".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            root: String::new(),
        }),
    );

    store
        .set(
            &connection.id,
            &Secret::KeyPair { id: "WRONG".to_owned(), secret: "wrong".to_owned() },
        )
        .unwrap();

    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::clone(&store) as _)), emit);

    engine.send(Command::ChangeCredentials {
        attempt: Attempt::next(),
        connection: Box::new(connection.clone()),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    assert!(matches!(asked.question, Question::KeyPair { .. }));

    asked.answer(Answer::Pair { id: "RIGHT".to_owned(), secret: "right".to_owned() });

    watcher.wait_for(|event| match event {
        Event::Notice { message, .. } => Some(message),
        _ => None,
    });

    assert_eq!(
        store.get(&connection.id).unwrap(),
        Secret::KeyPair { id: "RIGHT".to_owned(), secret: "right".to_owned() }
    );
}

/// A dialog closed by accident must not throw away a credential that was working.
#[test]
fn declining_leaves_the_old_credentials_alone() {
    use camion_providers::Secret;

    let store = Arc::new(InMemory::default());
    let connection = Connection::new(
        "Files",
        Destination::Ftp(camion_providers::destination::Ftp {
            host: "127.0.0.1".to_owned(),
            port: 1,
            username: "camion".to_owned(),
            encrypted: false,
            passive: true,
            home: String::new(),
        }),
    );

    let kept = Secret::Password("still good".to_owned());
    store.set(&connection.id, &kept).unwrap();

    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::clone(&store) as _)), emit);

    engine.send(Command::ChangeCredentials {
        attempt: Attempt::next(),
        connection: Box::new(connection.clone()),
    });

    let asked = watcher.wait_for(|event| match event {
        Event::Ask(prompt) => Some(prompt),
        _ => None,
    });

    asked.answer(Answer::Decline);

    // Nothing to wait for, so the next command's answer is what says the first one is over.
    engine.send(Command::Refresh(SessionId(404)));
    watcher.wait_for(|event| match event {
        Event::Failed { .. } => Some(()),
        _ => None,
    });

    assert_eq!(store.get(&connection.id).unwrap(), kept);
}

/// Two connections opening at once are two sets of questions, and the only thing telling them
/// apart is the attempt each one carries. Without it, both windows show both prompts and the
/// second dialog is left asking about work the first has already answered.
#[test]
fn a_question_says_which_connection_it_is_holding_up() {
    use camion_providers::destination::Ftp;

    // Two destinations that ask for a credential and then fail to connect, so each attempt is
    // over as soon as its question is answered.
    let refused = |name: &str| {
        Connection::new(
            name,
            Destination::Ftp(Ftp {
                host: "127.0.0.1".to_owned(),
                port: 1,
                username: "camion".to_owned(),
                encrypted: false,
                passive: true,
                home: String::new(),
            }),
        )
    };

    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    let (ours, theirs) = (Attempt::next(), Attempt::next());

    engine.send(Command::Connect { attempt: ours, connection: Box::new(refused("First")) });
    engine.send(Command::Connect { attempt: theirs, connection: Box::new(refused("Second")) });

    let mut asked = Vec::new();

    while asked.len() < 2 {
        let prompt = watcher.wait_for(|event| match event {
            Event::Ask(prompt) => Some(prompt),
            _ => None,
        });

        asked.push(prompt.concern);
        prompt.answer(Answer::Decline);
    }

    asked.sort_by_key(|concern| format!("{concern:?}"));

    assert_eq!(asked, vec![Concern::Attempt(ours), Concern::Attempt(theirs)]);
}

/// Two windows, two connections, both on the same in-memory destination.
///
/// A second connection rather than a second session on the first: what makes a drop a copy
/// rather than a rename is that the two ends are different servers, and two `Connection`s are
/// the only way to say that.
fn two_connections() -> (Engine, Watcher, SessionId, SessionId) {
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new("Scratch", Destination::Memory)),
    });

    let here = watcher.wait_for_session();

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(Connection::new("Elsewhere", Destination::Memory)),
    });

    let there = watcher.wait_for_session();

    (engine, watcher, here, there)
}

/// Dragging a file from one window into another, where the two are on different servers. The
/// bytes have to cross, and they cross without ever being on this machine.
#[test]
fn a_file_dropped_on_another_connection_is_carried_across() {
    let (engine, watcher, here, there) = two_connections();

    // The window being dropped into is looking at the folder being dropped into, which is what
    // makes it that folder. Redrawing after a drop means redrawing what that window has open.
    engine.send(Command::Open {
        session: there,
        path: RemotePath::parse("/documents").unwrap(),
    });
    watcher.wait_for_listing("/documents");

    engine.send(Command::Move {
        from: Place::new(here, RemotePath::root()),
        names: vec!["README.md".to_owned()],
        into: Place::new(there, RemotePath::parse("/documents").unwrap()),
    });

    let queued = watcher.wait_for(|event| match event {
        Event::TransferAdded(transfer) => Some(transfer),
        _ => None,
    });

    assert_eq!(queued.direction, camion_engine::transfer::Direction::Between);
    assert_eq!(queued.name, "README.md");
    assert_eq!(queued.total, Some(14));

    let outcome = watcher.wait_for(|event| match event {
        Event::TransferFinished { outcome, .. } => Some(outcome),
        _ => None,
    });
    assert_eq!(outcome, Outcome::Done);

    // The window dropped into redraws itself, the same as it does for a file dropped in from
    // the desktop. Having to press refresh to see what you just dragged in is the thing this
    // application exists not to do.
    let mut arrived = watcher.wait_for(|event| match event {
        Event::Listing { session, path, entries }
            if session == there && path == RemotePath::parse("/documents").unwrap() =>
        {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        _ => None,
    });
    arrived.sort();

    assert_eq!(arrived, vec!["README.md", "invoices", "notes.txt"]);

    // Copied, not moved. Taking somebody's only copy off the server it was on is not something
    // a gesture that can be made by accident should do.
    engine.send(Command::Refresh(here));
    assert!(watcher.wait_for_listing("/").contains(&"README.md".to_owned()));
}

/// A folder is a shape rather than a thing that can be transferred, so it is walked: the
/// folders are made on the far side and every file inside crosses on its own.
#[test]
fn a_folder_dropped_on_another_connection_arrives_with_its_tree_intact() {
    let (engine, watcher, here, there) = two_connections();

    engine.send(Command::Move {
        from: Place::new(here, RemotePath::root()),
        names: vec!["documents".to_owned()],
        into: Place::new(there, RemotePath::parse("/photos").unwrap()),
    });

    let mut landed = 0;
    watcher.wait_for(|event| match event {
        Event::TransferFinished { outcome: Outcome::Done, .. } => {
            landed += 1;
            (landed == 2).then_some(())
        }
        Event::TransferFinished { outcome, .. } => panic!("a file did not arrive: {outcome:?}"),
        _ => None,
    });

    engine.send(Command::Open {
        session: there,
        path: RemotePath::parse("/photos/documents/invoices").unwrap(),
    });

    assert_eq!(
        watcher.wait_for_listing("/photos/documents/invoices"),
        vec!["2026-08.pdf"]
    );
}

/// Two windows open on the *same* saved connection are two sessions onto one machine, and
/// asking that machine to rename a file is right whichever of them the drag started in. Copying
/// the bytes out and back would be the same file taking a round trip through this process for
/// no reason at all.
#[test]
fn a_file_dropped_on_the_same_connection_is_renamed_rather_than_copied() {
    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(Vault::open(Arc::new(InMemory::default()))), emit);

    let connection = Connection::new("Scratch", Destination::Memory);

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(connection.clone()),
    });
    let here = watcher.wait_for_session();

    engine.send(Command::Connect {
        attempt: Attempt::next(),
        connection: Box::new(connection),
    });
    let alongside = watcher.wait_for_session();

    engine.send(Command::Move {
        from: Place::new(here, RemotePath::root()),
        names: vec!["README.md".to_owned()],
        into: Place::new(alongside, RemotePath::parse("/documents").unwrap()),
    });

    // Whichever of these arrives first is the answer: a rename says so by the file leaving the
    // folder it was in, and a copy says so by queueing a transfer.
    let copied = watcher.wait_for(|event| match event {
        Event::TransferAdded(_) => Some(true),
        Event::Listing { session, path, entries }
            if session == here
                && path == RemotePath::root()
                && !entries.iter().any(|entry| entry.name == "README.md") =>
        {
            Some(false)
        }
        _ => None,
    });

    assert!(!copied, "a move within one connection carried the bytes instead of renaming");
}
