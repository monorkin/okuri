use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use camion_core::{
    ByteStream, Capabilities, Details, Permissions, Provider, ProviderExt, RemotePath, Visibility,
};
use camion_providers::{Secret, SecretShape};
use tokio::sync::mpsc;

use crate::config::Connection;
use crate::event::{Answer, Attempt, Concern, Event, Outcome, Prompt, Question};
use crate::known_hosts::KnownHosts;
use crate::secrets::{SecretStore, Vault};
use crate::session::{Session, SessionId};
use crate::transfer::{counting, Endpoint, Place, Transfer, TransferId};
use crate::trust::PromptingTrust;
use crate::{Emitter, Error, Result};

/// Everything the interface can ask for.
#[derive(Debug)]
pub enum Command {
    /// Opens a connection. The [`Attempt`] is minted by whoever asks, so everything this
    /// produces — the questions along the way, the session at the end, the reason there is no
    /// session — can be handed back to them and to nobody else.
    Connect { attempt: Attempt, connection: Box<Connection> },
    Disconnect(SessionId),

    /// Asks for a connection's credentials again and keeps what is given.
    ///
    /// Nothing else ever replaces a stored credential: connecting asks only when there is none,
    /// so a mistyped access key would otherwise be permanent.
    ChangeCredentials { attempt: Attempt, connection: Box<Connection> },

    Open { session: SessionId, path: RemotePath },
    Refresh(SessionId),

    CreateFolder { session: SessionId, name: String },
    Rename { session: SessionId, from: String, to: String },

    /// Puts files somewhere else.
    ///
    /// Where they came from is stated rather than assumed to be whatever is open: a move can be
    /// asked for in one folder and completed in another, which is exactly what cutting and
    /// pasting is, and what dragging somewhere else amounts to.
    ///
    /// Both ends name a connection as well as a folder, because a drag can now leave the window
    /// it started in. Whether that means renaming the files or carrying their bytes across is
    /// the engine's to work out — see [`Running::relocate`].
    Move { from: Place, names: Vec<String>, into: Place },
    Delete { session: SessionId, names: Vec<String> },

    /// Asks for everything else the destination knows about one file.
    Describe { session: SessionId, name: String },

    /// Changes a file's mode.
    SetPermissions { session: SessionId, name: String, mode: u32 },

    /// Asks who can read a file, and what its address is to them.
    Share { session: SessionId, name: String },

    /// Signs a link to a file that works for a while without an account.
    SignLink { session: SessionId, name: String },

    /// Changes who can read a file, and reports where it stands afterwards.
    Reshare { session: SessionId, name: String, public: bool },

    Upload { session: SessionId, into: RemotePath, sources: Vec<PathBuf> },
    Download { session: SessionId, names: Vec<String>, into: PathBuf },

    CancelTransfer(TransferId),
}

impl Command {
    /// Whose work this is, so whatever goes wrong doing it is reported to them.
    pub fn concern(&self) -> Concern {
        match self {
            Self::Connect { attempt, .. } | Self::ChangeCredentials { attempt, .. } => {
                Concern::Attempt(*attempt)
            }

            Self::Disconnect(session)
            | Self::Refresh(session)
            | Self::Open { session, .. }
            | Self::CreateFolder { session, .. }
            | Self::Rename { session, .. }
            | Self::Delete { session, .. }
            | Self::Describe { session, .. }
            | Self::SetPermissions { session, .. }
            | Self::Share { session, .. }
            | Self::SignLink { session, .. }
            | Self::Reshare { session, .. }
            | Self::Upload { session, .. }
            | Self::Download { session, .. } => Concern::Session(*session),

            // The window dropped into, not the one dragged from. It is the one being looked at,
            // and it is the one that ends up wrong if this does not work.
            Self::Move { into, .. } => Concern::Session(into.session),

            Self::CancelTransfer(_) => Concern::Everyone,
        }
    }
}

/// The half of Camion that talks to servers.
///
/// Owns a Tokio runtime on its own thread and is driven entirely by [`Command`]s in and
/// [`Event`]s out, so the interface never waits on a socket and the engine never knows there is
/// an interface at all.
pub struct Engine {
    commands: mpsc::UnboundedSender<Command>,
}

impl Engine {
    pub fn start(secrets: Arc<Vault>, emit: Emitter) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();

        std::thread::Builder::new()
            .name("camion-engine".to_owned())
            .spawn(move || {
                // Not one worker per core, which is what the default gives: this runtime waits
                // on sockets and on a handful of files, and on a large machine that default is
                // dozens of threads with stacks and allocator arenas doing nothing. Enough to
                // keep several transfers moving is all it ever needs.
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .expect("a Tokio runtime");

                runtime.block_on(serve(receiver, secrets, emit));
            })
            .expect("a thread for the engine");

