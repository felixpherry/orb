//! The plate's items: what each one asks of the user, where it came from,
//! where it stands, and the links it carries.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use super::view::PlateView;
use crate::feat::harness::HarnessId;
use crate::ids::{PaneId, PlateItemId, SessionId};

/// The user's next step on an item, shown as its tag (`REVIEW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Decide,
    Review,
    Read,
    Walk,
    Pick,
    Approve,
    Answer,
    Reply,
    Merge,
    Note,
    /// An agent's suggestion; it lands parked until the user takes it up.
    Idea,
}

impl Verb {
    /// Every verb, in the order the plate's docs list them.
    pub const ALL: [Self; 11] = [
        Self::Decide,
        Self::Review,
        Self::Read,
        Self::Walk,
        Self::Pick,
        Self::Approve,
        Self::Answer,
        Self::Reply,
        Self::Merge,
        Self::Note,
        Self::Idea,
    ];

    /// The verb named by `text`, in lowercase only (`review`).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|verb| verb.name() == text)
    }

    /// The lowercase name the wire and the store use (`review`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Decide => "decide",
            Self::Review => "review",
            Self::Read => "read",
            Self::Walk => "walk",
            Self::Pick => "pick",
            Self::Approve => "approve",
            Self::Answer => "answer",
            Self::Reply => "reply",
            Self::Merge => "merge",
            Self::Note => "note",
            Self::Idea => "idea",
        }
    }
}

/// The uppercase tag the plate shows (`REVIEW`).
impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name().to_ascii_uppercase())
    }
}

/// Where an item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemState {
    /// The user's turn.
    YourMove,
    /// Waiting on someone other than the user.
    Waiting,
    /// Set aside: by the user (`by_user`), or offered that way by an agent,
    /// as an idea is.
    Parked { by_user: bool },
    /// Closed; it shows under Done today until local midnight.
    Done,
}

impl ItemState {
    /// The name the wire uses: `your_move`, `waiting`, `parked` or `done`.
    /// Both kinds of parked item are `parked`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::YourMove => "your_move",
            Self::Waiting => "waiting",
            Self::Parked { .. } => "parked",
            Self::Done => "done",
        }
    }

    /// Whether the item is still open: anything but done.
    #[must_use]
    pub fn is_open(self) -> bool {
        self != Self::Done
    }
}

/// One thing that needs the user, added by an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlateItem {
    pub id: PlateItemId,
    pub verb: Verb,
    pub state: ItemState,
    /// The user's next step.
    pub title: String,
    /// What happened, a sentence or two.
    pub what: Option<String>,
    /// URLs or file paths, in the order given.
    pub links: Vec<String>,
    pub urgent: bool,
    /// The session it was added from; it may since be gone.
    pub session: SessionId,
    /// That session's title when the item was added, shown once it's gone.
    pub session_title: String,
    /// Where ⏎ goes.
    pub pane: PaneId,
    /// The adding agent's harness; `None` when added from a shell.
    pub harness: Option<HarnessId>,
    /// The last state change; the item's age counts from here.
    pub changed_at: SystemTime,
    pub done_at: Option<SystemTime>,
    pub created_at: SystemTime,
}

impl PlateItem {
    /// The item's links, each with its label and kind.
    #[must_use]
    pub fn links(&self) -> Vec<Link> {
        self.links.iter().map(|address| Link::of(address)).collect()
    }

    /// The item with `change` made at `now`. A move to another state counts
    /// the item's age from `now`; a change that keeps the state keeps its
    /// age. Given links replace all of the old ones.
    ///
    /// # Errors
    ///
    /// [`PlateRefusal::Done`] when the item is done.
    pub fn updated(&self, change: &PlateChange, now: SystemTime) -> Result<Self, PlateRefusal> {
        if self.state == ItemState::Done {
            return Err(PlateRefusal::Done(self.id));
        }
        let state = match change.waiting {
            Some(true) => ItemState::Waiting,
            Some(false) => ItemState::YourMove,
            None => self.state,
        };
        let mut item = self.clone();
        if let Some(title) = &change.title {
            item.title.clone_from(title);
        }
        if let Some(verb) = change.verb {
            item.verb = verb;
        }
        if let Some(what) = &change.what {
            item.what = Some(what.clone());
        }
        if let Some(links) = &change.links {
            item.links.clone_from(links);
        }
        if let Some(urgent) = change.urgent {
            item.urgent = urgent;
        }
        if state != self.state {
            item.state = state;
            item.changed_at = now;
        }
        Ok(item)
    }

