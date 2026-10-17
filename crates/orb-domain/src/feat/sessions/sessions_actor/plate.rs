//! The plate's items: brought back at start, then added, changed and closed
//! for agents, and saved after the user's x, p and u.

use error_stack::ResultExt;
use wherror::Error;

use super::*;
use crate::feat::plate::state::no_item;
use crate::feat::plate::{NewPlateItem, Plate, PlateChange, PlateItem};
use crate::ids::PlateItemId;

/// The message shown when the plate's saved items can't be read or tidied.
const PLATE_UNLOADED: &str = "couldn't load the plate";

/// Why an agent's add, change or close of a plate item failed. Every report
/// carries a one-line reason as its latest `String` attachment, the API's
/// message.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error(debug)]
pub enum PlateApiError {
    /// No item has that id.
    NotFound,
    /// The item is done, so it can't change.
    Done,
    /// The store couldn't save it.
    Unsaved,
}

/// An agent's new plate item. Only ever `ask`ed: an `Err` reply to a `tell`
/// would stop the actor.
#[derive(Debug)]
pub struct ApiPlateAdd {
    pub item: NewPlateItem,
}

/// An agent's change to plate item `item`. Only ever `ask`ed: an `Err` reply
/// to a `tell` would stop the actor.
#[derive(Debug)]
pub struct ApiPlateUpdate {
    pub item: PlateItemId,
    pub change: PlateChange,
}

/// An agent closing plate item `item`. Only ever `ask`ed: an `Err` reply to a
/// `tell` would stop the actor.
#[derive(Debug)]
pub struct ApiPlateDone {
    pub item: PlateItemId,
}

impl Message<ApiPlateAdd> for SessionsActor {
    type Reply = Result<PlateItemId, Report<PlateApiError>>;

    fn handle(
        &mut self,
        msg: ApiPlateAdd,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.add_plate_item(msg))
    }
}

impl Message<ApiPlateUpdate> for SessionsActor {
    type Reply = Result<(), Report<PlateApiError>>;

    fn handle(
        &mut self,
        msg: ApiPlateUpdate,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.update_plate_item(&msg))
    }
}

impl Message<ApiPlateDone> for SessionsActor {
    type Reply = Result<(), Report<PlateApiError>>;

    fn handle(
        &mut self,
        msg: ApiPlateDone,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.finish_plate_item(&msg))
    }
}

impl SessionsActor {
    /// Saves `add`'s item, added now, and puts it on the plate; its new id.
    ///
    /// # Errors
    ///
    /// [`PlateApiError::Unsaved`] when the store can't save it.
    fn add_plate_item(&self, add: ApiPlateAdd) -> Result<PlateItemId, Report<PlateApiError>> {
        let now = SystemTime::now();
        let id = self
            .store
            .insert_plate_item(&add.item, now)
            .map_err(|report| self.unsaved(&report))?;
        self.state.write().plate.add(add.item.into_item(id, now));
        self.report.changed();
        Ok(id)
    }

    /// Makes `update`'s change to its item now, saved and on the plate.
    ///
    /// # Errors
    ///
    /// [`PlateApiError::NotFound`] when there's no such item,
    /// [`PlateApiError::Done`] when it's done, the user's x landing while the
    /// change was saving included, and [`PlateApiError::Unsaved`] when the
    /// store can't save it.
    fn update_plate_item(&self, update: &ApiPlateUpdate) -> Result<(), Report<PlateApiError>> {
        let now = SystemTime::now();
        self.change_plate_item(update.item, |item| {
            item.updated(&update.change, now)
                .map(Some)
                .change_context(PlateApiError::Done)
        })
    }

    /// Closes `done`'s item now, saved and on the plate; an item already
    /// done is left as it is.
    ///
    /// # Errors
    ///
    /// [`PlateApiError::NotFound`] when there's no such item, and
    /// [`PlateApiError::Unsaved`] when the store can't save it.
    fn finish_plate_item(&self, done: &ApiPlateDone) -> Result<(), Report<PlateApiError>> {
        let now = SystemTime::now();
        self.change_plate_item(done.item, |item| Ok(item.finished(now)))
    }

    /// Makes an agent's `change` to item `id`: saves the item `change` makes
    /// of it, then puts that on the plate. `change` refuses with an error,
    /// or leaves the item as it is with `None`. When the user's x, p or u
    /// lands while the save runs, `change` is made again to the item as the
    /// plate has it now, so the store and the plate both keep what the user
    /// did; and when that's refused or changes nothing, the user's item is
    /// saved back over the agent's, so a restart keeps it too.
    ///
    /// # Errors
    ///
    /// [`PlateApiError::NotFound`] when there's no such item, `change`'s
    /// refusal, and [`PlateApiError::Unsaved`] when the store can't save
    /// it; the plate is left as it is.
    fn change_plate_item(
        &self,
        id: PlateItemId,
        change: impl Fn(&PlateItem) -> Result<Option<PlateItem>, Report<PlateApiError>>,
    ) -> Result<(), Report<PlateApiError>> {
        let mut saved_over_user = false;
        loop {
            let read = self
                .state
                .read()
                .plate
                .item(id)
                .cloned()
                .ok_or_else(|| not_found(id))?;
            let changed = match change(&read) {
                Ok(Some(changed)) => changed,
                kept => {
                    if saved_over_user {
                        self.store_plate_item(id);
                    }
                    return kept.map(drop);
                }
            };
            self.store
                .save_plate_item(&changed)
                .map_err(|report| self.unsaved(&report))?;
            let taken = {
                let mut app = self.state.write();
                let unchanged = app.plate.item(id) == Some(&read);
                if unchanged {
                    app.plate.replace(changed);
                }
                unchanged
            };
            if taken {
                self.report.changed();
                return Ok(());
            }
            saved_over_user = true;
        }
    }

    /// Logs the store's failure `report` and gives the agent's
    /// [`PlateApiError::Unsaved`] with the message box's words.
    fn unsaved(&self, report: &Report<StoreError>) -> Report<PlateApiError> {
        Report::new(PlateApiError::Unsaved).attach(save_failed(&self.services.log, report))
    }

    /// Saves item `id` as the plate has it, after the user's x, p or u; a
    /// failure shows in the message box. An item no longer there is left
    /// alone.
    pub(super) fn store_plate_item(&self, id: PlateItemId) {
        let Some(item) = self.state.read().plate.item(id).cloned() else {
            return;
        };
        if let Err(report) = self.store.save_plate_item(&item) {
            let error = save_failed(&self.services.log, &report);
            self.report.report(Outcome::Failed(error));
        }
    }
}

/// The saved plate, after deleting the items closed before `today` (local
/// midnight), with the reason when that delete or the load failed. A failed
/// delete still loads the rest; a failed load gives an empty plate.
pub(super) fn restore_plate(
    store: &Store,
    log: &Log,
    today: SystemTime,
) -> (Plate, Option<String>) {
    let deleted = store
        .delete_done_before(today)
        .err()
        .map(|report| failed(log, PLATE_UNLOADED, &report));
    match store.plate_items() {
        Ok(items) => (Plate::from_saved(items), deleted),
        Err(report) => (Plate::default(), Some(failed(log, PLATE_UNLOADED, &report))),
    }
}

/// The refusal for a change to item `id`, which isn't on the plate.
fn not_found(id: PlateItemId) -> Report<PlateApiError> {
    Report::new(PlateApiError::NotFound).attach(no_item(id))
}
