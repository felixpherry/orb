//! Opening a plate item's links: gx opens the selected item's first link and
//! a digit opens that link. A web link opens in its session's one
//! terminal-browser tab: the first one opens that tab, later ones open as
//! new browser tabs inside it while its pane is still in the session's
//! layout, and either way the tab is shown with the keys in it. Any other
//! file opens in the editor in a new tab. With no session to open in, or no
//! terminal-browser for a web link, the desktop's opener takes it and the
//! plate stays open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::board::{Board, RowKey};
use super::intent::{enter_pane, give_back};
use super::state::{Link, LinkKind, PlateItem};
use crate::command::SessionsCommand;
use crate::feat::control::agent::quoted;
use crate::feat::layout::state::Layouts;
use crate::feat::picker::intent::pick_session;
use crate::ids::{PaneId, SessionId};
use crate::{AppState, Command};

/// Each session's one terminal-browser tab that plate links open in, kept
/// while orb runs. Whether terminal-browser is there is set once at start-up;
/// the sessions actor, the owner, writes each tab it opens.
#[derive(Debug, Default)]
pub struct BrowserTabs {
    /// Whether terminal-browser is installed.
    available: bool,
    /// The pane of each session's browser tab.
    panes: HashMap<SessionId, PaneId>,
}

impl BrowserTabs {
    /// No browser tabs yet, with terminal-browser `available` or not.
    pub fn new(available: bool) -> Self {
        Self {
            available,
            panes: HashMap::new(),
        }
    }

    /// Whether terminal-browser is installed, so web links can open in orb.
    pub fn available(&self) -> bool {
        self.available
    }

    /// `session`'s browser tab's pane, while that pane is still in its
    /// layout. A closed pane is in no layout, so it's never handed out again.
    pub fn pane(&self, session: SessionId, layouts: &Layouts) -> Option<PaneId> {
        self.panes
            .get(&session)
            .copied()
            .filter(|&pane| layouts.owner_of(pane) == Some(session))
    }

    /// Makes pane `pane` `session`'s browser tab, in place of any earlier one.
    pub fn remember(&mut self, session: SessionId, pane: PaneId) {
        self.panes.insert(session, pane);
    }
}

/// Where a plate link opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkRoute {
    /// A new tab of `session` running terminal-browser on `url`, which
    /// becomes its browser tab.
    NewBrowserTab { session: SessionId, url: String },
    /// A new browser tab in `session`'s browser tab, pane `pane`, which is
    /// then shown.
    BrowserTab {
        session: SessionId,
        pane: PaneId,
        url: String,
    },
    /// A new tab of `session` running `$EDITOR`, else `less`, on `path`.
    Editor { session: SessionId, path: PathBuf },
    /// The desktop's own opener, on a URL or an absolute path.
    Outside(String),
}

/// Where link `n` (from 1) of `item` opens; `None` when it has no link `n`.
///
/// It opens in the item's session while that session exists and isn't
/// being deleted, else in the shown session. A URL (with `://`) or an
/// `.html`/`.htm` file opens in that session's browser tab while it has one
/// and isn't settled, else in a new one; any other file opens in the
/// editor. Without terminal-browser, a URL or `.html` file opens with the
/// desktop's opener, and so does any link with no session to open in. A
/// path is made absolute first: `~/` starts at home, and a relative path
/// starts at the session's folder, or at home with no session.
pub(crate) fn route(state: &AppState, item: &PlateItem, n: usize) -> Option<LinkRoute> {
    let address = item.links.get(n.checked_sub(1)?)?;
    let live = |id: SessionId| {
        state
            .sessions
            .session(id)
            .filter(|found| !state.sessions.deleting.contains(&found.id))
    };
    let target = live(item.session).or_else(|| state.shown_session().and_then(live));
    let dir = target.map(|session| session.dir.as_path());
    let path = (Link::of(address).kind == LinkKind::File).then(|| resolved(state, dir, address));
    let Some(session) = target else {
        return Some(LinkRoute::Outside(
            path.map_or_else(|| address.clone(), text_of),
        ));
    };
    let url = match path {
        Some(path) if !is_html(&path) => {
            return Some(LinkRoute::Editor {
                session: session.id,
                path,
            });
        }
        path => path.map_or_else(|| address.clone(), text_of),
    };
    if !state.browser_tabs.available() {
        return Some(LinkRoute::Outside(url));
    }
    let pane = state
        .browser_tabs
        .pane(session.id, &state.layouts)
        .filter(|_| session.settled_at.is_none());
    Some(match pane {
        Some(pane) => LinkRoute::BrowserTab {
            session: session.id,
            pane,
            url,
        },
        None => LinkRoute::NewBrowserTab {
            session: session.id,
            url,
        },
    })
}