        Self { commands }
    }

    pub fn send(&self, command: Command) {
        // The engine thread lives as long as the app does. If its channel has closed, every
        // command from here on would vanish and the window would simply stop responding.
        self.commands.send(command).expect("the engine thread to be running");
    }
}

async fn serve(
    mut commands: mpsc::UnboundedReceiver<Command>,
    secrets: Arc<Vault>,
    emit: Emitter,
) {
    let engine = Arc::new(Running {
        emit,
        secrets,
        known_hosts: KnownHosts::default_path().unwrap_or_default(),
        sessions: Mutex::new(HashMap::new()),
        running: Mutex::new(HashMap::new()),
    });

    // Each command runs on its own task, so a slow listing on one connection never holds up
    // anything happening on another.
    while let Some(command) = commands.recv().await {
        let engine = Arc::clone(&engine);

        tokio::spawn(async move { engine.handle(command).await });
    }
}

struct Running {
    emit: Emitter,
    secrets: Arc<Vault>,

    /// Where the trusted host keys live. Held as a path rather than as one host-key checker,
    /// because a checker asks questions and a question has to say which connection it is
    /// holding up — so there is one per attempt at connecting.
    known_hosts: PathBuf,
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    /// What is in flight, and on which connection — the session is what tells us when a batch
    /// has finished and the folder underneath it is worth looking at again.
    running: Mutex<HashMap<TransferId, (SessionId, tokio::task::AbortHandle)>>,
}

impl Running {
    async fn handle(self: &Arc<Self>, command: Command) {
        let concern = command.concern();

        let outcome = match command {
            Command::Connect { attempt, connection } => self.connect(attempt, *connection).await,
            Command::Disconnect(session) => self.disconnect(session).await,
            Command::Open { session, path } => self.open(session, path).await,
            Command::Refresh(session) => self.refresh(session).await,
            Command::CreateFolder { session, name } => self.create_folder(session, &name).await,
            Command::Rename { session, from, to } => self.rename(session, &from, &to).await,
            Command::Move { from, names, into } => {
                self.relocate(from, names, into).await
            }
            Command::Delete { session, names } => self.delete(session, names).await,
            Command::Upload { session, into, sources } => {
                self.upload(session, into, sources).await
            }
            Command::Download { session, names, into } => {
                self.download(session, names, into).await
            }
            Command::ChangeCredentials { attempt, connection } => {
                self.change_credentials(attempt, *connection).await
            }
            Command::Describe { session, name } => self.describe(session, &name).await,
            Command::SetPermissions { session, name, mode } => {
                self.set_permissions(session, &name, mode).await
            }
            Command::Share { session, name } => self.share(session, &name, None).await,
            Command::SignLink { session, name } => self.sign_link(session, &name).await,
            Command::Reshare { session, name, public } => {
                let wanted = match public {
                    true => Visibility::Public,
                    false => Visibility::Private,
                };

                self.share(session, &name, Some(wanted)).await
            }
            Command::CancelTransfer(transfer) => {
                self.cancel(transfer);
                Ok(())
            }
        };

        if let Err(error) = outcome {
            self.report(concern, error);
        }
    }

    fn emit(&self, event: Event) {
        (self.emit)(event);
    }

    fn report(&self, concern: Concern, error: Error) {
        self.emit(Event::Failed { concern, message: error.to_string() });
    }

