//! The plate's items: brought back at start, then added, changed and closed
//! for agents.

use super::*;
use crate::feat::plate::{
    NewPlateItem, Plate, PlateChange, PlateItem, PlateRefusal, local_midnight,
};
use crate::ids::PlateItemId;

/// The message shown when the plate's saved items can't be read or tidied.
const PLATE_UNLOADED: &str = "couldn't load the plate";

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
    type Reply = Result<PlateItemId, String>;

    fn handle(
        &mut self,
        msg: ApiPlateAdd,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.add_plate_item(msg))
    }
}

impl Message<ApiPlateUpdate> for SessionsActor {
    type Reply = Result<(), PlateRefusal>;

    fn handle(
        &mut self,
        msg: ApiPlateUpdate,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.update_plate_item(&msg))
    }
}

impl Message<ApiPlateDone> for SessionsActor {
    type Reply = Result<(), PlateRefusal>;

    fn handle(
        &mut self,
        msg: ApiPlateDone,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        std::future::ready(self.finish_plate_item(&msg))
    }
}

impl SessionsActor {
    /// Saves `add`'s item, added now, and puts it on the plate; its new id,
    /// or why it couldn't be saved.
    fn add_plate_item(&self, add: ApiPlateAdd) -> Result<PlateItemId, String> {
        let now = SystemTime::now();
        let id = self
            .store
            .insert_plate_item(&add.item, now)
            .map_err(|report| save_failed(&self.services.log, &report))?;
        self.state.write().plate.add(add.item.into_item(id, now));
        self.report.changed();
        Ok(id)
    }

    /// Makes `update`'s change to its item now, saved and on the plate.
    fn update_plate_item(&self, update: &ApiPlateUpdate) -> Result<(), PlateRefusal> {
        let updated = {
            let app = self.state.read();
            let item = app
                .plate
                .item(update.item)
                .ok_or(PlateRefusal::NotFound(update.item))?;
            item.updated(&update.change, SystemTime::now())?
        };
        self.save_plate_item(updated)
    }

    /// Closes `done`'s item now, saved and on the plate; an item already
    /// done is left as it is.
    fn finish_plate_item(&self, done: &ApiPlateDone) -> Result<(), PlateRefusal> {
        let finished = {
            let app = self.state.read();
            app.plate
                .item(done.item)
                .ok_or(PlateRefusal::NotFound(done.item))?
                .finished(SystemTime::now())
        };
        finished.map_or(Ok(()), |item| self.save_plate_item(item))
    }

    /// Saves `item` over its old row, then puts it on the plate in place of
    /// the old one.
    fn save_plate_item(&self, item: PlateItem) -> Result<(), PlateRefusal> {
        self.store
            .save_plate_item(&item)
            .map_err(|report| PlateRefusal::Unsaved(save_failed(&self.services.log, &report)))?;
        self.state.write().plate.replace(item);
        self.report.changed();
        Ok(())
    }
}

/// The saved plate, after deleting the items closed before today's midnight
/// in `tz`, with the reason when that delete or the load failed. A failed
/// delete still loads the rest; a failed load gives an empty plate.
pub(super) fn restore_plate(store: &Store, log: &Log, tz: &TimeZone) -> (Plate, Option<String>) {
    let deleted = store
        .delete_done_before(local_midnight(SystemTime::now(), tz))
        .err()
        .map(|report| failed(log, PLATE_UNLOADED, &report));
    match store.plate_items() {
        Ok(items) => (Plate::from_saved(items), deleted),
        Err(report) => (Plate::default(), Some(failed(log, PLATE_UNLOADED, &report))),
    }
}