/// `address` as an absolute path: kept when absolute, `~/` joined to
/// home, else joined to `dir`, or to home without one.
fn resolved(state: &AppState, dir: Option<&Path>, address: &str) -> PathBuf {
    let path = Path::new(address);
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(rest) = address.strip_prefix("~/") {
        state.home.join(rest)
    } else {
        dir.unwrap_or(&state.home).join(path)
    }
}

/// Whether `path` is a web page, by its `.html` or `.htm` ending in any case.
fn is_html(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
}

/// `path` as the text an opener is handed.
fn text_of(path: PathBuf) -> String {
    path.into_os_string()
        .into_string()
        .unwrap_or_else(|path| path.to_string_lossy().into_owned())
}

/// gx (`n` = 1) or a digit at `now`: opens link `n` of the selected item. A
/// link that opens in a tab gives the keys back where they came from,
/// enters the target session as ⏎ on its card does, and shows the tab. One
/// that opens outside orb leaves the plate open. Nothing happens on orb's
/// rows or with no link `n`.
pub(crate) fn open_link(state: &mut AppState, now: SystemTime, n: usize) -> Vec<Command> {
    let selected = Board::new(state, state.zone.midnight(now)).selected;
    let Some(RowKey::Item(id)) = selected else {
        return vec![];
    };
    let Some(route) = state.plate.item(id).and_then(|item| route(state, item, n)) else {
        return vec![];
    };
    let (session, then) = match route {
        LinkRoute::Outside(address) => {
            return vec![Command::Sessions(SessionsCommand::OpenOutside(address))];
        }
        LinkRoute::BrowserTab { session, pane, url } => {
            give_back(state);
            let mut commands = enter_pane(state, session, pane);
            commands.push(Command::Sessions(SessionsCommand::OpenInBrowser {
                pane,
                url,
            }));
            return commands;
        }
        LinkRoute::NewBrowserTab { session, url } => {
            (session, SessionsCommand::OpenBrowserTab { session, url })
        }
        LinkRoute::Editor { session, path } => (
            session,
            SessionsCommand::AddHistoryTab {
                session,
                line: editor_line(&path),
            },
        ),
    };
    give_back(state);
    let mut commands = pick_session(state, session);
    commands.push(Command::Sessions(then));
    commands
}

/// The line a browser tab's fresh shell runs to show `url`:
/// `exec terminal-browser open '<url>'`, so the tab closes when the browser
/// quits.
pub(crate) fn browser_line(url: &str) -> String {
    format!("exec terminal-browser open {}", quoted(url))
}

