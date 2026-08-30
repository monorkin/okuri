# Camion

Remote storage that feels like a folder. One window, one file list, drag files in from your
file manager, use the shortcuts you already know.

Rust and Qt 6, for Linux.

## Destinations

| Destination | Notes |
| --- | --- |
| SFTP | Signs in the way `ssh` does: the agent, then the usual key files. Encrypted keys are asked about only when they turn out to need it. Unknown host keys are checked against `~/.ssh/known_hosts`. |
| FTP / FTPS | Explicit FTPS by default. |
| Amazon S3 | Multipart uploads. |
| Cloudflare R2 | The S3 client with a preset endpoint. |
| Backblaze B2 | The S3 client with a preset endpoint. |
| WebDAV | PROPFIND / GET / PUT / MKCOL / MOVE, hand-rolled. |
| Azure Blob Storage | Shared Key signing, so the account key from the portal is enough. |

Every one of them implements the same small trait and is graded by the same conformance suite.
What a destination cannot do natively it either emulates or declares unsupported, and the
interface reads that rather than discovering it from a failed click — renaming a folder on an
object store is a copy and a delete, and Camion says so before it starts.

## Running it

```sh
cargo run                  # the connection picker
cargo run -- production    # open a saved connection straight away
```

Connections live in `~/.config/camion/connections.toml`, which is meant to be read and edited by
hand. It holds hosts, buckets, and usernames — never a secret. Passwords and keys go to the
desktop's keyring, or to a passphrase-encrypted file when no keyring is running.

## Moving files around

Dragging inside the window moves things on the server: onto a folder to put them in it, or onto
a folder in the breadcrumb to send them back up a level. Holding over either for a moment opens
it and the drag carries on inside, so a file can go from one branch of the tree to another
without ever being let go of.

Nothing is transferred — it is a rename — so it is immediate, and on the object stores it is the
copy-and-delete their capabilities already warn about. `Ctrl+X` and `Ctrl+V` do the same thing
without the mouse, including across folders.

## Dragging files out

A drag is a system drag from the moment it starts — it has to be, because once the pointer
leaves the window the compositor owns it and the application hears nothing more. So what a drag
carries is settled when it begins, and it is always a *copy*: a file manager offered a move
would take the original off the server.

- **SFTP, FTP, WebDAV** — the address of the file itself (`sftp://…`). GNOME and KDE can both
  open these, but **only once the location is mounted**; neither will mount one in the middle of
  a drop, and dropping onto an unmounted server fails with "the specified location is not
  mounted". Mount it first and the copy runs in the file manager's own progress window, however
  large the file is:

  ```sh
  gio mount sftp://user@host/
  ```

- **S3, R2, B2, Azure** — there is no address the desktop understands, so a drag out of one
  lands nowhere. Use Download instead.

Moving files *inside* the window is unaffected by any of this, and works everywhere.

## Appearance

Camion follows the desktop. On Omarchy it reads the current theme's palette *and* its icon
theme, and repaints when you switch themes without a restart. Elsewhere it uses a built-in
light or dark set, chosen from the desktop's own preference, and the icon theme GTK is set to.

How the list is shown — grid or rows, icon size, sort order, which columns — lives in
`~/.config/camion/view.toml` and is remembered between sessions.

## Testing

```sh
cargo test --workspace
```

That covers everything that does not need a server, including the conformance suite run against
an in-memory provider.

For the real thing:

```sh
docker compose -f test/compose.yaml up -d
cargo test --workspace -- --ignored
```

That runs the same conformance suite against OpenSSH, vsftpd, MinIO, Apache mod_dav, and
Azurite. A check that a connection genuinely cannot support is skipped by reading its
capabilities — never by editing the suite — so "SFTP passes and S3 does not" is always a bug.

## Shape

```
crates/
  camion-core/       paths, entries, capabilities, the Provider trait, the conformance suite
  camion-providers/  the seven destinations and the S3 preset table
  camion-engine/     sessions, transfers, configuration, secrets, known_hosts
  camion/            the Qt application
```

Dependencies only point downwards. `camion-core` knows nothing about HTTP or SSH, and the
interface knows nothing about either — it sends commands and receives events on one channel,
and never waits on a socket.
