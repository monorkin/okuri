# Working on Okuri

The README says what Okuri is. This says what it is like to change it, and what has already
gone wrong so it does not go wrong again.

## The layers, and what each one is allowed to know

```
okuri-core       the domain. No HTTP, no SSH, no Qt.
okuri-providers  the adapters, plus config shapes, secrets, host trust.
okuri-engine     the runtime: sessions, transfers, prompts, config. No Qt.
okuri            the Qt binary: bridges + QML.
```

Dependencies run one way and nothing flows back down. The interface talks to the engine only in
`Command`s and `Event`s.

## Windows, and why nothing is a singleton any more

Okuri opens as many windows as you ask for, and each one is a separate connection. The engine
always allowed that — sessions have lived in a registry from the start — so what changed was
above it:

- **The root QML file is `Okuri.qml`, not `Main.qml`.** It is a `QtObject`, not a window: it
  owns the windows and creates them, and a window asks for the next one by emitting `another`.
  A window that owned the list of windows would take the list with it when it closed.
- **`App` is one per window**, declared in `Main.qml` and handed down. Every component that
  needs it takes `required property App app`. Do not reintroduce `#[qml_singleton]` on it, and
  do not have a component reach for the application on its own — with two windows open, "the"
  application is whichever one was built first.
- **In `Main.qml` the object is `App { id: theApp }` behind `readonly property App app`.**
  Writing `app: app` on a child is a binding loop: the right-hand `app` resolves to the child's
  own property. Children get `app: window.app`.
- **The engine is process-wide**, in `crates/okuri/src/running.rs`. One runtime, one session
  registry, one transfer queue. A second engine would mean a transfer one window could not see.
- **Every event says whose it is.** `Concern` is `Everyone`, `Attempt`, or `Session`;
  `Screen::receive` drops anything that is not this window's. An `Attempt` names a connection
  being opened, which is the one stretch of work with no session yet and the one that asks the
  most questions. Messages marked `Everyone` show in every window; a *question* marked
  `Everyone` goes only to the primary window, because two dialogs over one `oneshot` leaves one
  of them asking about something already answered.
- **A new event that carries a session must be added to `Event::concern`.** Answering
  `Everyone` by default is how one window starts drawing another's files.
- **A drag carries everything needed to act on it** (`Carried` in `screen.rs`, as the
  `application/x-okuri-move` payload): the session, the folder, and the names. It has to — a
  drop can land in another window, and that window's `App` cannot ask the other one what was
  picked up.

## Dropping files from one window into another

`Command::Move` names a connection at both ends and `Running::relocate` picks how:

- **Same connection, even across two windows → `rename`.** Two windows on one saved connection
  are two sessions to one machine, so the machine renames the file and both windows redraw.
  Compare `Session::connection`, never `SessionId` — same-session is only the in-window case.
- **Different connections → the bytes are carried**, and it is a *copy*: dragging between two
  servers leaves the original where it was, the way dragging between two disks does. Taking
  somebody's only copy off a server on the strength of a gesture is not something to do.

The carry (`engine::carry`) hands the read stream straight to the write. Nothing touches the
disk and nothing waits for a whole file, so a hundred-gigabyte object crosses in the memory of
one chunk. Two things make it fast rather than merely correct, and both are easy to undo:

- **The size goes across.** A destination told how big a file is writes it in one request; one
  that is not has to split it or hold it to find out.
- **A transfer between two servers holds a slot on both**, taken in `SessionId` order.
  Both, because it is using both. In a fixed order, because files dragged both ways at once
  would otherwise each hold the slot the other is waiting for.

**Metadata travels on the `ByteStream`, not in a second request.** `read` fills in `Serve`
(content type, cache control, content encoding) from the download response it already has, and
`write` prefers it over guessing from the file name — which is the whole point, since the files
that most need this are the ones with no extension to guess from. `counting` passes it through;
anything else that rewraps a stream must too, or the type is silently lost. The Unix mode is the
exception and rides on `Crossing`: no protocol puts it in a response, and it is set after the
write, only where both ends have modes.

What deliberately does *not* travel: ETag, storage class, encryption, version. Those describe
where a file lives rather than what it is, and copying them to another store would be stating
something untrue about it.

**Adding a destination** is one file in `okuri-providers` implementing `Provider`, one arm in
`Destination`, and a conformance run. If it needs anything above it, something is wrong.

## The trait shape, which is the one architectural rule

`Provider` is small and deliberately universal — list, stat, read, write, delete, create folder,
rename. Anything only *some* destinations can do is its own small trait, answered through an
accessor that returns `None` by default:

| Trait | Answers | Implemented by |
|---|---|---|
| `Sharing` | visibility, public and signed URLs | S3-shaped |
| `Permitting` | changing the mode | SFTP |
| `Owning` | user and group | SFTP |
| `Linking` | where a symlink points | SFTP |
| `Serving` | content type, ETag, cache headers | S3, Azure, WebDAV |
| `Storing` | storage class, encryption, version | S3, Azure |

Two rules that came out of getting this wrong:

- **Never add an optional method to `Provider` that adapters must decline.** An SFTP server has
  no storage class and an object store has no group. A trait it simply does not implement is
  better than a method it answers with an error.
- **Never let a provider return labelled strings for the interface to display.** There was a
  `describe() -> Vec<Fact>` doing exactly that; it put UI wording in the providers crate and
  made every value un-actionable. Wording lives in `crates/okuri/src/screen.rs`.

