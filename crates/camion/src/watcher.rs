use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, RecursiveMode, Watcher};

/// Watches a directory and calls back once things have settled.
///
/// The caller keeps the returned watcher alive; dropping it stops the watching.
///
/// The callback fires after the events stop rather than when they start. Replacing a directory
/// produces a flurry — the old one going away, the new one arriving — and the only moment worth
/// reacting to is the end of it, when what is on disk is what was meant to be there.
pub fn watch(
    directory: &Path,
    settle: Duration,
    mut changed: impl FnMut() + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    let (events, arrived) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        if event.is_ok() {
            let _ = events.send(());
        }
    })
    .ok()?;

    watcher.watch(directory, RecursiveMode::NonRecursive).ok()?;

    std::thread::Builder::new()
        .name("camion-watcher".to_owned())
        .spawn(move || {
            while arrived.recv().is_ok() {
                // Swallow everything that arrives while the change is still in progress, then
                // act once on the state it settled into.
                while arrived.recv_timeout(settle).is_ok() {}

                changed();
            }
        })
        .ok()?;

    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const SETTLE: Duration = Duration::from_millis(50);

    fn wait_for(count: &AtomicUsize, at_least: usize) -> usize {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            let seen = count.load(Ordering::SeqCst);

            if seen >= at_least {
                return seen;
            }

            std::thread::sleep(Duration::from_millis(20));
        }

        panic!("the watcher never reported a change");
    }

    /// The shape of an Omarchy theme switch: the whole directory is removed and a new one is
    /// moved into its place. Watching the file itself would miss this entirely, which is the
    /// bug this test exists to keep out.
    #[test]
    fn replacing_a_directory_wholesale_is_noticed() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let theme = current.join("theme");
        let next = current.join("next-theme");

        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), "bg = \"#101010\"\n").unwrap();

        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);

        let _watcher = watch(&current, SETTLE, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("a watcher");

        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(next.join("colors.toml"), "bg = \"#fdfdfd\"\n").unwrap();
        std::fs::remove_dir_all(&theme).unwrap();
        std::fs::rename(&next, &theme).unwrap();

        wait_for(&changes, 1);

        assert_eq!(
            std::fs::read_to_string(theme.join("colors.toml")).unwrap(),
            "bg = \"#fdfdfd\"\n"
        );
    }

    #[test]
    fn a_flurry_of_changes_is_reported_once_it_settles() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("watched");
        std::fs::create_dir_all(&directory).unwrap();

        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);

        let _watcher = watch(&directory, SETTLE, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("a watcher");

        for each in 0..20 {
            std::fs::write(directory.join(format!("file-{each}")), "x").unwrap();
        }

        wait_for(&changes, 1);
        std::thread::sleep(SETTLE * 4);

        // Twenty writes in a burst are one change worth reacting to, not twenty.
        assert!(
            changes.load(Ordering::SeqCst) <= 3,
            "expected the burst to coalesce, saw {} callbacks",
            changes.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn watching_somewhere_that_does_not_exist_is_not_a_crash() {
        let root = tempfile::tempdir().unwrap();

        assert!(watch(&root.path().join("nothing-here"), SETTLE, || {}).is_none());
    }
}