    /// The item closed at `now`, or `None` when it's already done.
    #[must_use]
    pub fn finished(&self, now: SystemTime) -> Option<Self> {
        (self.state != ItemState::Done).then(|| Self {
            state: ItemState::Done,
            done_at: Some(now),
            changed_at: now,
            ..self.clone()
        })
    }

    /// The item parked by the user at `now`, or back to the user's move at
    /// `now` when it's parked; `None` for a done item.
    #[must_use]
    pub fn parked_or_back(&self, now: SystemTime) -> Option<Self> {
        let state = match self.state {
            ItemState::YourMove | ItemState::Waiting => ItemState::Parked { by_user: true },
            ItemState::Parked { .. } => ItemState::YourMove,
            ItemState::Done => return None,
        };
        Some(Self {
            state,
            changed_at: now,
            ..self.clone()
        })
    }

    /// The item back in `state`, last changed at `changed_at`, with no close
    /// time: how it was before an x or a p.
    #[must_use]
    pub fn restored(&self, state: ItemState, changed_at: SystemTime) -> Self {
        Self {
            state,
            changed_at,
            done_at: None,
            ..self.clone()
        }
    }
}

/// An item an agent asks to add, before orb gives it an id and its times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlateItem {
    pub verb: Verb,
    pub state: ItemState,
    pub title: String,
    pub what: Option<String>,
    pub links: Vec<String>,
    pub urgent: bool,
    pub session: SessionId,
    pub session_title: String,
    pub pane: PaneId,
    pub harness: Option<HarnessId>,
}

impl NewPlateItem {
    /// The item saved as `id`, added and last changed at `now`.
    #[must_use]
    pub fn into_item(self, id: PlateItemId, now: SystemTime) -> PlateItem {
        PlateItem {
            id,
            verb: self.verb,
            state: self.state,
            title: self.title,
            what: self.what,
            links: self.links,
            urgent: self.urgent,
            session: self.session,
            session_title: self.session_title,
            pane: self.pane,
            harness: self.harness,
            changed_at: now,
            done_at: None,
            created_at: now,
        }
    }
}

/// An update to an item: each field given replaces the item's, each `None`
/// keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlateChange {
    pub title: Option<String>,
    pub verb: Option<Verb>,
    pub what: Option<String>,
    /// All the links, replacing the old ones.
    pub links: Option<Vec<String>>,
    pub urgent: Option<bool>,
    /// `true` moves the item to Waiting, `false` back to the user's move.
    pub waiting: Option<bool>,
}

/// Why an update or a close of an item was turned down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlateRefusal {
    /// No item has that id.
    NotFound(PlateItemId),
    /// The item is done, so it can't change.
    Done(PlateItemId),
    /// The store couldn't save it; the reason.
    Unsaved(String),
}

impl fmt::Display for PlateRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no item {}", id.0),
            Self::Done(_) => f.write_str("item is done"),
            Self::Unsaved(why) => f.write_str(why),
        }
    }
}

/// Every item on the plate, in id order, and how the user is viewing it.
/// The sessions actor writes the items; the intent handler writes the view.
#[derive(Debug, Default)]
pub struct Plate {
    items: Vec<PlateItem>,
    view: PlateView,
}

impl Plate {
    /// A plate holding the saved `items`.
    #[must_use]
    pub fn from_saved(mut items: Vec<PlateItem>) -> Self {
        items.sort_by_key(|item| item.id.0);
        Self {
            items,
            view: PlateView::default(),
        }
    }

    /// Every item, in id order.
    #[must_use]
    pub fn items(&self) -> &[PlateItem] {
        &self.items
    }

    /// How the user is viewing the plate.
    #[must_use]
    pub fn view(&self) -> &PlateView {
        &self.view
    }