/// The line an editor tab's fresh shell runs to show `path`:
/// `exec sh -c '${EDITOR:-less} "$1"' orb-link '<path>'`. It runs through
/// `sh` so `${EDITOR:-less}` works whatever the pane's shell, `$EDITOR` is
/// left unquoted so an editor with flags (`code -w`) splits into words, and
/// the tab closes with the editor.
pub(crate) fn editor_line(path: &Path) -> String {
    format!(
        "exec sh -c '${{EDITOR:-less}} \"$1\"' orb-link {}",
        quoted(&path.to_string_lossy())
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{BrowserTabs, browser_line, editor_line};
    use crate::command::SessionsCommand;
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::plate::intent::{PlateKey, key, open};
    use crate::feat::plate::state::{ItemState, Plate, PlateItem, test_item};
    use crate::feat::sessions::state::{AgentStatus, Session};
    use crate::ids::{PaneId, SessionId};
    use crate::intent_handler::test_support::{
        agent, at, settle, shown_focus, state_with, with_layout,
    };
    use crate::{AppState, Command, Focus};

    /// The clock the tests press keys at.
    const NOW: u64 = 1000;

    /// Item 1's two links.
    const FIRST: &str = "https://example.com/a";
    const SECOND: &str = "https://example.com/b";

    /// The browser tab pane the reuse tests remember.
    const BROWSER: PaneId = PaneId(50);

    /// Item 1, from session 1's pane 1, with [`FIRST`] and [`SECOND`].
    fn linked_item() -> PlateItem {
        PlateItem {
            links: vec![FIRST.to_owned(), SECOND.to_owned()],
            ..test_item(1, ItemState::YourMove)
        }
    }

    /// Session 1 shown with terminal-browser there, item 1 on the plate,
    /// and the plate given the keys from the sidebar.
    fn linked() -> AppState {
        let mut state = with_layout(Focus::Sidebar);
        state.browser_tabs = BrowserTabs::new(true);
        state.plate = Plate::from_saved(vec![linked_item()]);
        open(&mut state);
        state
    }

    /// Where home is in the path tests.
    const HOME: &str = "/home/u";

    /// Session 1's folder in [`linked`].
    const DIR: &str = "/work/1";

    /// [`linked`], with item 1 holding `links` from session `session`, and
    /// home at [`HOME`].
    fn linked_to(session: i64, links: &[&str]) -> AppState {
        let mut state = linked();
        state.home = HOME.into();
        state.plate = Plate::from_saved(vec![PlateItem {
            links: links.iter().map(|&link| link.to_owned()).collect(),
            session: SessionId(session),
            ..test_item(1, ItemState::YourMove)
        }]);
        state
    }

    /// Like [`linked_to`], but with no session shown.
    fn nothing_shown(session: i64, links: &[&str]) -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        state.browser_tabs = BrowserTabs::new(true);
        state.home = HOME.into();
        state.plate = Plate::from_saved(vec![PlateItem {
            links: links.iter().map(|&link| link.to_owned()).collect(),
            session: SessionId(session),
            ..test_item(1, ItemState::YourMove)
        }]);
        open(&mut state);
        state
    }

    /// The desktop opener's command on `address`.
    fn outside(address: &str) -> Command {
        Command::Sessions(SessionsCommand::OpenOutside(address.to_owned()))
    }

    /// The sessions actor's new editor tab in session 1 on `path`.
    fn editor_tab(path: &str) -> Command {
        Command::Sessions(SessionsCommand::AddHistoryTab {
            session: SessionId(1),
            line: editor_line(Path::new(path)),
        })
    }

    /// [`linked`], with shell pane [`BROWSER`] split off agent 1's pane and
    /// remembered as session 1's browser tab, agent 1's pane focused.
    fn with_browser_tab() -> AppState {
        let mut state = linked();
        state
            .layouts
            .split(SessionId(1), Split::Right, test_entry(BROWSER.0));
        state.layouts.reveal(SessionId(1), PaneId(1));
        state.browser_tabs.remember(SessionId(1), BROWSER);
        state
    }

    /// Presses `keys` in order at [`NOW`], returning every command.
    fn pressed(state: &mut AppState, keys: &[PlateKey]) -> Vec<Command> {
        keys.iter()
            .flat_map(|&pressed| key(state, pressed, at(NOW)))
            .collect()
    }

    /// Presses `g` then `x`.
    fn gx(state: &mut AppState) -> Vec<Command> {
        pressed(state, &[PlateKey::G, PlateKey::Done])
    }

    /// The sessions actor's new browser tab in session 1 on `url`.
    fn new_browser_tab(url: &str) -> Command {
        Command::Sessions(SessionsCommand::OpenBrowserTab {
            session: SessionId(1),
            url: url.to_owned(),
        })
    }

    #[rstest::rstest]
    fn gx_opens_a_new_browser_tab_in_the_items_session() {
        // Given item 1 from session 1, which has no browser tab yet.
        let mut state = linked();

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the last command opens session 1's browser tab on link 1.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(FIRST)),
            "gx should open the first link in a new browser tab, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn opening_a_browser_tab_gives_the_keys_back() {
        // Given item 1 from session 1, the plate with the keys.
        let mut state = linked();

        // When pressing gx.
        gx(&mut state);

        // Then the plate no longer has the keys.
        assert_ne!(
            state.focus,
            Focus::Plate,
            "opening a link in a tab should give the keys back"
        );
    }

    #[rstest::rstest]
    fn a_number_opens_that_link() {
        // Given item 1 with two links.
        let mut state = linked();

        // When pressing 2.
        let commands = pressed(&mut state, &[PlateKey::Link(2)]);

        // Then the browser tab opens on the second link.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(SECOND)),
            "2 should open the item's second link, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn a_number_past_the_items_links_does_nothing() {
        // Given item 1 with two links.
        let mut state = linked();

        // When pressing 3.
        let commands = pressed(&mut state, &[PlateKey::Link(3)]);

        // Then nothing happens and the plate keeps the keys.
        assert_eq!(
            (commands, state.focus),
            (vec![], Focus::Plate),
            "an item with two links has no link 3"
        );
    }

    #[rstest::rstest]
    fn gx_on_an_orb_row_does_nothing() {
        // Given agent 1 waiting for approval, so orb's row for it is
        // selected above item 1.
        let mut state = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);
        state.browser_tabs = BrowserTabs::new(true);
        state.plate = Plate::from_saved(vec![linked_item()]);
        open(&mut state);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then nothing happens and the plate keeps the keys.
        assert_eq!(
            (commands, state.focus),
            (vec![], Focus::Plate),
            "orb's rows have no links"
        );
    }

    #[rstest::rstest]
    fn gx_reuses_the_sessions_browser_tab() {
        // Given session 1's browser tab in pane 50.
        let mut state = with_browser_tab();

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the link opens in that browser, and no new tab is asked for.
        let open_in = Command::Sessions(SessionsCommand::OpenInBrowser {
            pane: BROWSER,
            url: FIRST.to_owned(),
        });
        assert!(
            commands.contains(&open_in) && !commands.contains(&new_browser_tab(FIRST)),
            "a second link should reuse the session's browser tab, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn gx_shows_the_browser_tab_it_reuses() {
        // Given session 1's browser tab in pane 50, agent 1's pane focused.
        let mut state = with_browser_tab();

        // When pressing gx.
        gx(&mut state);

        // Then pane 50 is shown with the keys in it.
        assert_eq!(
            (state.focus, shown_focus(&state)),
            (Focus::Pane, Some(BROWSER)),
            "the reused browser tab should be shown with the keys"
        );
    }

    #[rstest::rstest]
    fn a_closed_browser_tab_is_not_reused() {
        // Given pane 99 remembered as session 1's browser tab, but no longer
        // in its layout.
        let mut state = linked();
        state.browser_tabs.remember(SessionId(1), PaneId(99));

        // When pressing gx.
        let commands = gx(&mut state);

        // Then a new browser tab is opened.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(FIRST)),
            "a closed browser tab should be forgotten, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn a_settled_sessions_browser_tab_is_not_reused() {
        // Given session 1's browser tab in pane 50, session 1 settled.
        let mut state = settle(with_browser_tab(), &[1]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then a new browser tab is opened.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(FIRST)),
            "settling stops the browser, so its tab can't take links, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn gx_on_a_settled_sessions_item_resumes_it() {
        // Given item 1 from session 1, session 1 settled.
        let mut state = settle(linked(), &[1]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then session 1 is un-settled.
        assert!(
            commands.contains(&Command::Sessions(SessionsCommand::UnsettleSession(
                SessionId(1)
            ))),
            "a link from a settled session should resume it, got {commands:?}"
        );
    }

    #[rstest::rstest]
    #[case::html("report.html", "/work/1/report.html")]
    #[case::upper_htm("page.HTM", "/work/1/page.HTM")]
    fn an_html_file_opens_in_the_browser_tab(#[case] link: &str, #[case] url: &str) {
        // Given item 1 from session 1 linking a web page by a relative path.
        let mut state = linked_to(1, &[link]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then a browser tab opens on the page's absolute path.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(url)),
            "an .html file should open in the browser tab, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn a_file_opens_in_the_editor_in_a_new_tab() {
        // Given item 1 from session 1 linking notes.md.
        let mut state = linked_to(1, &["notes.md"]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then a new tab of session 1 runs the editor on it.
        assert_eq!(
            commands.last(),
            Some(&editor_tab(&format!("{DIR}/notes.md"))),
            "a file should open in the editor in a new tab, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn opening_a_file_gives_the_keys_back() {
        // Given item 1 from session 1 linking notes.md.
        let mut state = linked_to(1, &["notes.md"]);

        // When pressing gx.
        gx(&mut state);

        // Then the plate no longer has the keys.
        assert_ne!(
            state.focus,
            Focus::Plate,
            "opening a file in a tab should give the keys back"
        );
    }

    #[rstest::rstest]
    #[case::relative("docs/notes.md", "/work/1/docs/notes.md")]
    #[case::absolute("/etc/hosts", "/etc/hosts")]
    #[case::home("~/todo.md", "/home/u/todo.md")]
    fn a_file_path_is_resolved_against_the_sessions_dir(#[case] link: &str, #[case] path: &str) {
        // Given item 1 from session 1 linking `link`.
        let mut state = linked_to(1, &[link]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the editor opens the absolute path.
        assert_eq!(
            commands.last(),
            Some(&editor_tab(path)),
            "{link} should open as {path}, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn without_terminal_browser_a_web_link_opens_outside() {
        // Given item 1 from session 1, terminal-browser not installed.
        let mut state = linked();
        state.browser_tabs = BrowserTabs::new(false);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then only the desktop's opener is asked.
        assert_eq!(
            commands,
            vec![outside(FIRST)],
            "without terminal-browser a link should open outside orb"
        );
    }

    #[rstest::rstest]
    fn opening_outside_keeps_the_plate_open() {
        // Given item 1 from session 1, terminal-browser not installed.
        let mut state = linked();
        state.browser_tabs = BrowserTabs::new(false);

        // When pressing gx.
        gx(&mut state);

        // Then the plate keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Plate,
            "a link opened outside orb should leave the plate open"
        );
    }

    #[rstest::rstest]
    fn a_deleted_sessions_link_opens_in_the_shown_session() {
        // Given item 1 from session 9, which is gone, and session 1 shown.
        let mut state = linked_to(9, &[FIRST]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the link opens in session 1's browser tab.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(FIRST)),
            "a gone session's link should open in the shown session, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn a_deleting_sessions_link_opens_in_the_shown_session() {
        // Given item 1 from session 2, being deleted, and session 1 shown.
        let mut state = linked_to(2, &[FIRST]);
        let second: Vec<Session> = (state.sessions.sessions.iter())
            .map(|session| Session {
                id: SessionId(2),
                ..session.clone()
            })
            .collect();
        state.sessions.sessions.extend(second);
        state.sessions.deleting.insert(SessionId(2));

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the link opens in session 1's browser tab.
        assert_eq!(
            commands.last(),
            Some(&new_browser_tab(FIRST)),
            "a session being deleted shouldn't take the link, got {commands:?}"
        );
    }

    #[rstest::rstest]
    #[case::web(FIRST, FIRST)]
    #[case::file("notes.md", "/home/u/notes.md")]
    fn a_deleted_sessions_link_with_nothing_shown_opens_outside(
        #[case] link: &str,
        #[case] address: &str,
    ) {
        // Given item 1 from session 9, which is gone, and no session shown.
        let mut state = nothing_shown(9, &[link]);

        // When pressing gx.
        let commands = gx(&mut state);

        // Then the desktop's opener gets the link, a path made absolute.
        assert_eq!(
            commands,
            vec![outside(address)],
            "with no session to open in, the link should open outside orb"
        );
    }

    #[rstest::rstest]
    #[case::plain(
        "/work/1/notes.md",
        r#"exec sh -c '${EDITOR:-less} "$1"' orb-link '/work/1/notes.md'"#
    )]
    #[case::space(
        "/work/1/my notes.md",
        r#"exec sh -c '${EDITOR:-less} "$1"' orb-link '/work/1/my notes.md'"#
    )]
    #[case::quote(
        "/work/1/it's.md",
        r#"exec sh -c '${EDITOR:-less} "$1"' orb-link '/work/1/it'\''s.md'"#
    )]
    fn editor_line_quotes_the_path(#[case] path: &str, #[case] expected: &str) {
        // Given a file's absolute path.
        // When building its editor tab's line.
        let line = editor_line(Path::new(path));

        // Then the shell reads the path as one word after the editor.
        assert_eq!(line, expected, "the path should be one quoted word");
    }

    #[rstest::rstest]
    #[case::plain(
        "https://example.com/a",
        "exec terminal-browser open 'https://example.com/a'"
    )]
    #[case::space(
        "/tmp/my report.html",
        "exec terminal-browser open '/tmp/my report.html'"
    )]
    #[case::quote(
        "https://x.test/?q=it's",
        r"exec terminal-browser open 'https://x.test/?q=it'\''s'"
    )]
    fn browser_line_quotes_the_url(#[case] url: &str, #[case] expected: &str) {
        // Given a link.
        // When building its browser tab's line.
        let line = browser_line(url);

        // Then the shell reads the link as one word.
        assert_eq!(line, expected, "the link should be one quoted word");
    }
}