    fn session(&self, id: SessionId) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(Error::NoSuchSession)
    }

    async fn ask(&self, concern: Concern, question: Question) -> Answer {
        let (prompt, answer) = Prompt::new(concern, question);
        self.emit(Event::Ask(prompt));

        answer.await.unwrap_or(Answer::Decline)
    }

    async fn connect(self: &Arc<Self>, attempt: Attempt, connection: Connection) -> Result<()> {
        self.emit(Event::Connecting {
            attempt,
            connection: connection.id.clone(),
        });

        match self.open_connection(attempt, &connection).await {
            Ok(provider) => {
                let session = Arc::new(Session::new(&connection.id, provider));
                let id = session.id;

                self.emit(Event::Connected {
                    attempt,
                    session: id,
                    label: session.provider.label(),
                    // Taken from what the adapter actually implements rather than from what it
                    // declares, so the interface cannot offer something nothing answers.
                    capabilities: Capabilities {
                        sharing: session.provider.sharing().is_some(),
                        permissions: session.provider.permitting().is_some(),
                        details: Details {
                            owning: session.provider.owning().is_some(),
                            linking: session.provider.linking().is_some(),
                            serving: session.provider.serving().is_some(),
                            storing: session.provider.storing().is_some(),
                        },
                        ..session.provider.capabilities()
                    },
                    home: session.provider.home(),
                });

                self.sessions.lock().unwrap().insert(id, session);
                self.open(id, RemotePath::root()).await
            }
            Err(error) => {
                self.emit(Event::ConnectionFailed {
                    attempt,
                    connection: connection.id,
                    reason: error.to_string(),
                });

                Ok(())
            }
        }
    }

    /// Opens a connection, asking for whatever it turns out to need along the way.
    async fn open_connection(
        &self,
        attempt: Attempt,
        connection: &Connection,
    ) -> Result<Arc<dyn Provider>> {
        let concern = Concern::Attempt(attempt);
        let secret = self.secret_for(concern, connection).await?;

        let trust = Arc::new(PromptingTrust::new(
            KnownHosts::new(&self.known_hosts),
            Arc::clone(&self.emit),
            concern,
        )) as Arc<dyn camion_providers::HostTrust>;

        let opened =
            camion_providers::connect(&connection.destination, &secret, Arc::clone(&trust)).await;

        // A key file that turns out to be encrypted is not a failure yet: ask for the
        // passphrase and try the same connection again with it.
        let Err(camion_core::Error::NeedsPassphrase { path }) = opened else {
            return Ok(opened?);
        };

        let secret = self.ask_for_passphrase(concern, connection, path).await?;

        Ok(camion_providers::connect(&connection.destination, &secret, trust).await?)
    }

    /// Reports who can read a file, having first changed it if `wanted` says so.
    ///
    /// Reading it back rather than assuming the change took is the point: a store that will not
    /// do per-file access says so here, and a bucket policy can leave a file readable that we
    /// have just marked private.
    async fn share(
        &self,
        id: SessionId,
        name: &str,
        wanted: Option<Visibility>,
    ) -> Result<()> {
        let session = self.session(id)?;
        let path = session.path().join(name)?;

        let sharing = session
            .provider
            .sharing()
            .ok_or_else(|| Error::config("this destination does not share files"))?;

        if let Some(wanted) = wanted {
            sharing.set_visibility(&path, wanted).await?;
        }

        // Read back rather than assumed, and a refusal to say is reported as not knowing rather
        // than raised: opening a file to look at it must not fill the window with an error.
        let (public, why_not) = match sharing.visibility(&path).await {
            Ok(visibility) => (Some(visibility.is_public()), String::new()),
            Err(error) => (None, error.to_string()),
        };

        self.emit(Event::Shared {
            session: id,
            name: name.to_owned(),
            public,
            why_not,
            url: sharing.public_url(&path),
        });

        Ok(())
    }

    /// Everything else the destination knows about one file.
    ///
    /// Each part is asked of whichever small trait answers it, and a destination that answers
    /// none of them simply says nothing. A part that fails is left out rather than failing the
    /// lot: looking at a file must not turn into an error because one header could not be read.
    async fn describe(&self, id: SessionId, name: &str) -> Result<()> {
        let session = self.session(id)?;
        let path = session.path().join(name)?;
        let provider = &session.provider;

        let ownership = match provider.owning() {
            Some(owning) => owning.ownership(&path).await.ok(),
            None => None,
        };

        let link_target = match provider.linking() {
            Some(linking) => linking.link_target(&path).await.ok().flatten(),
            None => None,
        };

        let served = match provider.serving() {
            Some(serving) => serving.served(&path).await.ok(),
            None => None,
        };

        let stored = match provider.storing() {
            Some(storing) => storing.stored(&path).await.ok(),
            None => None,
        };

        self.emit(Event::Described {
            session: id,
            name: name.to_owned(),
            ownership,
            link_target,
            served,
            stored,
        });

        Ok(())
    }

    /// Changes a file's mode, then lists the folder so the change is on screen.
    async fn set_permissions(&self, id: SessionId, name: &str, mode: u32) -> Result<()> {
        let session = self.session(id)?;
        let path = session.path().join(name)?;

        let permitting = session
            .provider
            .permitting()
            .ok_or_else(|| Error::config("this destination does not keep file permissions"))?;

        permitting.set_permissions(&path, Permissions(mode)).await?;
        self.refresh(id).await
    }

    /// Signs a link that works for a week, which is as long as a signature is allowed to last.
    async fn sign_link(&self, id: SessionId, name: &str) -> Result<()> {
        let session = self.session(id)?;
        let path = session.path().join(name)?;

        let sharing = session
            .provider
            .sharing()
            .ok_or_else(|| Error::config("this destination does not share files"))?;

        let link = sharing.temporary_url(&path, LINK_LASTS).await?;

        self.emit(Event::Linked { session: id, name: name.to_owned(), url: link });

        Ok(())
    }

    /// Replaces what is stored for a connection, asking for it exactly as connecting would.
    ///
    /// Typing an access key wrong is ordinary. Without this the only way to correct one is to
    /// go into the desktop's keyring by hand, because a stored credential is never asked about
    /// again. Declining changes nothing, so a dialog closed by accident is not destructive.
    async fn change_credentials(&self, attempt: Attempt, connection: Connection) -> Result<()> {
        let concern = Concern::Attempt(attempt);
        let shape = connection.destination.secret_shape();

        if shape == SecretShape::None {
            return Err(Error::config(format!("{} needs no credentials", connection.name)));
        }

        let secrets = self.secrets(concern).await?;
        let Some(secret) = self.ask_for_secret(concern, &connection, shape).await else {
            return Ok(());
        };

        secrets.set(&connection.id, &secret)?;

        self.emit(Event::Notice {
            concern,
            message: format!("Saved new credentials for {}.", connection.name),
        });

        Ok(())
    }

    /// The store, opening it first if it is still locked.
    ///
    /// Asked for here rather than at start-up, because this is the first moment a passphrase is
    /// worth anything to anybody: something wants a credential. A wrong one is asked again
    /// rather than failing the connection, which is what anyone mistyping expects.
    async fn secrets(&self, concern: Concern) -> Result<Arc<dyn SecretStore>> {
        loop {
            if let Some(store) = self.secrets.store() {
                return Ok(store);
            }

            let answer = self.ask(concern, Question::Passphrase).await;

            let Some(passphrase) = answer.text() else {
                return Err(Error::Cancelled);
            };

            match self.secrets.unlock(passphrase) {
                Ok(store) => return Ok(store),
                Err(Error::WrongPassphrase) => self.report(concern, Error::WrongPassphrase),
                Err(error) => return Err(error),
            }
        }
    }

    /// The secret this destination needs, from the store if it is there and from the person at
    /// the keyboard if it is not.
    async fn secret_for(&self, concern: Concern, connection: &Connection) -> Result<Secret> {
        let secrets = self.secrets(concern).await?;
        let stored = secrets.get(&connection.id)?;
        let shape = connection.destination.secret_shape();

        if shape == SecretShape::None || !stored.is_none() {
            return Ok(stored);
        }

        let Some(secret) = self.ask_for_secret(concern, connection, shape).await else {
            return Err(Error::Cancelled);
        };

        secrets.set(&connection.id, &secret)?;

        Ok(secret)
    }

    /// Asks for whatever a destination signs in with, or `None` if the question is declined.
    async fn ask_for_secret(
        &self,
        concern: Concern,
        connection: &Connection,
        shape: SecretShape,
    ) -> Option<Secret> {
        let name = connection.name.clone();

        // What is asked for follows what the destination needs: one field for a password, two
        // for the key pairs the object stores want.
        let answer = match shape {
            SecretShape::KeyPair => {
                self.ask(concern, Question::KeyPair { connection: name }).await
            }
            _ => self.ask(concern, Question::Password { connection: name }).await,
        };

        match (answer.text(), answer.pair()) {
            (Some(password), _) => Some(Secret::Password(password.to_owned())),
            (_, Some((id, key))) => Some(Secret::KeyPair {
                id: id.to_owned(),
                secret: key.to_owned(),
            }),
            _ => None,
        }
    }

    async fn ask_for_passphrase(
        &self,
        concern: Concern,
        connection: &Connection,
        path: String,
    ) -> Result<Secret> {
        let answer = self.ask(concern, Question::KeyPassphrase { path }).await;

        let Some(passphrase) = answer.text() else {
            return Err(Error::Cancelled);
        };

        let secret = Secret::Password(passphrase.to_owned());
        self.secrets(concern).await?.set(&connection.id, &secret)?;

        Ok(secret)
    }

    async fn disconnect(&self, id: SessionId) -> Result<()> {
        let session = self.sessions.lock().unwrap().remove(&id);

        if let Some(session) = session {
            // The connection is gone from the registry either way, but a server that would not
            // let go is worth saying out loud rather than leaving a half-closed socket behind.
            if let Err(error) = session.provider.disconnect().await {
                self.report(Concern::Session(id), error.into());
            }

            self.emit(Event::Disconnected { session: id });
        }

        Ok(())
    }

    /// Lists the folder a connection is already looking at, which is what everything that
    /// changes the contents of one asks for afterwards.
    async fn refresh(&self, id: SessionId) -> Result<()> {
        let path = self.session(id)?.path();

        self.open(id, path).await
    }

    async fn open(&self, id: SessionId, path: RemotePath) -> Result<()> {
        let session = self.session(id)?;
        let navigation = session.navigating();

        self.emit(Event::Working { session: id, working: true });
        let listing = session.provider.list(&path).await;
        self.emit(Event::Working { session: id, working: false });

        // Somewhere else was asked for while this was in flight, so this answer is about a
        // folder nobody is waiting for any more. Showing it would move the window on its own.
        if !session.is_current(navigation) {
            return Ok(());
        }

        match listing {
            Ok(entries) => {
                session.move_to(path.clone());
                self.emit(Event::Listing { session: id, path, entries });
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn create_folder(&self, id: SessionId, name: &str) -> Result<()> {
        let session = self.session(id)?;
        let path = session.path().join(name)?;

        session.provider.create_folder(&path).await?;
        self.refresh(id).await
    }

    async fn rename(&self, id: SessionId, from: &str, to: &str) -> Result<()> {
        let session = self.session(id)?;
        let folder = session.path();

        session
            .provider
            .rename(&folder.join(from)?, &folder.join(to)?)
            .await?;

        self.open(id, folder).await
    }

    /// Puts files somewhere else, by whichever means the two ends allow.
    ///
    /// The two cases are genuinely different work and the difference is not which window the
    /// files were dragged from — it is which *server* they are on. Two windows open on the same
    /// saved connection are two sessions to one machine, and asking that machine to rename a
    /// file is right whichever of the two windows asked. Two different machines share nothing,
    /// and the bytes have to travel.
    async fn relocate(
        self: &Arc<Self>,
        from: Place,
        names: Vec<String>,
        into: Place,
    ) -> Result<()> {
        let source = self.session(from.session)?;
        let target = self.session(into.session)?;

        for name in &names {
            // Moving a folder inside itself would take the tree with it. Only ever possible on
            // one server, since two of them share no paths. The provider refuses this too, but
            // saying so here means the same words whichever destination it is.
            if source.connection == target.connection && into.path.starts_with(&from.path.join(name)?) {
                return Err(Error::config(format!("{name} cannot be moved inside itself")));
            }
        }

        if source.connection == target.connection {
            self.rename_into(&source, &from, names, &into).await?;
        } else {
            self.carry_across(&source, &from, names, &target, &into).await?;
        }

        Ok(())
    }

    /// The same server, so the files never move: only what they are called does.
    ///
    /// Both connections are redrawn rather than only the one dropped into. They are two views
    /// of one machine, and a file that has left the folder the other window is looking at has
    /// left it whether or not that window was told.
    async fn rename_into(
        &self,
        source: &Arc<Session>,
        from: &Place,
        names: Vec<String>,
        into: &Place,
    ) -> Result<()> {
        if into.path == from.path {
            return Ok(());
        }

        for name in names {
            source
                .provider
                .rename(&from.path.join(&name)?, &into.path.join(&name)?)
                .await?;
        }

        self.refresh(from.session).await?;

        if into.session != from.session {
            self.refresh(into.session).await?;
        }

        Ok(())
    }

    /// Two different servers, so the bytes have to travel.
    ///
    /// A copy rather than a move, the way dragging between two disks is a copy: the file is
    /// still on the machine it came from, and taking it away would mean deleting somebody's
    /// only copy on the strength of a gesture that can be made by accident.
    ///
    /// The source folder is left alone and only the destination is redrawn, which
    /// [`Running::start`] does once the last file has landed.
    async fn carry_across(
        self: &Arc<Self>,
        source: &Arc<Session>,
        from: &Place,
        names: Vec<String>,
        target: &Arc<Session>,
        into: &Place,
    ) -> Result<()> {
        // What is already there, asked once for the whole drop rather than once per file, for
        // the same reason an upload asks once: a miss costs a round trip and there may be
        // thousands of them before a single byte moves.
        let mut taken = target
            .provider
            .list(&into.path)
            .await?
            .into_iter()
            .map(|entry| entry.name)
            .collect::<std::collections::HashSet<String>>();

        let mut files = Vec::new();

        for name in names {
            let Some(arriving) = self.arriving_as(into.session, name.clone(), &taken).await? else {
                continue;
            };

            taken.insert(arriving.clone());

            self.spread(
                source,
                from.path.join(&name)?,
                target,
                into.path.join(&arriving)?,
                &mut files,
            )
            .await?;
        }

        for file in files {
            let (at, to) = (file.from.clone(), file.to.clone());

            let mut transfer = Transfer::new(
                Endpoint::Remote { session: source.id, path: at.clone() },
                Endpoint::Remote { session: target.id, path: to.clone() },
            );

            transfer.total = file.size;

            let (reading, writing) = (Arc::clone(source), Arc::clone(target));

            self.start(transfer, &[Arc::clone(source), Arc::clone(target)], move |progress| {
                carry(reading, at, writing, to, file, progress)
            });
        }

        Ok(())
    }

    /// Walks the source tree, making the folders on the far side, and collects the files.
    ///
    /// A folder is not a thing that can be transferred, it is a shape. The folders are made
    /// even where they turn out to hold nothing, so an empty one still arrives — the same
    /// promise downloading a folder makes.
    async fn spread(
        &self,
        source: &Arc<Session>,
        at: RemotePath,
        target: &Arc<Session>,
        to: RemotePath,
        files: &mut Vec<Crossing>,
    ) -> Result<()> {
        let entry = source.provider.stat(&at).await?;

        if entry.kind.is_file() {
            files.push(Crossing {
                permissions: entry.permissions,
                from: at,
                to,
                size: Some(entry.size),
            });

            return Ok(());
        }

        // Already being there is the state this is asking for, so it is not a failure. Replacing
        // a folder is what the person answered "replace" to, and the files inside it are what
        // actually get replaced.
        match target.provider.create_folder(&to).await {
            Ok(()) | Err(camion_core::Error::AlreadyExists { .. }) => {}
            Err(error) => return Err(error.into()),
        }

        for child in source.provider.list(&at).await? {
            Box::pin(self.spread(
                source,
                at.join(&child.name)?,
                target,
                to.join(&child.name)?,
                files,
            ))
            .await?;
        }

        Ok(())
    }

    async fn delete(&self, id: SessionId, names: Vec<String>) -> Result<()> {
        let session = self.session(id)?;
        let folder = session.path();

        for name in names {
            session
                .provider
                .delete_recursively(&folder.join(&name)?)
                .await?;
        }

        self.open(id, folder).await
    }

    async fn upload(
        self: &Arc<Self>,
        id: SessionId,
        into: RemotePath,
        sources: Vec<PathBuf>,
    ) -> Result<()> {
        let session = self.session(id)?;

        // What is already in the folder, asked once for the whole drop.
        //
        // Asking per file instead means a round trip each — two on the object stores, where a
        // miss costs a HEAD and then a listing, and a whole directory listing each on FTP. A
        // few thousand small files spend minutes on that before a single byte moves.
        let mut taken = session
            .provider
            .list(&into)
            .await?
            .into_iter()
            .map(|entry| entry.name)
            .collect::<std::collections::HashSet<_>>();

        for source in sources {
            // A path with nothing on the end of it would upload as a file with no name, which
            // every destination reads as a folder marker or refuses outright.
            let name = source
                .file_name()
                .ok_or_else(|| Error::local_file(&source, "it has no file name"))?
                .to_string_lossy()
                .into_owned();

            let Some(name) = self.arriving_as(id, name, &taken).await? else {
                continue;
            };

            // Asked once and carried, rather than here and again when the file is opened.
            let size = tokio::fs::metadata(&source).await.ok().map(|metadata| metadata.len());

            // Held so that dropping two files of the same name in one go does not quietly
            // become one file.
            taken.insert(name.clone());

            let destination = into.join(&name)?;
            let mut transfer = Transfer::new(
                Endpoint::Local(source.clone()),
                Endpoint::Remote { session: id, path: destination.clone() },
            );

            transfer.total = size;

            let writing = Arc::clone(&session);

            self.start(transfer, std::slice::from_ref(&session), move |progress| {
                send_up(writing, source, destination, size, progress)
            });
        }

        Ok(())
    }

    /// What an uploaded file ends up called, once whatever is already there has been taken
    /// into account.
    ///
    /// `None` means it is not to be uploaded at all. Overwriting without asking is how an
    /// afternoon's work goes missing under a file of the same name from a downloads folder.
    ///
    /// `taken` is what the folder held when the drop began, which is what the answer is about.
    async fn arriving_as(
        &self,
        id: SessionId,
        name: String,
        taken: &std::collections::HashSet<String>,
    ) -> Result<Option<String>> {
        if !taken.contains(&name) {
            return Ok(Some(name));
        }

        let asked = self
            .ask(Concern::Session(id), Question::Overwrite { name: name.clone() })
            .await;

        match asked {
            Answer::Accept => Ok(Some(name)),
            Answer::KeepBoth => Ok(Some(beside(&name, taken))),
            _ => Ok(None),
        }
    }

    async fn download(
        self: &Arc<Self>,
        id: SessionId,
        names: Vec<String>,
        into: PathBuf,
    ) -> Result<()> {
        let session = self.session(id)?;

        for file in self.plan(&session, names, into).await? {
            let (source, destination) = (file.source, file.destination);
            let mut transfer = Transfer::new(
                Endpoint::Remote { session: id, path: source.clone() },
                Endpoint::Local(destination.clone()),
            );

            transfer.total = file.size;

            let reading = Arc::clone(&session);

            self.start(transfer, std::slice::from_ref(&session), move |progress| {
                bring_down(reading, source, destination, progress)
            });
        }

        Ok(())
    }

    /// Turns what was asked for into the files that actually have to come down.
    ///
    /// A folder is not a thing that can be transferred, it is a shape: this walks it, makes the
    /// directories on the way, and returns the files inside. Downloading a folder is what
    /// people mean when they drag one, so it is what dragging one does.
    async fn plan(
        &self,
        session: &Arc<Session>,
        names: Vec<String>,
        into: PathBuf,
    ) -> Result<Vec<Planned>> {
        let folder = session.path();
        let mut files = Vec::new();

        for name in names {
            self.walk(session, folder.join(&name)?, into.join(&name), &mut files)
                .await?;
        }

        Ok(files)
    }

    async fn walk(
        &self,
        session: &Arc<Session>,
        source: RemotePath,
        destination: PathBuf,
        files: &mut Vec<Planned>,
    ) -> Result<()> {
        let entry = session.provider.stat(&source).await?;

        if entry.kind.is_file() {
            files.push(Planned { source, destination, size: Some(entry.size) });

            return Ok(());
        }

        // The folder is made even when it holds nothing, so an empty one still arrives.
        tokio::fs::create_dir_all(&destination).await.map_err(|error| {
            Error::local_file(&destination, error)
        })?;

        for child in session.provider.list(&source).await? {
            Box::pin(self.walk(
                session,
                source.join(&child.name)?,
                destination.join(&child.name),
                files,
            ))
            .await?;
        }

        Ok(())
    }

    /// Queues one transfer and runs it as soon as the connection has a free slot.
    ///
    /// The work is handed a way to report progress rather than returning something for the
    /// engine to measure, because only the transfer itself knows where the bytes are flowing.
    fn start<Work, Run>(
        self: &Arc<Self>,
        transfer: Transfer,
        holding: &[Arc<Session>],
        work: Work,
    ) -> tokio::task::JoinHandle<Outcome>
    where
        Work: FnOnce(Progress) -> Run + Send + 'static,
        Run: std::future::Future<Output = Result<()>> + Send,
    {
        let id = transfer.id;
        let engine = Arc::clone(self);

        // Every transfer has a connection at one end — nothing here moves a local file to
        // another local file — and this is the one whose folder is about to change.
        let on = transfer.session().expect("a transfer with a connection at one end");

        // A slot on every connection this occupies, taken in a fixed order.
        //
        // Both, for a transfer between two servers: it is using both, and counting only one end
        // lets a fast server flood a slow one through a queue that thinks it is idle. The order
        // is by connection rather than by which end is which, so files being dragged both ways
        // at once cannot each hold the slot the other is waiting for.
        let mut slots = holding
            .iter()
            .map(|session| (session.id, session.transfer_slots()))
            .collect::<Vec<_>>();

        slots.sort_by_key(|(session, _)| *session);
        slots.dedup_by_key(|(session, _)| *session);

        let slots = slots.into_iter().map(|(_, slots)| slots).collect::<Vec<_>>();

        // A download changes nothing on the server, so only work that lands there is worth
        // looking at the folder again for.
        let lands_remotely = matches!(transfer.to, Endpoint::Remote { .. });

        self.emit(Event::TransferAdded(transfer));

        let task = tokio::spawn(async move {
            let mut held = Vec::with_capacity(slots.len());

            for slot in &slots {
                held.push(slot.acquire().await);
            }

            let reporter = Arc::clone(&engine);
            let progress: Progress = Arc::new(move |transferred| {
                reporter.emit(Event::TransferProgress { transfer: id, transferred });
            });

            let outcome = match work(progress).await {
                Ok(()) => Outcome::Done,
                Err(error) => Outcome::Failed(error.to_string()),
            };

            engine.running.lock().unwrap().remove(&id);
            engine.emit(Event::TransferFinished { transfer: id, outcome: outcome.clone() });

            // Dropping ten files should redraw the list once, when the last one lands — not
            // ten times, and not only when the person thinks to press refresh.
            if engine.session(on).is_ok() && lands_remotely && !engine.still_working(on)
                && let Err(error) = engine.refresh(on).await
            {
                engine.report(Concern::Session(on), error);
            }

            outcome
        });

        self.running.lock().unwrap().insert(id, (on, task.abort_handle()));

        task
    }

    /// Whether this connection still has transfers in flight.
    fn still_working(&self, session: SessionId) -> bool {
        self.running
            .lock()
            .unwrap()
            .values()
            .any(|(on, _)| *on == session)
    }

    fn cancel(&self, id: TransferId) {
        if let Some((_, task)) = self.running.lock().unwrap().remove(&id) {
            task.abort();
            self.emit(Event::TransferFinished { transfer: id, outcome: Outcome::Cancelled });
        }
    }
}

/// A name like `name` that nothing in `taken` is using, by adding ` (2)`, ` (3)`, and so on
/// before the extension. What "keep both" means.
fn beside(name: &str, taken: &std::collections::HashSet<String>) -> String {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };

    let mut suffix = 2;

    loop {
        let candidate = match extension {
            Some(extension) => format!("{stem} ({suffix}).{extension}"),
            None => format!("{stem} ({suffix})"),
        };

        if !taken.contains(&candidate) {
            return candidate;
        }

        suffix += 1;
    }
}

/// One file on its way from one server to another, and everything worth taking with it.
///
/// What travels is what describes the file: how big it is, what it is, how it was encoded, and
/// who may do what with it. What does not travel is what describes where it lives — an ETag, a
/// storage class, a version. Those belong to the store that made them, and copying them to
/// another store would be stating something untrue about it.
struct Crossing {
    from: RemotePath,
    to: RemotePath,
    size: Option<u64>,

    /// The mode, where the source has one. Losing it turns a script into a file that will not
    /// run, and there is nothing on the far side to tell you it used to.
    ///
    /// Carried here rather than read off the download the way the content type is, because no
    /// protocol puts a Unix mode in a response body — it comes from the listing, which has
    /// already been asked for.
    permissions: Option<Permissions>,
}

/// One file that has to come down, and where it goes.
struct Planned {
    source: RemotePath,
    destination: PathBuf,
    size: Option<u64>,
}

/// How long a signed link stays good for.
///
/// A week, which is the longest a signature is allowed to last. Anything a person means to
/// share is worth more than an hour, and a link that has quietly expired is worse than one that
/// was never made.
const LINK_LASTS: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// How much of a file is read at a time on its way up.
///
/// Large enough that a protocol acknowledging each write still fills the link, small enough
/// that a handful of transfers at once is not worth noticing.
const READ_CHUNK: usize = 256 * 1024;

/// How a running transfer says how far it has got.
pub type Progress = Arc<dyn Fn(u64) + Send + Sync>;

async fn send_up(
    session: Arc<Session>,
    source: PathBuf,
    destination: RemotePath,
    size: Option<u64>,
    progress: Progress,
) -> Result<()> {
    let file = tokio::fs::File::open(&source)
        .await
        .map_err(|error| Error::local_file(&source, error))?;

    // Read in pieces worth sending. The default is four kilobytes, and since every provider
    // writes what it is handed, that becomes the size of what goes on the wire — which on a
    // protocol that waits for each write to be acknowledged sets the speed of the whole
    // transfer from the round trip time rather than from the bandwidth.
    let reader = tokio_util::io::ReaderStream::with_capacity(file, READ_CHUNK);
    let chunks = futures::StreamExt::map(reader, |chunk| {
        chunk.map_err(|error| camion_core::Error::caused_by("could not read the file", error))
    });

    let body = counting(ByteStream::new(chunks, size), move |transferred| {
        progress(transferred)
    });

    Ok(session.provider.write(&destination, body).await?)
}

/// Moves one file from one server straight to another.
///
/// Nothing touches the disk and nothing waits for the whole file. The read stream is handed to
/// the write, so a chunk arrives from one connection and leaves on the other, and what is held
/// at any moment is a chunk rather than a file. A hundred-gigabyte object crosses in as much
/// memory as a small one.
///
/// The size goes across with it, and that matters more than it looks: a destination told how
/// big a file is can write it in a single request, while one that is not has to split it into
/// parts or hold it to find out.
async fn carry(
    source: Arc<Session>,
    at: RemotePath,
    target: Arc<Session>,
    to: RemotePath,
    file: Crossing,
    progress: Progress,
) -> Result<()> {
    let stream = source.provider.read(&at, None).await?;
    let known = stream.size().or(file.size);

    // What the source said about the file, which the download response has already told us.
    // Asking the source again would be a round trip per file for something in hand.
    let serve = stream.serve().clone();

    let body = counting(
        ByteStream::new(stream, known).served_as(serve),
        move |transferred| progress(transferred),
    );

    target.provider.write(&to, body).await?;

    // Afterwards, because a file has to exist before its mode can be set, and only where both
    // ends have one — an object store has no modes and an SFTP server has nothing else.
    if let (Some(permissions), Some(permitting)) = (file.permissions, target.provider.permitting())
    {
        permitting.set_permissions(&to, permissions).await?;
    }

    Ok(())
}

async fn bring_down(
    session: Arc<Session>,
    source: RemotePath,
    destination: PathBuf,
    progress: Progress,
) -> Result<()> {
    let stream = session.provider.read(&source, None).await?;
    let counted = counting(stream, move |transferred| progress(transferred));

    let mut reader = tokio_util::io::StreamReader::new(futures::StreamExt::map(
        counted,
        |chunk| chunk.map_err(|error| std::io::Error::other(error.to_string())),
    ));

    let mut file = tokio::fs::File::create(&destination).await.map_err(|error| {
        Error::local_file(&destination, error)
    })?;

    tokio::io::copy(&mut reader, &mut file).await.map_err(|error| {
        Error::local_file(&destination, error)
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn keeping_both_puts_a_number_before_the_extension() {
        assert_eq!(beside("report.pdf", &taken(&["report.pdf"])), "report (2).pdf");
        assert_eq!(beside("README", &taken(&["README"])), "README (2)");
    }

    /// Dropping the same file three times in a row has to produce three files, so each answer
    /// has to step over the ones before it.
    #[test]
    fn a_name_already_kept_twice_gets_the_next_number() {
        let folder = taken(&["report.pdf", "report (2).pdf", "report (3).pdf"]);

        assert_eq!(beside("report.pdf", &folder), "report (4).pdf");
    }

    /// A leading dot is a name, not an extension: `.bashrc` must not become ` (2).bashrc`.
    #[test]
    fn a_dotfile_keeps_its_whole_name() {
        assert_eq!(beside(".bashrc", &taken(&[".bashrc"])), ".bashrc (2)");
    }
}