    /// The view, for the intent handler to change.
    pub fn view_mut(&mut self) -> &mut PlateView {
        &mut self.view
    }

    /// The item with `id`.
    #[must_use]
    pub fn item(&self, id: PlateItemId) -> Option<&PlateItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Adds a newly saved item, whose id is above every other.
    pub fn add(&mut self, item: PlateItem) {
        self.items.push(item);
    }

    /// Puts `item` in place of the item with its id; does nothing when
    /// there's none.
    pub fn replace(&mut self, item: PlateItem) {
        if let Some(slot) = self.items.iter_mut().find(|slot| slot.id == item.id) {
            *slot = item;
        }
    }
}

/// A link on an item, with the short label and the kind the plate shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The URL or path, as given.
    pub address: String,
    pub label: String,
    pub kind: LinkKind,
}

/// What a link points at, which picks its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// A Jira issue; labelled with its key.
    Jira,
    /// A GitHub pull request; labelled `#<n>`.
    PullRequest,
    /// A Sentry issue; labelled with its id.
    Sentry,
    /// A Grafana page; labelled with the path's last segment.
    Grafana,
    /// A path with no scheme; labelled with the file name.
    File,
    /// Any other URL; labelled with its host.
    Web,
}

impl Link {
    /// The label and kind read off `address`: a URL has a scheme (`://`),
    /// anything else is a file path.
    #[must_use]
    pub fn of(address: &str) -> Self {
        let (kind, label) = match address.split_once("://") {
            Some((_, rest)) => web_label(rest),
            None => (LinkKind::File, last_part(address).to_owned()),
        };
        Self {
            address: address.to_owned(),
            label,
            kind,
        }
    }
}

/// The kind and label of a URL, from what follows its scheme.
fn web_label(rest: &str) -> (LinkKind, String) {
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.split('/');
    let host = parts.next().unwrap_or_default();
    let path: Vec<&str> = parts.filter(|part| !part.is_empty()).collect();
    let after = |name: &str| {
        path.iter()
            .position(|&part| part == name)
            .and_then(|at| path.get(at + 1))
            .copied()
    };
    let lower = host.to_ascii_lowercase();
    if let Some(key) = after("browse") {
        (LinkKind::Jira, key.to_owned())
    } else if lower.contains("github")
        && let Some(number) = after("pull")
    {
        (LinkKind::PullRequest, format!("#{number}"))
    } else if lower.contains("sentry")
        && let Some(issue) = after("issues")
    {
        (LinkKind::Sentry, issue.to_owned())
    } else if lower.contains("grafana") {
        let last = path.last().copied().unwrap_or(host);
        (LinkKind::Grafana, last.to_owned())
    } else {
        (LinkKind::Web, host.to_owned())
    }
}

