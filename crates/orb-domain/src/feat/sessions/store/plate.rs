//! Saving and loading the plate's items.

use std::time::SystemTime;

use super::*;
use crate::feat::plate::{ItemState, NewPlateItem, PlateItem, Verb};
use crate::feat::sessions::sessions_actor::{from_ms, to_ms};
use crate::ids::PlateItemId;

impl Store {
    /// Every saved plate item, oldest id first. A row whose verb or state
    /// orb doesn't know, or whose links aren't a list of text, is skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn plate_items(&self) -> Result<Vec<PlateItem>, Report<StoreError>> {
        let rows = self
            .query(
                "SELECT id, verb, state, title, what, links, urgent, session_id, session_title,
                        pane_id, harness, changed_at, done_at, created_at
                 FROM plate_items ORDER BY id",
                plate_row,
            )
            .attach("failed to read the plate")?;
        Ok(rows.into_iter().flatten().collect())
    }

    /// Saves a new plate item, added and last changed `at`, and gives its id.
    /// Ids only grow: one is never given again after its item is deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn insert_plate_item(
        &self,
        item: &NewPlateItem,
        at: SystemTime,
    ) -> Result<PlateItemId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO plate_items
                   (verb, state, title, what, links, urgent, session_id, session_title, pane_id,
                    harness, changed_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11) RETURNING id",
                params![
                    item.verb.name(),
                    state_text(item.state),
                    item.title,
                    item.what,
                    links_text(&item.links)?,
                    item.urgent,
                    item.session.0,
                    item.session_title,
                    item.pane.0,
                    item.harness.as_ref().map(HarnessId::as_str),
                    to_ms(at),
                ],
                |row| row.get(0),
            )
            .map(PlateItemId)
            .change_context(StoreError)
            .attach("failed to save a plate item")
    }

    /// Saves `item` over its row: its verb, state, title, description,
    /// links, urgency, last move and close. Marking an item done is this save
    /// with the item done.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn save_plate_item(&self, item: &PlateItem) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "UPDATE plate_items
                    SET verb = ?2, state = ?3, title = ?4, what = ?5, links = ?6, urgent = ?7,
                        changed_at = ?8, done_at = ?9
                  WHERE id = ?1",
                params![
                    item.id.0,
                    item.verb.name(),
                    state_text(item.state),
                    item.title,
                    item.what,
                    links_text(&item.links)?,
                    item.urgent,
                    to_ms(item.changed_at),
                    item.done_at.map(to_ms),
                ],
            )
            .change_context(StoreError)
            .attach_with(|| format!("failed to save plate item {}", item.id.0))?;
        Ok(())
    }

    /// Deletes every done item closed before `before`; open items and items
    /// closed since are kept.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn delete_done_before(&self, before: SystemTime) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "DELETE FROM plate_items WHERE state = 'done' AND done_at < ?1",
                [to_ms(before)],
            )
            .change_context(StoreError)
            .attach("failed to delete old done plate items")?;
        Ok(())
    }
}

/// How an item's state is saved: a parked item is `offered` when an agent
/// set it aside and `parked` when the user did.
fn state_text(state: ItemState) -> &'static str {
    match state {
        ItemState::YourMove => "your_move",
        ItemState::Waiting => "waiting",
        ItemState::Parked { by_user: false } => "offered",
        ItemState::Parked { by_user: true } => "parked",
        ItemState::Done => "done",
    }
}

/// The state saved as `text`; `None` for text orb doesn't know.
fn item_state(text: &str) -> Option<ItemState> {
    match text {
        "your_move" => Some(ItemState::YourMove),
        "waiting" => Some(ItemState::Waiting),
        "offered" => Some(ItemState::Parked { by_user: false }),
        "parked" => Some(ItemState::Parked { by_user: true }),
        "done" => Some(ItemState::Done),
        _ => None,
    }
}

/// Links as they're saved: a JSON list of text, in order.
fn links_text(links: &[String]) -> Result<String, Report<StoreError>> {
    serde_json::to_string(links)
        .change_context(StoreError)
        .attach("failed to write a plate item's links")
}

/// The plate item in `row`; `None` when its verb, state or links don't read.
fn plate_row(row: &Row<'_>) -> rusqlite::Result<Option<PlateItem>> {
    let (Some(verb), Some(state), Ok(links)) = (
        Verb::parse(&row.get::<_, String>(1)?),
        item_state(&row.get::<_, String>(2)?),
        serde_json::from_str::<Vec<String>>(&row.get::<_, String>(5)?),
    ) else {
        return Ok(None);
    };
    Ok(Some(PlateItem {
        id: PlateItemId(row.get(0)?),
        verb,
        state,
        title: row.get(3)?,
        what: row.get(4)?,
        links,
        urgent: row.get(6)?,
        session: SessionId(row.get(7)?),
        session_title: row.get(8)?,
        pane: PaneId(row.get(9)?),
        harness: row.get::<_, Option<String>>(10)?.map(HarnessId::new),
        changed_at: from_ms(row.get(11)?),
        done_at: row.get::<_, Option<i64>>(12)?.map(from_ms),
        created_at: from_ms(row.get(13)?),
    }))
}
