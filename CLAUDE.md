# Working on Okuri

The README says what Okuri is. This says what it is like to change it, and what has already
gone wrong so it does not go wrong again.

## The layers, and what each one is allowed to know

```
okuri-core       the domain. No HTTP, no SSH, no GTK.
okuri-providers  the adapters, plus config shapes, secrets, host trust.
okuri-engine     the runtime: sessions, transfers, prompts, config. No GTK.
okuri            the GTK 4 + libadwaita binary, written with gtk-rs.
```

Dependencies run one way and nothing flows back down. The interface talks to the engine only in
`Command`s and `Event`s.

## How the interface is put together

There is no GObject subclassing in the binary. Every piece of the window is a plain Rust struct
holding the widgets it built, and the pieces are wired with closures that capture a
`Weak<Window>` and upgrade it when they fire.

- **`window.rs` is the window** — what `App` and `Main.qml` used to be. It owns the `Screen`,
  the queue of unanswered questions, the file list, and the header, and it turns clicks into
  `Command`s. `render()` copies the `Screen` onto the widgets; there are no property bindings,
  so anything that should follow the screen has to be set there.
- **`Screen` (`screen.rs`) is still where the rules live**, and still the thing to test. The
  window only copies its answers onto widgets.
- **List rows are `glib::BoxedAnyObject`s** holding a plain `Row`, in a `gio::ListStore`. The
  list and the grid share the store and one `MultiSelection`, which is what keeps the selection
  when you switch between them. Progress is written into the row in place and announced with
  `items_changed(pos, 1, 1)`; the same object going back in is what keeps it selected.
- **Row widgets are recycled.** A controller attached in a factory's `setup` is attached once
  and fires for whichever row has scrolled into that widget since, so it must ask the
  `gtk::ListItem` for its `position()` and `item()` at the moment it fires, never remember them.
  The cell structs are rebuilt from the widget tree in `bind` (`ListCell::from_root`) for the
  same reason.
- **The relay (`relay.rs`) is the one door between the engine's threads and GTK's.** It hops
  every bus event, theme change and view-settings change onto the main loop and fans them out
  to `Rc` listeners. Subscriptions are held by the window and dropped with it; anything that
  outlives every window calls `.forever()`.
- **`transfers.rs` and `connections.rs` are process-wide** (`thread_local` singletons), because
  the queue and the saved connections are the two things it would be wrong to have two of.

## Windows, and why nothing is a singleton any more

Okuri opens as many windows as you ask for, and each one is a separate connection. The engine
always allowed that — sessions have lived in a registry from the start — so what changed was
above it:

- **`window::WINDOWS` owns the windows**, oldest first. A window asks for another through
  `Window::another`, and takes itself out of the list on `close-request`. The oldest open window
  is the *primary* one, which is where a question that belongs to no connection gets asked.
- **Everything below the window takes a `Weak<Window>`**, never reaches for "the" application.
  With two windows open, "the" application is whichever one was built first.
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
  drop can land in another window, and that window cannot ask the other one what was picked
  up.

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

## Verifying the interface

GTK logs to stderr, so a run says what went wrong. `Gtk-CRITICAL` lines are bugs, not noise:
each one is a call GTK refused, and the thing that call was for silently did not happen.

There is no headless backend worth using; run it on the real desktop, against a scratch config
so nothing of the person's is touched, and look at it:

```sh
export XDG_CONFIG_HOME=/tmp/okuri-scratch/config XDG_STATE_HOME=/tmp/okuri-scratch/state
# a connections.toml with the in-memory destination (`kind = "memory"`) needs no server
timeout 20 ./target/debug/okuri sample-files > run.log 2>&1 &
sleep 4
grim -g "$(hyprctl clients -j | jq -r '.[] | select(.class=="sh.okuri.Okuri") | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')" shot.png
wtype -k Down -k F2      # drives the window from the keyboard
```

There is no pointer tool installed, but `/dev/uinput` is writable by the user here, so a
virtual mouse is sixty lines of Python (ioctls `UI_SET_EVBIT`/`UI_DEV_SETUP`/`UI_DEV_CREATE`,
then `input_event` structs; `hyprctl cursorpos` reads the position back). That is how drag
and drop gets tested for real — a drag from Nautilus into Okuri, or between two Okuri
windows. Screenshot mid-drag: the drag icon proves the drag started, and the accent outline
proves the target accepted it. `GDK_DEBUG=dnd` prints nothing on Arch's GTK; do not wait for
it.

