use std::collections::HashMap;
use std::pin::Pin;

use camion_engine::event::Outcome;
use camion_engine::transfer::{Direction, State, Transfer, TransferId};
use camion_engine::Event;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QVariant,
};

use crate::format;

/// The transfer queue, as the toolbar button and its window show it.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);

        type QAbstractListModel = crate::qt::qobject::QAbstractListModel;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        /// How many transfers are still going, which is what the toolbar badge shows.
        #[qproperty(i32, active)]
        #[qproperty(i32, count)]
        type Transfers = super::TransfersRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &Transfers, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &Transfers, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &Transfers) -> QHash_i32_QByteArray;

        /// Drops everything that has already finished, so the window shows what is happening
        /// rather than what happened.
        #[qinvokable]
        fn clear_finished(self: Pin<&mut Transfers>);

        #[qinvokable]
        fn id_at(self: &Transfers, row: i32) -> i64;
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut Transfers>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut Transfers>);

        #[inherit]
        #[cxx_name = "dataChanged"]
        unsafe fn data_changed(
            self: Pin<&mut Transfers>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[inherit]
        fn index(self: &Transfers, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
    }

    impl cxx_qt::Threading for Transfers {}

    impl cxx_qt::Initialize for Transfers {}
}

const USER_ROLE: i32 = 0x0100;

const NAME: i32 = USER_ROLE;
const STATUS: i32 = USER_ROLE + 1;
const FRACTION: i32 = USER_ROLE + 2;
const DIRECTION: i32 = USER_ROLE + 3;
const RUNNING: i32 = USER_ROLE + 4;

#[derive(Default)]
pub struct TransfersRust {
    active: i32,
    count: i32,
    queue: Vec<Transfer>,
    rows: HashMap<TransferId, usize>,
}

impl cxx_qt::Initialize for qobject::Transfers {
    fn initialize(self: Pin<&mut Self>) {
        let thread = self.qt_thread();

        crate::bus::listen(move |event| match event.as_ref() {
            Event::TransferAdded(transfer) => {
                let transfer = transfer.clone();

                let _ = thread.queue(move |model| model.add(transfer));
            }
            Event::TransferProgress { transfer, transferred } => {
                let (id, transferred) = (*transfer, *transferred);

                let _ = thread.queue(move |model| model.advance(id, transferred));
            }
            Event::TransferFinished { transfer, outcome } => {
                let (id, outcome) = (*transfer, outcome.clone());

                let _ = thread.queue(move |model| model.finish(id, outcome));
            }
            _ => {}
        });
    }
}

impl qobject::Transfers {
    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(transfer) = self.rust().queue.get(index.row() as usize) else {
            return QVariant::default();
        };

        match role {
            NAME => QVariant::from(&QString::from(&transfer.name)),
            STATUS => QVariant::from(&QString::from(&status(transfer))),
            FRACTION => QVariant::from(&transfer.fraction().unwrap_or(0.0)),
            DIRECTION => QVariant::from(&QString::from(match transfer.direction {
                Direction::Upload => "upload",
                Direction::Download => "download",
                Direction::Between => "between",
            })),
            RUNNING => QVariant::from(&!transfer.state.is_finished()),
            _ => QVariant::default(),
        }
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.rust().queue.len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut names = QHash::<QHashPair_i32_QByteArray>::default();

        names.insert(NAME, QByteArray::from("name"));
        names.insert(STATUS, QByteArray::from("status"));
        names.insert(FRACTION, QByteArray::from("fraction"));
        names.insert(DIRECTION, QByteArray::from("direction"));
        names.insert(RUNNING, QByteArray::from("running"));

        names
    }

    pub fn clear_finished(mut self: Pin<&mut Self>) {
        unsafe { self.as_mut().begin_reset_model() };
        self.as_mut()
            .rust_mut()
            .queue
            .retain(|transfer| !transfer.state.is_finished());
        self.as_mut().reindex();
        unsafe { self.as_mut().end_reset_model() };

        self.recount();
    }

    pub fn id_at(&self, row: i32) -> i64 {
        self.rust()
            .queue
            .get(row.max(0) as usize)
            .map(|transfer| transfer.id.0 as i64)
            .unwrap_or(-1)
    }

    fn add(mut self: Pin<&mut Self>, transfer: Transfer) {
        unsafe { self.as_mut().begin_reset_model() };
        self.as_mut().rust_mut().queue.push(transfer);
        self.as_mut().reindex();
        unsafe { self.as_mut().end_reset_model() };

        self.recount();
    }

    /// Progress is the one thing that changes often, so it updates the single row it belongs to
    /// rather than resetting the model and losing the user's scroll position.
    fn advance(mut self: Pin<&mut Self>, id: TransferId, transferred: u64) {
        let Some(row) = self.rust().rows.get(&id).copied() else {
            return;
        };

        if let Some(transfer) = self.as_mut().rust_mut().queue.get_mut(row) {
            transfer.transferred = transferred;
            transfer.state = State::Running;
        }

        self.touch(row as i32);
    }

    fn finish(mut self: Pin<&mut Self>, id: TransferId, outcome: Outcome) {
        let Some(row) = self.rust().rows.get(&id).copied() else {
            return;
        };

        if let Some(transfer) = self.as_mut().rust_mut().queue.get_mut(row) {
            transfer.state = match outcome {
                Outcome::Done => State::Done,
                Outcome::Failed(reason) => State::Failed(reason),
                Outcome::Cancelled => State::Cancelled,
            };
        }

        self.as_mut().touch(row as i32);
        self.recount();
    }

    fn touch(mut self: Pin<&mut Self>, row: i32) {
        let at = self.index(row, 0, &QModelIndex::default());

        unsafe { self.as_mut().data_changed(&at, &at, &QList::<i32>::default()) };
    }

    fn reindex(mut self: Pin<&mut Self>) {
        let rows = self
            .rust()
            .queue
            .iter()
            .enumerate()
            .map(|(row, transfer)| (transfer.id, row))
            .collect();

        self.as_mut().rust_mut().rows = rows;
    }

    fn recount(mut self: Pin<&mut Self>) {
        let active = self
            .rust()
            .queue
            .iter()
            .filter(|transfer| !transfer.state.is_finished())
            .count() as i32;
        let count = self.rust().queue.len() as i32;

        self.as_mut().set_active(active);
        self.as_mut().set_count(count);
    }
}

fn status(transfer: &Transfer) -> String {
    match &transfer.state {
        State::Queued => "Waiting".to_owned(),
        State::Running => match transfer.total {
            Some(total) => format!(
                "{} of {}",
                format::size(transfer.transferred),
                format::size(total)
            ),
            None => format::size(transfer.transferred),
        },
        State::Done => format::size(transfer.transferred),
        State::Failed(reason) => reason.clone(),
        State::Cancelled => "Cancelled".to_owned(),
    }
}
