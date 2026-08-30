use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use camion_core::{ByteStream, Provider, ProviderExt, RemotePath};
use camion_providers::{Secret, SecretShape};
use tokio::sync::mpsc;

use crate::config::Connection;
use crate::event::{Answer, Event, Outcome, Prompt, Question};
use crate::known_hosts::KnownHosts;
use crate::secrets::{SecretStore, Vault};
use crate::session::{Session, SessionId};
use crate::transfer::{counting, Endpoint, Transfer, TransferId};
use crate::trust::PromptingTrust;
use crate::{Emitter, Error, Result};

/// Everything the interface can ask for.
#[derive(Debug)]
pub enum Command {
    Connect(Box<Connection>),
    Disconnect(SessionId),

    Open { session: SessionId, path: RemotePath },
    Refresh(SessionId),

    CreateFolder { session: SessionId, name: String },
    Rename { session: SessionId, from: String, to: String },

    /// Moves files between two folders on the same connection.
    ///
    /// Where they came from is stated rather than assumed to be whatever is open: a move can be
    /// asked for in one folder and completed in another, which is exactly what cutting and
    /// pasting is, and what dragging somewhere else amounts to.
    Move { session: SessionId, from: RemotePath, names: Vec<String>, into: RemotePath },
    Delete { session: SessionId, names: Vec<String> },

    Upload { session: SessionId, into: RemotePath, sources: Vec<PathBuf> },
    Download { session: SessionId, names: Vec<String>, into: PathBuf },

    CancelTransfer(TransferId),
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
                let runtime = tokio::runtime::Builder::new_multi_thread()
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
    let trust = Arc::new(PromptingTrust::new(
        KnownHosts::new(KnownHosts::default_path().unwrap_or_default()),
        Arc::clone(&emit),
    ));