/// The last non-empty `/` part of a path: the file name.
fn last_part(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// The start of `now`'s day in `tz`: the local midnight "today" counts from.
/// A time the calendar can't hold gives the epoch, so nothing counts as
/// before today.
#[must_use]
pub fn local_midnight(now: SystemTime, tz: &TimeZone) -> SystemTime {
    Timestamp::try_from(now)
        .ok()
        .and_then(|at| at.to_zoned(tz.clone()).start_of_day().ok())
        .map_or(UNIX_EPOCH, |day| SystemTime::from(day.timestamp()))
}

/// Item `id` in `state`: a review in session 1, pane 1, added by claude,
/// changed and added at 10 s past the epoch, and done at 20 s when done.
#[cfg(test)]
pub(crate) fn test_item(id: i64, state: ItemState) -> PlateItem {
    let at = |secs| UNIX_EPOCH + std::time::Duration::from_secs(secs);
    PlateItem {
        id: PlateItemId(id),
        verb: Verb::Review,
        state,
        title: format!("item {id}"),
        what: None,
        links: Vec::new(),
        urgent: false,
        session: SessionId(1),
        session_title: "session 1".to_owned(),
        pane: PaneId(1),
        harness: Some(HarnessId::new("claude")),
        changed_at: at(10),
        done_at: (state == ItemState::Done).then(|| at(20)),
        created_at: at(10),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate parse failures with `?` and assert on the outcome"
)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[rstest::rstest]
    #[case("decide", Verb::Decide)]
    #[case("review", Verb::Review)]
    #[case("read", Verb::Read)]
    #[case("walk", Verb::Walk)]
    #[case("pick", Verb::Pick)]
    #[case("approve", Verb::Approve)]
    #[case("answer", Verb::Answer)]
    #[case("reply", Verb::Reply)]
    #[case("merge", Verb::Merge)]
    #[case("note", Verb::Note)]
    #[case("idea", Verb::Idea)]
    fn verb_parses_from_its_lowercase_name(#[case] text: &str, #[case] verb: Verb) {
        // Given / When / Then: the lowercase name gives its verb.
        assert_eq!(Verb::parse(text), Some(verb), "{text} names a verb");
    }

    #[rstest::rstest]
    #[case("REVIEW")]
    #[case("Review")]
    #[case("fix")]
    #[case("")]
    fn verb_refuses_text_that_is_no_lowercase_name(#[case] text: &str) {
        // Given / When / Then: only a lowercase verb name parses.
        assert_eq!(Verb::parse(text), None, "{text:?} names no verb");
    }

    #[rstest::rstest]
    #[case(Verb::Decide, "DECIDE")]
    #[case(Verb::Review, "REVIEW")]
    #[case(Verb::Read, "READ")]
    #[case(Verb::Walk, "WALK")]
    #[case(Verb::Pick, "PICK")]
    #[case(Verb::Approve, "APPROVE")]
    #[case(Verb::Answer, "ANSWER")]
    #[case(Verb::Reply, "REPLY")]
    #[case(Verb::Merge, "MERGE")]
    #[case(Verb::Note, "NOTE")]
    #[case(Verb::Idea, "IDEA")]
    fn verb_displays_its_uppercase_tag(#[case] verb: Verb, #[case] tag: &str) {
        // Given / When / Then: the plate shows the verb's uppercase tag.
        assert_eq!(verb.to_string(), tag, "the tag of {verb:?}");
    }

    #[rstest::rstest]
    fn verb_travels_lowercase_on_the_wire() -> serde_json::Result<()> {
        // Given a review.
        let verb = Verb::Review;

        // When writing it for the wire.
        let wire = serde_json::to_string(&verb)?;

        // Then it's the lowercase name.
        assert_eq!(wire, "\"review\"", "verbs travel lowercase");
        Ok(())
    }

    #[rstest::rstest]
    #[case(ItemState::YourMove, "your_move")]
    #[case(ItemState::Waiting, "waiting")]
    #[case(ItemState::Parked { by_user: true }, "parked")]
    #[case(ItemState::Parked { by_user: false }, "parked")]
    #[case(ItemState::Done, "done")]
    fn item_state_has_its_wire_name(#[case] state: ItemState, #[case] name: &str) {
        // Given / When / Then: each state travels under its wire name.
        assert_eq!(state.name(), name, "the wire name of {state:?}");
    }

    #[rstest::rstest]
    #[case(ItemState::YourMove, true)]
    #[case(ItemState::Waiting, true)]
    #[case(ItemState::Parked { by_user: true }, true)]
    #[case(ItemState::Parked { by_user: false }, true)]
    #[case(ItemState::Done, false)]
    fn only_a_done_item_is_closed(#[case] state: ItemState, #[case] open: bool) {
        // Given / When / Then: every state but Done is open.
        assert_eq!(state.is_open(), open, "whether {state:?} is open");
    }

    #[rstest::rstest]
    #[case("https://acme.atlassian.net/browse/GT-123", "GT-123", LinkKind::Jira)]
    #[case(
        "https://github.com/acme/orb/pull/42/files",
        "#42",
        LinkKind::PullRequest
    )]
    #[case(
        "https://acme.sentry.io/issues/987654/?project=1",
        "987654",
        LinkKind::Sentry
    )]
    #[case(
        "https://grafana.acme.com/d/abc/checkout-latency?orgId=1",
        "checkout-latency",
        LinkKind::Grafana
    )]
    #[case("https://grafana.acme.com", "grafana.acme.com", LinkKind::Grafana)]
    #[case(".plans/plate-board/plan.md", "plan.md", LinkKind::File)]
    #[case("/tmp/report.html", "report.html", LinkKind::File)]
    #[case("https://docs.rs/jiff/latest", "docs.rs", LinkKind::Web)]
    #[case("https://github.com/acme/orb", "github.com", LinkKind::Web)]
    fn link_label_and_kind_come_from_its_address(
        #[case] address: &str,
        #[case] label: &str,
        #[case] kind: LinkKind,
    ) {
        // Given / When / Then: the label and kind are read off the address.
        let link = Link::of(address);
        assert_eq!(
            (link.label.as_str(), link.kind),
            (label, kind),
            "the label and kind of {address}"
        );
    }

    #[rstest::rstest]
    fn local_midnight_is_the_start_of_the_day() -> Result<(), jiff::Error> {
        // Given an afternoon in UTC.
        let now = SystemTime::from("2026-10-17T15:04:05Z".parse::<Timestamp>()?);

        // When finding its midnight.
        let midnight = local_midnight(now, &TimeZone::UTC);

        // Then it's the start of that day.
        let expected = SystemTime::from("2026-10-17T00:00:00Z".parse::<Timestamp>()?);
        assert_eq!(midnight, expected, "today starts at local midnight");
        Ok(())
    }

    #[rstest::rstest]
    fn update_that_moves_the_state_counts_the_age_from_now() {
        // Given an item that's the user's move.
        let item = test_item(1, ItemState::YourMove);
        let change = PlateChange {
            waiting: Some(true),
            ..PlateChange::default()
        };

        // When moving it to Waiting.
        let updated = item.updated(&change, at(50));

        // Then it waits, changed now.
        assert_eq!(
            updated.map(|item| (item.state, item.changed_at)),
            Ok((ItemState::Waiting, at(50))),
            "a state move resets the item's age"
        );
    }

    #[rstest::rstest]
    fn update_that_keeps_the_state_keeps_the_age() {
        // Given an item that's the user's move.
        let item = test_item(1, ItemState::YourMove);
        let change = PlateChange {
            title: Some("look again".to_owned()),
            waiting: Some(false),
            ..PlateChange::default()
        };

        // When retitling it, still the user's move.
        let updated = item.updated(&change, at(50));

        // Then the title changed and the age didn't.
        assert_eq!(
            updated.map(|item| (item.title, item.changed_at)),
            Ok(("look again".to_owned(), at(10))),
            "a change without a state move keeps the item's age"
        );
    }

    #[rstest::rstest]
    fn update_with_links_replaces_every_link() {
        // Given an item with two links.
        let item = PlateItem {
            links: vec!["a".to_owned(), "b".to_owned()],
            ..test_item(1, ItemState::YourMove)
        };
        let change = PlateChange {
            links: Some(vec!["c".to_owned()]),
            ..PlateChange::default()
        };

        // When updating it with one link.
        let updated = item.updated(&change, at(50));

        // Then that's its only link.
        assert_eq!(
            updated.map(|item| item.links),
            Ok(vec!["c".to_owned()]),
            "given links replace the old ones"
        );
    }

    #[rstest::rstest]
    fn update_of_a_done_item_is_refused() {
        // Given a done item.
        let item = test_item(1, ItemState::Done);

        // When updating it.
        let updated = item.updated(&PlateChange::default(), at(50));

        // Then it's refused.
        assert_eq!(
            updated,
            Err(PlateRefusal::Done(PlateItemId(1))),
            "a done item can't change"
        );
    }

    #[rstest::rstest]
    fn finishing_an_item_stamps_when_it_was_done() {
        // Given a waiting item.
        let item = test_item(1, ItemState::Waiting);

        // When finishing it.
        let finished = item.finished(at(50));

        // Then it's done now.
        assert_eq!(
            finished.map(|item| (item.state, item.done_at, item.changed_at)),
            Some((ItemState::Done, Some(at(50)), at(50))),
            "finishing stamps done_at and the age"
        );
    }

    #[rstest::rstest]
    fn finishing_a_done_item_gives_nothing() {
        // Given a done item.
        let item = test_item(1, ItemState::Done);

        // When finishing it again.
        let finished = item.finished(at(50));

        // Then there's nothing to change.
        assert_eq!(finished, None, "a done item stays as it was");
    }

    #[rstest::rstest]
    #[case::your_move(ItemState::YourMove)]
    #[case::waiting(ItemState::Waiting)]
    fn parked_or_back_parks_your_move_and_waiting_items(#[case] state: ItemState) {
        // Given an open item that isn't parked.
        let item = test_item(1, state);

        // When the user parks it.
        let parked = item.parked_or_back(at(50));

        // Then it's parked by the user, its age counting from now.
        assert_eq!(
            parked.map(|item| (item.state, item.changed_at)),
            Some((ItemState::Parked { by_user: true }, at(50))),
            "p should park the item now"
        );
    }

    #[rstest::rstest]
    #[case::by_the_user(true)]
    #[case::offered(false)]
    fn parked_or_back_brings_a_parked_item_back(#[case] by_user: bool) {
        // Given a parked item.
        let item = test_item(1, ItemState::Parked { by_user });

        // When the user presses p on it.
        let back = item.parked_or_back(at(50));

        // Then it's the user's move again, its age counting from now.
        assert_eq!(
            back.map(|item| (item.state, item.changed_at)),
            Some((ItemState::YourMove, at(50))),
            "p on a parked item should bring it back"
        );
    }

    #[rstest::rstest]
    fn parked_or_back_leaves_a_done_item() {
        // Given a done item.
        let item = test_item(1, ItemState::Done);

        // When the user presses p on it.
        let parked = item.parked_or_back(at(50));

        // Then there's nothing to change.
        assert_eq!(parked, None, "a done item can't be parked");
    }

    #[rstest::rstest]
    fn restored_clears_the_close_time() {
        // Given a done item, done at 20 s.
        let item = test_item(1, ItemState::Done);

        // When restoring it to the user's move as of 10 s.
        let restored = item.restored(ItemState::YourMove, at(10));

        // Then it's open again with no close time.
        assert_eq!(
            (restored.state, restored.changed_at, restored.done_at),
            (ItemState::YourMove, at(10), None),
            "restoring should bring back the state and age and drop done_at"
        );
    }

    #[rstest::rstest]
    fn new_item_is_added_and_changed_when_saved() {
        // Given an item an agent asks to add.
        let item = test_item(1, ItemState::YourMove);
        let new = NewPlateItem {
            verb: item.verb,
            state: item.state,
            title: item.title.clone(),
            what: item.what.clone(),
            links: item.links.clone(),
            urgent: item.urgent,
            session: item.session,
            session_title: item.session_title.clone(),
            pane: item.pane,
            harness: item.harness.clone(),
        };

        // When saving it as item 7 at 50 s.
        let saved = new.into_item(PlateItemId(7), at(50));

        // Then it has that id and both times, and isn't done.
        assert_eq!(
            saved,
            PlateItem {
                id: PlateItemId(7),
                changed_at: at(50),
                created_at: at(50),
                done_at: None,
                ..item
            },
            "a saved item gets its id and times"
        );
    }

    #[rstest::rstest]
    fn saved_items_are_kept_in_id_order() {
        // Given items saved out of order.
        let items = vec![
            test_item(3, ItemState::YourMove),
            test_item(1, ItemState::Done),
            test_item(2, ItemState::Waiting),
        ];

        // When making the plate.
        let plate = Plate::from_saved(items);

        // Then they're in id order.
        let ids: Vec<i64> = plate.items().iter().map(|item| item.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3], "the plate keeps items in id order");
    }

    #[rstest::rstest]
    fn replacing_an_item_swaps_the_one_with_its_id() {
        // Given a plate of two items.
        let mut plate = Plate::from_saved(vec![
            test_item(1, ItemState::YourMove),
            test_item(2, ItemState::YourMove),
        ]);

        // When replacing item 2 with its done self.
        plate.replace(test_item(2, ItemState::Done));

        // Then item 2 is done.
        assert_eq!(
            plate.item(PlateItemId(2)).map(|item| item.state),
            Some(ItemState::Done),
            "the replaced item is the new one"
        );
    }
}