`Capabilities` flags for these are **derived in the engine** from whether the accessor answers,
never declared by the adapter — a declared flag drifts from what the code does. That has already
happened once: FTP declared four concurrent transfers while serialising everything behind one
mutex.

## Verifying a change

```sh
cargo test --workspace                                   # unit + integration
cargo clippy --workspace --all-targets                   # must be clean
docker compose -f test/compose.yaml up -d --wait         # --wait matters
cargo test --workspace -- --ignored --test-threads=1     # against real servers
docker compose -f test/compose.yaml down
```

`--wait` is not optional: the bucket and container are created by one-shot services, and without
it the suite races them and fails on a bucket that is seconds from existing.

The **conformance suite** is the load-bearing test idea. One set of checks written against
`dyn Provider`, run against `MemoryProvider` in-process and against real containers behind
`--ignored`, with capability flags deciding what is skipped. A new adapter is not done until it
passes it. Anything a real server does that a fake cannot tell you — ACL support, content types,
mode changes — belongs in `crates/okuri-providers/tests/conformance.rs`, not a unit test.

If the FTP container starts failing on folders that should exist, it is holding state from an
earlier broken run: `docker compose -f test/compose.yaml restart ftp`.

**A test that hangs rather than fails will only hang under load.** The cancel test passes alone
every time and hung twice under `--workspace`, because opening a FIFO one way waits for the
other end. Open both ends read-write; anything else in a test is a deadlock waiting for a busy
machine. `wait_for` has a deadline for the same reason — a blocked test is far harder to read
than a failed one.

## Verifying QML, which is where the traps are

**Qt logs to journald on this machine, not stderr.** Without the environment variable below, a
run produces no output at all — not even for a guaranteed `ReferenceError` — and silence reads
as success. This wasted several rounds:

```sh
QT_FORCE_STDERR_LOGGING=1 QT_QPA_PLATFORM=offscreen timeout 8 ./target/debug/okuri > run.log 2>&1
cat run.log     # empty means clean, but only with the variable set
```

**A clean startup proves nothing about a dialog.** QML type errors surface when a component is
*instantiated*, and `FileDetails`, `ConnectionEditor` and friends are not built until they are
opened. To check one, temporarily add it to `Main.qml` with `visible: true` and a populated
property, run headless, read the log, then take it out. `qmlcachegen` runs at build time and
catches structural errors — an unbalanced brace fails the build — but not type errors.

**Every new `.qml` file must be listed in `crates/okuri/build.rs`.** Otherwise it does not
exist to the engine, and the failure is `X is not a type` at runtime, which cascades into a
window that never opens.

**A missing `required property` is caught only when the component is built.** It reads
`Required property app was not initialized`, and the component silently does not exist. Grid,
Rows and Breadcrumb each build a `SpringLoaded`, so anything threaded down to it has to be
threaded through all three.

Other QML lessons already paid for:

- **Clicking a `Switch` or `CheckBox` destroys its `checked` binding.** Restore it in the
  handler with `checked = Qt.binding(() => …)`, or the control stops following the server and
  starts lying about state.
- **Inline components (`component Foo: …`) must be at the document's top level**, not nested in
  a layout.
- **A button on top of a `MouseArea` takes the hover away from it.** Anything whose visibility
  depends on the row being hovered has to use a `HoverHandler` on the row, or reaching for the
  button makes the button disappear.
- **`Drag.active = true` does not block.** The comment that said it did cost a long debugging
  session: the compositor takes the gesture and the call returns immediately, so anything
  cleared on the next line is cleared while the drag is still in flight. Clean up in
  `Drag.onActiveChanged` instead.
- **A `Rectangle` used as a control's `indicator` needs `implicitWidth`/`implicitHeight`.**
  Setting `width`/`height` draws it but tells the control nothing, and the control collapses to
  nothing.

## CXX-Qt

- **`#[auto_cxx_name]` is required** on every `extern "RustQt"` block. CXX-Qt does not
  camel-case names, and without it every multi-word property is `undefined` in QML.
- **`QAbstractListModel` may be declared in exactly one bridge.** It lives in `src/qt.rs` and
  every model aliases it; declaring it twice fails to link.
- **C++ keywords cannot be parameter names.** A `public: bool` argument generates C++ that will
  not compile.
- Cross-thread updates go through `crate::qt::queue`, which is the only door between the
  engine's threads and Qt's.

## Things that look wrong and are not

- **FTP has one transfer slot.** One control connection, one command at a time. Raising it only
  queues transfers behind the same mutex while holding more files in memory.
- **Azure's signature includes the account twice on the emulator.** Azurite addresses accounts
  by path, and the canonical resource covers the path as sent. There is a test for it.
- **A drag carries `text/uri-list` only for destinations the desktop can open.** Offering it for
  S3 makes a drop fail instead of being refused.
- **`moveInto` clears what is being carried only once it is acting on it.** A drop reaches more
  than one target, and clearing early leaves the real one with nothing.
- **The progress throttle is deliberate.** Reporting per chunk was millions of events for a
  large file, and it both slowed the transfer and buried the interface.

## Conventions

- Style follows `~/.claude/STYLE.md`: expanded conditionals over guard clauses, no ternaries,
  public before private, methods ordered by call order. Comments explain *why*, never what.
- Tests are named as sentences and assert behaviour, not markup. Prefer exact assertions; an
  or-of-both-outcomes assertion usually means the test does not know what it is testing.
- Errors are loud. Anything that quietly does nothing is a bug — a drop that lands and does
  nothing looks identical to one that was never delivered.
- Never commit or push unless asked.