    let engine = Arc::new(Running {
        emit,
        secrets,
        trust,
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
    trust: Arc<PromptingTrust>,
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    /// What is in flight, and on which connection — the session is what tells us when a batch
    /// has finished and the folder underneath it is worth looking at again.
    running: Mutex<HashMap<TransferId, (SessionId, tokio::task::AbortHandle)>>,
}

impl Running {
    async fn handle(self: &Arc<Self>, command: Command) {
        let outcome = match command {
            Command::Connect(connection) => self.connect(*connection).await,
            Command::Disconnect(session) => self.disconnect(session).await,
            Command::Open { session, path } => self.open(session, path).await,
            Command::Refresh(session) => self.refresh(session).await,
            Command::CreateFolder { session, name } => self.create_folder(session, &name).await,
            Command::Rename { session, from, to } => self.rename(session, &from, &to).await,
            Command::Move { session, from, names, into } => {
                self.move_to(session, from, names, into).await
            }
            Command::Delete { session, names } => self.delete(session, names).await,
            Command::Upload { session, into, sources } => {
                self.upload(session, into, sources).await
            }
            Command::Download { session, names, into } => {
                self.download(session, names, into).await
            }
            Command::CancelTransfer(transfer) => {
                self.cancel(transfer);
                Ok(())
            }
        };

        if let Err(error) = outcome {
            self.report(error);
        }
    }

    fn emit(&self, event: Event) {
        (self.emit)(event);
    }

    fn report(&self, error: Error) {
        self.emit(Event::Failed { message: error.to_string() });
    }

    fn session(&self, id: SessionId) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(Error::NoSuchSession)
    }

    async fn ask(&self, question: Question) -> Answer {
        let (prompt, answer) = Prompt::new(question);
        self.emit(Event::Ask(prompt));

        answer.await.unwrap_or(Answer::Decline)
    }

    async fn connect(self: &Arc<Self>, connection: Connection) -> Result<()> {
        self.emit(Event::Connecting { connection: connection.id.clone() });

        match self.open_connection(&connection).await {
            Ok(provider) => {
                let session = Arc::new(Session::new(&connection.id, provider));
                let id = session.id;

                self.emit(Event::Connected {
                    session: id,
                    label: session.provider.label(),
                    capabilities: session.provider.capabilities(),
                    home: session.provider.home(),
                });

                self.sessions.lock().unwrap().insert(id, session);
                self.open(id, RemotePath::root()).await
            }
            Err(error) => {
                self.emit(Event::ConnectionFailed {
                    connection: connection.id,
                    reason: error.to_string(),
                });

                Ok(())
            }
        }
    }

    /// Opens a connection, asking for whatever it turns out to need along the way.
    async fn open_connection(&self, connection: &Connection) -> Result<Arc<dyn Provider>> {
        let secret = self.secret_for(connection).await?;
        let trust = Arc::clone(&self.trust) as Arc<dyn camion_providers::HostTrust>;
        let opened =
            camion_providers::connect(&connection.destination, &secret, Arc::clone(&trust)).await;

        // A key file that turns out to be encrypted is not a failure yet: ask for the
        // passphrase and try the same connection again with it.
        let Err(camion_core::Error::NeedsPassphrase { path }) = opened else {
            return Ok(opened?);
        };

        let secret = self.ask_for_passphrase(connection, path).await?;

        Ok(camion_providers::connect(&connection.destination, &secret, trust).await?)
    }

    /// The store, opening it first if it is still locked.
    ///
    /// Asked for here rather than at start-up, because this is the first moment a passphrase is
    /// worth anything to anybody: something wants a credential. A wrong one is asked again
    /// rather than failing the connection, which is what anyone mistyping expects.
    async fn secrets(&self) -> Result<Arc<dyn SecretStore>> {
        loop {
            if let Some(store) = self.secrets.store() {
                return Ok(store);
            }

            let answer = self.ask(Question::Passphrase).await;

            let Some(passphrase) = answer.text() else {
                return Err(Error::Cancelled);
            };

            match self.secrets.unlock(passphrase) {
                Ok(store) => return Ok(store),
                Err(Error::WrongPassphrase) => self.report(Error::WrongPassphrase),
                Err(error) => return Err(error),
            }
        }
    }

    /// The secret this destination needs, from the store if it is there and from the person at
    /// the keyboard if it is not.
    async fn secret_for(&self, connection: &Connection) -> Result<Secret> {
        let secrets = self.secrets().await?;
        let stored = secrets.get(&connection.id)?;
        let shape = connection.destination.secret_shape();

        if shape == SecretShape::None || !stored.is_none() {
            return Ok(stored);
        }

        let name = connection.name.clone();

        // What is asked for follows what the destination needs: one field for a password, two
        // for the key pairs the object stores want.
        let answer = match shape {
            SecretShape::KeyPair => self.ask(Question::KeyPair { connection: name }).await,
            _ => self.ask(Question::Password { connection: name }).await,
        };

        let secret = match (answer.text(), answer.pair()) {
            (Some(password), _) => Secret::Password(password.to_owned()),
            (_, Some((id, key))) => Secret::KeyPair {
                id: id.to_owned(),
                secret: key.to_owned(),
            },
            _ => return Err(Error::Cancelled),
        };

        secrets.set(&connection.id, &secret)?;

        Ok(secret)
    }

    async fn ask_for_passphrase(&self, connection: &Connection, path: String) -> Result<Secret> {
        let answer = self.ask(Question::KeyPassphrase { path }).await;

        let Some(passphrase) = answer.text() else {
            return Err(Error::Cancelled);
        };

        let secret = Secret::Password(passphrase.to_owned());
        self.secrets().await?.set(&connection.id, &secret)?;

        Ok(secret)
    }

    async fn disconnect(&self, id: SessionId) -> Result<()> {
        let session = self.sessions.lock().unwrap().remove(&id);

        if let Some(session) = session {
            // The connection is gone from the registry either way, but a server that would not
            // let go is worth saying out loud rather than leaving a half-closed socket behind.
            if let Err(error) = session.provider.disconnect().await {
                self.report(error.into());
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

    /// Moves files into another folder on the same connection.
    ///
    /// A rename with a different parent, which is exactly what a move is — so an object store
    /// copies and deletes, and says so beforehand through its capabilities, the same as it does
    /// for a rename in place.
    async fn move_to(
        &self,
        id: SessionId,
        from: RemotePath,
        names: Vec<String>,
        into: RemotePath,
    ) -> Result<()> {
        let session = self.session(id)?;

        if into == from {
            return Ok(());
        }

        for name in names {
            let from = from.join(&name)?;

            // Moving a folder inside itself would take the tree with it. The provider refuses
            // this too, but saying so here means the same words whichever destination it is.
            if into.starts_with(&from) {
                return Err(Error::config(format!(
                    "{name} cannot be moved inside itself"
                )));
            }

            session.provider.rename(&from, &into.join(&name)?).await?;
        }

        // Whichever folder is open now is the one that has to be redrawn — it may be the one
        // they left, the one they arrived in, or neither.
        self.refresh(id).await
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

        for source in sources {
            // A path with nothing on the end of it would upload as a file with no name, which
            // every destination reads as a folder marker or refuses outright.
            let name = source
                .file_name()
                .ok_or_else(|| Error::local_file(&source, "it has no file name"))?
                .to_string_lossy()
                .into_owned();

            let Some(name) = self.arriving_as(&session, &into, name).await? else {
                continue;
            };

            let destination = into.join(&name)?;
            let mut transfer = Transfer::new(
                Endpoint::Local(source.clone()),
                Endpoint::Remote { session: id, path: destination.clone() },
            );

            transfer.total = tokio::fs::metadata(&source)
                .await
                .ok()
                .map(|metadata| metadata.len());

            self.start(transfer, Arc::clone(&session), move |session, progress| {
                send_up(session, source, destination, progress)
            });

        }

        Ok(())
    }

    /// What an uploaded file ends up called, once whatever is already there has been taken
    /// into account.
    ///
    /// `None` means it is not to be uploaded at all. Overwriting without asking is how an
    /// afternoon's work goes missing under a file of the same name from a downloads folder.
    async fn arriving_as(
        &self,
        session: &Session,
        into: &RemotePath,
        name: String,
    ) -> Result<Option<String>> {
        if !session.provider.exists(&into.join(&name)?).await? {
            return Ok(Some(name));
        }

        match self.ask(Question::Overwrite { name: name.clone() }).await {
            Answer::Accept => Ok(Some(name)),
            Answer::KeepBoth => Ok(Some(session.provider.unused_name(into, &name).await?)),
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

            self.start(transfer, Arc::clone(&session), move |session, progress| {
                bring_down(session, source, destination, progress)
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
        session: Arc<Session>,
        work: Work,
    ) -> tokio::task::JoinHandle<Outcome>
    where
        Work: FnOnce(Arc<Session>, Progress) -> Run + Send + 'static,
        Run: std::future::Future<Output = Result<()>> + Send,
    {
        let id = transfer.id;
        let engine = Arc::clone(self);
        let slots = session.transfer_slots();
        let on = session.id;

        // A download changes nothing on the server, so only work that lands there is worth
        // looking at the folder again for.
        let lands_remotely = matches!(transfer.to, Endpoint::Remote { .. });

        self.emit(Event::TransferAdded(transfer));

        let task = tokio::spawn(async move {
            let _slot = slots.acquire().await;

            let reporter = Arc::clone(&engine);
            let progress: Progress = Arc::new(move |transferred| {
                reporter.emit(Event::TransferProgress { transfer: id, transferred });
            });

            let outcome = match work(session, progress).await {
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
                engine.report(error);
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

/// One file that has to come down, and where it goes.
struct Planned {
    source: RemotePath,
    destination: PathBuf,
    size: Option<u64>,
}

/// How a running transfer says how far it has got.
pub type Progress = Arc<dyn Fn(u64) + Send + Sync>;

async fn send_up(
    session: Arc<Session>,
    source: PathBuf,
    destination: RemotePath,
    progress: Progress,
) -> Result<()> {
    let file = tokio::fs::File::open(&source)
        .await
        .map_err(|error| Error::local_file(&source, error))?;

    let size = file.metadata().await.ok().map(|metadata| metadata.len());
    let chunks = futures::StreamExt::map(tokio_util::io::ReaderStream::new(file), |chunk| {
        chunk.map_err(|error| camion_core::Error::caused_by("could not read the file", error))
    });

    let body = counting(ByteStream::new(chunks, size), move |transferred| {
        progress(transferred)
    });

    Ok(session.provider.write(&destination, body).await?)
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