Copying an Omarchy theme directory into `$XDG_STATE_HOME/omarchy/current/theme` is how to
check a palette, and deleting it and moving another into its place while the app is running is
how to check the live switch — that is exactly what `omarchy-theme-set` does.

**A clean startup proves nothing about a dialog.** The details panel, the editor, the questions
and the queue are built when they are opened, so a `CRITICAL` in one of them only shows once
you open it.

Lessons already paid for:

- **Set the icon theme through `gtk::Settings`**, not `IconTheme::set_theme_name`. The
  display's own icon theme refuses to be renamed (`assertion '!self->is_display_singleton'`),
  and the refusal is a `CRITICAL` rather than an error.
- **A hand-parented `PopoverMenu` must be unparented by hand** (`Browser::drop`), or GTK warns
  about a widget finalised with a child still attached.
- **Every drop goes through `DropTargetAsync`**, Okuri's own `application/x-okuri-move` and a
  file manager's `text/uri-list` alike: the payload is read with `Drop::read_future` and drained
  with `read_bytes_future` until empty.
- **A file manager's plain drag offers only `MOVE`.** A target whose actions are `COPY` alone
  never matches, so GTK never tells the compositor the drop is acceptable and the compositor
  cancels it on release — no event, no log line, no highlight. The file target accepts
  `COPY | MOVE` and answers with whichever is offered; nothing is moved either way, and
  Nautilus leaves the original where it was. This cost a full afternoon of reading GTK source
  before a virtual mouse reproduced it in one run.
- **A `DragSource` icon captured strongly by its own closure is a cycle.** Downgrade the widget
  before capturing it.
- **`MultiSelection` keeps a row selected across `items_changed` only if the same object comes
  back.** Replace the object and the selection is gone with it.
- **Shortcuts that would fight an entry live in a local-scope `ShortcutController` on the
  browser**, not in `set_accels_for_action`. BackSpace, Delete, F2, Ctrl+X and Ctrl+V only
  mean anything with the list in front of you, and a dialog's entry must keep them.
- **A `gtk::Switch` handler that stops the signal has to be guarded while the server's answer
  is copied back**, or setting it from the answer asks the server again. `Details::refreshing`
  is that guard, for the permission ticks too.
- **`ListBox` selects the first row when it takes focus.** Not a bug; it is where Enter goes.

## The theme

`theme.rs` writes the palette out as a `CssProvider` at `APPLICATION` priority: every colour
under `@okuri_*` for the rules in `style.css`, and again under Adwaita's names
(`accent_bg_color`, `window_bg_color`, `popover_bg_color`, …) so stock widgets — entries,
switches, dialogs, the file chooser — are painted from the same palette without knowing it.
`StyleManager` is forced dark or light from `palette.dark`, and the icon theme follows
`icons.theme`.

- **`style.css` never names a colour.** There is a test for it. A hex code in a rule is a
  colour that stays put when the theme changes.
- **The user's own `gtk.css` wins.** It loads at `USER` priority, above ours, and a higher
  priority beats a more specific selector. Omarchy ships one per theme; where it says square
  corners, the window gets square corners. That is the theme's call, not something to fight.
- **A palette that cannot be read is left alone**, same as before: mid-switch there is no
  `colors.toml` on disk, and flashing the built-in colours through that gap is worse than
  waiting.

## Things that look wrong and are not

- **FTP has one transfer slot.** One control connection, one command at a time. Raising it only
  queues transfers behind the same mutex while holding more files in memory.
- **Azure's signature includes the account twice on the emulator.** Azurite addresses accounts
  by path, and the canonical resource covers the path as sent. There is a test for it.
- **A drag carries `text/uri-list` only for destinations the desktop can open.** Offering it for
  S3 makes a drop fail instead of being refused.
- **`move_into` consumes nothing.** A drop is offered to more than one target on its way up
  the widget tree, and what it carries comes from the drop itself, so a target that declines
  leaves the real one with everything it needs. Only the end of the drag clears the carry.
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
