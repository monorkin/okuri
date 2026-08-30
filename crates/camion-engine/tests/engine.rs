//! The engine driven the way the interface drives it: commands in, events out.

use std::sync::Arc;
use std::time::Duration;

use camion_core::RemotePath;
use camion_engine::engine::Command;
use camion_engine::event::Outcome;
use camion_engine::secrets::InMemory;
use camion_engine::{Answer, Connection, Emitter, Engine, Event, SessionId};
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
    fn wait_for<T>(&self, mut accept: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

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
    let engine = Engine::start(Arc::new(InMemory::default()), emit);

    engine.send(Command::Connect(Box::new(Connection::new(
        "Scratch",
        Destination::Memory,
    ))));

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

    let names = watcher.wait_for(|event| match event {
        Event::Listing { entries, .. } if entries.iter().any(|entry| entry.name == "uploads") => {
            Some(entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
        }
        _ => None,
    });

    assert!(names.contains(&"uploads".to_owned()));
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

#[test]
fn moving_a_file_into_a_folder_takes_it_out_of_this_one() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    engine.send(Command::Move {
        session,
        from: RemotePath::root(),
        names: vec!["README.md".to_owned()],
        into: RemotePath::parse("/documents").unwrap(),
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
        session,
        from: RemotePath::root(),
        names: vec!["documents".to_owned()],
        into: RemotePath::parse("/documents/invoices").unwrap(),
    });

    let message = watcher.wait_for(|event| match event {
        Event::Failed { message } => Some(message),
        _ => None,
    });

    assert!(message.contains("cannot be moved inside itself"), "{message}");
}

// Dragging a folder out means the folder, with what is in it — not a refusal, and not its
#[test]
fn files_can_be_moved_after_navigating_away_from_them() {
    let (engine, watcher, session) = started();
    watcher.wait_for_listing("/");

    // Pick something up in the root, then go somewhere else entirely.
    engine.send(Command::Open { session, path: RemotePath::parse("/photos").unwrap() });
    watcher.wait_for_listing("/photos");

    engine.send(Command::Move {
        session,
        from: RemotePath::root(),
        names: vec!["README.md".to_owned()],
        into: RemotePath::parse("/photos").unwrap(),
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
    let engine = Engine::start(Arc::new(InMemory::default()), emit);

    engine.send(Command::Refresh(SessionId(404)));

    let message = watcher.wait_for(|event| match event {
        Event::Failed { message } => Some(message),
        _ => None,
    });

    assert_eq!(message, "that connection is not open");
}

#[test]
fn an_object_store_asks_for_both_halves_of_its_key() {
    use camion_engine::Question;
    use camion_providers::destination::{S3Preset, S3};

    let (watcher, emit) = Watcher::new();
    let engine = Engine::start(Arc::new(InMemory::default()), emit);

    engine.send(Command::Connect(Box::new(Connection::new(
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
    ))));

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
    let engine = Engine::start(Arc::new(InMemory::default()), emit);

    engine.send(Command::Connect(Box::new(Connection::new(
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
    ))));

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
