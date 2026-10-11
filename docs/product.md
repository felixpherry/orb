# orb — Product spec

> The product source of truth for **why** and **what**: problem, UX spec, keys, storage. Verified external facts are in [`research.md`](research.md); decisions and rejected alternatives in [`decisions.md`](decisions.md); the app's current state in `.agents/RECORD.md`.

## Problem

T3 Code (pingdotgg/t3code, a GUI for coding agents) makes it very, very easy to monitor many agent sessions, switch worktrees/branches, and switch projects. Two things are missing for this user:

1. No vim keybindings / keyboard-first navigation.
2. Native Claude Code slash commands don't work: T3 drives Claude through the Agent SDK in headless mode, where `/status` is refused and `/context` output is discarded by T3's adapter.

orb then mapped one agent session to one pane. Tools (nvim, lazygit, shells) opened as zellij floating panes and piled up across projects, orb chose the model and permission and ran its own trust confirm where Claude and pi already do, the keys depended on where the focus was, and orb needed zellij.

## Solution

orb is a terminal multiplexer that is mission control for your agents: T3 Code's sidebar with herdr's panes, written in Rust (ratatui).

- A session is one directory (a checkout, a worktree, or a Research, Learn or Incognito folder) holding tabs of split panes. The user starts `claude` or `pi` in any pane like any other program.
- Every pane runs under zmx, so quitting orb leaves every program running and orb reattaches at start.
- Claude's status comes from `claude agents --json --all` (its interactive records), matched to the pane its process runs in; pi's from an orb pi extension. A Claude `SessionStart`/`SessionEnd` hook and that extension tell orb which conversation runs in which pane, through files under `~/.orb/panes/`. `orb integration install` puts both in place.
- The sidebar stays T3 Code-style: one card per session with a row per agent pane.
- Model, permission and trust are set inside Claude and pi; orb has no drafts.
- Keys: Cmd keys move between panes and tabs as in the user's zellij setup, `<C-g>` is the which-key leader, and `<C-[>`/`<C-]>` move through the jump list.
- The wheel scrolls a pane's history and a drag copies its text, following zellij's mouse rules.

Deliberately **not** built, because native Claude already does them inside its pane: approvals, plan mode, checkpoints/rewind (`/rewind`), in-thread model picker, context meter.

## User & environment (design constraints)

- Runs straight in kitty on macOS, and in kitty 0.49.2 on Linux (EndeavourOS, niri 26.04, zmx 0.8.1 from the AUR, `zmx-bin`). On Linux the freedesktop notification server is DMS (DankMaterialShell, a quickshell 1.2 shell). It keeps critical notifications on screen; a popup that's closed or times out moves into the DMS notification centre with its click still waited on, and a click there on one that timed out was seen to reach orb's waiter. A click finds orb's niri window by orb's process ancestry. The macOS setup below doesn't apply there.
- Modifier layering: `Ctrl` → neovim, `Option` → paneru (window manager, never bound), `Cmd` → orb, which kitty forwards as Super (`map cmd+X send_key super+X`).
- Leader is `<C-g>`.
- Installed tools: nvim, lazygit, yazi, gh, python3 (for the Claude hook), terminal-notifier 3.1.0 (Homebrew; clickable notifications), pi 1.0.0 (`~/.local/bin/pi`) and zmx 0.8.1 (Homebrew tap `neurosnap/tap`; it hosts every pane). Doesn't use VS Code.
- Setup orb needs:
  - `orb integration install`, once, for the Claude hook and the pi extension.
  - The kitty `map` lines for the Cmd keys: `map cmd+X send_key super+X` for `h`, `j`, `k`, `l`, the four arrows, `n`, `x`, `plus`, `equal`, `minus`, `1` to `9`, `[`, `]`, `i` and `o`, plus `map ctrl+shift+h no_op` and `map ctrl+shift+l no_op` so kitty's own `show_scrollback` and `next_layout` let `<C-S-h>`/`<C-S-l>` through (README lists them).
  - terminal-notifier allowed in System Settings → Notifications. Until then, it prints `Notifications are not allowed for this application` and exits 3, and orb re-sends each notice through `osascript`, whose click opens Script Editor instead of going back to orb.
  - kitty's remote control on a socket (`allow_remote_control` and `listen_on` in `kitty.conf`, which set `KITTY_LISTEN_ON`), so that a notification click can bring kitty forward.

## UX specification

### Layout

Sidebar on the left, the selected session's tabs on the right (or the start screen), mode line at the bottom. The mockup's look is a stand-in until the look pass.

```
┌─ orb ─────────────────────────┬────────────────────────────────────────┐
│╭ Sessions  i  ──────────────╮ │ 1 claude  2 tests                      │
││>                       3/42│ │┌──────────────────┐┌──────────────────┐│
│╰────────────────────────────╯ ││ ✻ Refactor the   ││ ~/dev/orb        ││
│ ⠼ fix-login              ⚑ 2m ││   auth flow…     ││ ❯ just test      ││
│ ├╴□ orb               working ││                  ││                  ││
│ └╴⎇ orb/fix-login           ⌄ ││ >                ││                  ││
│    ├╴⠼ Refactor auth flow   ✳ ││                  ││                  ││
│    └╴⚠ Write tests          π │└──────────────────┘└──────────────────┘│
│ ○ docs                     3h │                                        │
│ ├╴□ orb                       │                                        │
│ └╴⎇ main                      │                                        │
│                               │                                        │
│ □ Settled                  12 │                                        │
├───────────────────────────────┴────────────────────────────────────────┤
│ PANE   orb/fix-login   orb/fix-login                1 running   14:02 │
└────────────────────────────────────────────────────────────────────────┘
```

- The sidebar is one T3-style list of sessions across projects, drawn like LazyVim's snacks.nvim explorer in tokyonight-moon (`bg_dark` behind it, one blank column on its right and no border). Icons are Nerd Font glyphs, drawn here as stand-ins (`⚑` the pin, `□` a folder, `⎇` the branch).
- An orange rounded input box heads it: ` Sessions ` and an ` i ` badge (lit blue while the sidebar search has the keys) in its top border, then a cyan `>` and, on the right, `shown/total` sessions.
- Each session is a card: its most urgent agent pane's status icon and its title, with the pin and the time on the right; `├╴` + the project's folder (a Research or Learn session's kind icon) and name, with the status word on the right; `└╴` + the branch (a Research or Learn session's folder as a `~/…` path), ending with one status icon per agent and `⌄` while its agent lines show, or `›` while `h` has folded them away; then one line per agent pane hanging one level under that line, `    ├╴`/`    └╴` + its status icon and title, with its harness mark on the right (`✳` Claude, `π` pi). Shells, nvim and other programs get no line; they show in the tab bar. The selected card's first line gets the explorer's cursorline (`bg_visual`), and the sidebar scrolls to keep the whole card in view.
- The Settled shelf sits at the bottom, a folder (open or closed) + `Settled` with its count on the right. While it's open, each settled session is a one-line row on a `├╴`/`└╴` guide.
- Status (icon and word, tokyonight colours): a braille spinner + `working` (blue; ten frames a second) · `⚠` + `approval` (yellow) · `?` + `input` (magenta) · `✓` + `done` (green; the turn ended after the user last focused the pane) · `✗` + `failed` / `⊘` + `gone` (red) · `■` + `stopped` · otherwise `○` and no word, filled while orb holds the session's panes.
- The sidebar is 32 columns wide until resized with `Cmd +`/`Cmd -`, and always between 24 and 80. `<C-g> e` hides it, and the panes take the full width. The width persists; the hidden state doesn't.
- While a project filter is set (`<C-g> f`), the input box shows the project after the `>` prompt, and only that project's sessions and Settled shelf are listed.
- Search (`/` or `i`): the keys move to the input box, and the list keeps only sessions whose title or an agent pane's title fuzzy-matches, settled ones included, with the matched characters blue and bold.
- Rename (`r` on a session): a LazyVim/snacks-style input box titled ` Rename Session `. The name is orb's own and beats every other title; Claude's and pi's own session names don't change.
- The right-hand area shows the selected session's tab bar, zellij's chevron tabs after the session's name in tokyonight-moon, over its shown tab's panes, each in a rounded frame titled with its name, while orb holds that session's panes; otherwise the start screen. A stack is a centred list of its panes, as wide as its longest name with a dim `│` on each side and a `>` outside the left bar on the shown one, on top of the shown pane's frame at full width, as zellij's stacked panes; the wheel over the list shows the next or previous stacked pane; from 11 panes the whole tab is one stack, and `<C-g> p s` stacks a new pane with the focused one. Each session has one layer of floats over all its tabs, drawn last; a pinned float stays drawn while the layer is hidden. Dragging a float's border moves it, a click on a float brings it to the top with the keys, and a click on a tiled pane hides the floats.
- The mode line is drawn like LazyVim's lualine in tokyonight-moon, on `bg_statusline`. On the left: the mode in a block of its colour (`NORMAL` blue, `PANE` teal, `PICKER` yellow, `INSERT` green; `INSERT` while typing in the rename box or the search), the selected session's branch and project. On the right: `N running` (or `fetching origin/<base>…`), the approval and input counts over every agent pane, the selected row's `at/shown` position and the local time. There are no key hints. Errors and notices (the copy notice, `pruned N worktrees`) show instead in a message box like LazyVim's notifier, just under the tab bar at the right edge: ` Error` in red or ` Messages` in cyan, 40 columns to 40% of the right side wide, long lines cut with `…`; it hides after 5 s for an error or 3 s for info, a newer message replaces it, and the same message sent again does not bring it back.

### Keys

| Focus | Keys |
|---|---|
| Sidebar | `j`/`k` next/prev session card or agent row, wrapping at the ends · `gg`/`G` first/last row · `<C-d>`/`<C-u>` half the sidebar's height · `⏎` move the keys into the session's panes (resuming it if settled; on the Settled header: open/close; on an agent row: show its tab with the keys in its pane) · `p` pin/unpin (sessions only) · `s` settle/un-settle (settling asks `No`/`Yes`; sessions only) · `d` delete (after a `No`/`Yes` confirm; on an agent row: close its pane after a `No`/`Yes` confirm) · `r` rename (on an agent row: name its pane) · `/`/`i` search · `l`/`h` show/fold a card's agent rows (`h` on an agent row folds its card) or open/close the Settled shelf · `<C-S-l>` into the panes · the Cmd keys (`Cmd h/j/k/l` do nothing here) and `<C-g>` keys below |
| Pane | every key goes to the pane's program, except the Cmd keys, `<C-S-h>`, `<C-g>`, `<C-[>` and `<C-]>` |
| Picker | typing filters (in the search picker it searches the transcripts) · `←`/`→` move the filter cursor · `Backspace`/`<C-w>` delete a char/word · `<C-j>`/`<C-k>` or `↑`/`↓` next/prev item, wrapping at the ends (focus stays in the filter input) · `<C-d>`/`<C-u>` half page, stopping at the ends · `⏎` pick (nothing in the worktree picker) · `Tab` open the highlighted directory (directory picker) · `Esc` cancel. The projects screen and the session and worktree pickers open in **list mode**, where letters are commands, not filter text: `j`/`k` or `↓`/`↑` next/prev · `gg`/`G` first/last · `<C-d>`/`<C-u>` half page · `/` search mode (the keys above; `Esc` goes back to the list keeping the text) · `d` remove the highlighted project, delete the highlighted session or delete the highlighted worktree (after a `No`/`Yes` confirm over the list; a worktree is refused while a session in it isn't settled) · `a` add a project (projects screen) · `a`/`b`/`w`/`i`/`s` show all/blocked/working/idle/settled sessions, the active filter's key again shows all (session picker) · `⏎` pick · `Esc` close |
| Rename box, search | the picker's keys: typing edits the text · `←`/`→` move its cursor · `Backspace`/`<C-w>` delete a char/word · search only: `<C-j>`/`<C-k>` or `↓`/`↑` next/prev match, wrapping at the ends · rename `⏎` saves (an empty or whitespace-only name clears orb's name, back to the agent's title) · search `⏎` clears the text and keeps the cursor on the match (with no match it acts as `Esc`) · `Esc` cancels the rename, or clears the search and puts the cursor back on the row it was on before `/` (the first row if that one is gone) · the other picker keys do nothing |

Full key table:

| Key | Action |
|---|---|
| `Cmd h/j/k/l`, Cmd arrows | Move focus between panes; at the tab's left/right edge go to the previous/next tab, wrapping, focusing the pane on the edge entered; nothing in the sidebar; while a float has the keys, move between floats only, never switching tabs |
| `Cmd n` | Add a pane: an untouched tab re-tiles in its swap layout; a tab changed by hand keeps its shape and the new pane joins the focused pane's stack, else halves the focused pane (top/bottom when tall, left/right when wide, zellij's thresholds), else stacks with it; while a float has the keys, open another float |
| `Cmd x` | Close pane (the focused float while a float has the keys) |
| `Cmd +`/`Cmd =`, `Cmd -` | Grow / shrink the focused pane or the sidebar. In a pane, `Cmd +` moves its split's border (or its stack's) 4 cells; when a step would leave a pane under 5 rows or 5 columns or push the split past its 0.1 to 0.9 ratio bound, that split becomes one stack showing the focused pane, so repeated presses stack part of a column, then the column, then the whole tab, and then zoom it (a tab's only pane doesn't zoom). `Cmd -` leaves a zoom first, then undoes each `Cmd +`, else shrinks the pane 4 cells; `Cmd +` undoes each `Cmd -`. The history holds only while the tab's tree is what the last `Cmd +` or `Cmd -` left, so an add, a close or a focus move elsewhere drops it. On the sidebar they widen and narrow it 4 columns; a float with the keys grows or shrinks 5% of the tab body on every side |
| `Cmd 1` to `Cmd 9` | Go to tab N |
| `Cmd [` / `Cmd ]` | Previous / next swap layout of the shown tab (BASE → vertical → horizontal → stacked → half-stacked), skipping ones that don't fit its pane count and wrapping; on a tab changed by hand the first press re-applies its layout and clears the mark; while a float has the keys, the float layouts instead (staggered → enlarged ≤10 → spread ≤4) |
| `Cmd i` / `Cmd o` | Move tab left / right |
| `Cmd f` | Show/hide the session's floats; with none, open a shell float |
| `<C-[>` / `<C-]>` | Jump list back / forward |
| `<C-S-h>` / `<C-S-l>` | Keys to the sidebar (showing it) / into the shown session's panes |
| `<C-g> e` | Hide/show the sidebar |
| `<C-g> q` | Quit orb |
| `<C-g> p d/r/s/f/x/c/w/e/i` | Pane: split down, split right, stack, zoom, close, rename, show/hide floats, pane to float / float to tab, pin float |
| `<C-g> t n/x/r/b/1-9` | Tab: new, close, rename, break the pane out to a new tab (in a pane), go to N |
| `<C-g> ␣` | Session picker |
| `<C-g> n` | New session |
| `<C-g> P` | Projects screen |
| `<C-g> f` | Project filter |
| `<C-g> i` | Incognito session |
| `<C-g> w` / `<C-g> b` | Workspace / branch |
| `<C-g> g r` / `<C-g> g l` | New Research / Learn session |
| `<C-g> /` | Grep transcripts |
| `<C-g> W` | Worktree picker |
| `<C-g> <C-g>` | Send Ctrl g to the focused pane |

- Keys not listed here are defined by the user in a plan. Agents don't invent bindings.
- Routing order for a key: a picker, the rename box or the search takes it first; then the Cmd keys; then `<C-S-h>`/`<C-S-l>`; then `<C-[>`/`<C-]>` (bare Ctrl only); then `<C-g>` and which-key; then the sidebar's plain keys while the sidebar has the keys; else the focused pane.
- `<C-g> <C-g>` sends Ctrl g to the focused pane, which Claude uses to open the prompt in `$EDITOR`.
- `<C-[>` never matches a plain Esc: under kitty's disambiguate flag Ctrl+[ arrives apart from Esc. The legacy `<C-]>` byte reads as Ctrl+5 and isn't matched (research §21).
- In a pane, a key that doesn't continue a pending `<C-g>` sequence is swallowed, and Esc closes the popup.
- Keys are bound by what's selected, so which-key never lists a key that does nothing there: `<C-g> w`/`<C-g> b` not on Incognito, Research or Learn sessions; `+pane` and `<C-g> <C-g>` in panes only; `<C-g> f` in the sidebar only; `p`/`s` only on a session card, `r`/`d` on a card or an agent row.
- `<C-g> q` quits orb from the sidebar or a pane; every pane keeps running under zmx. There is no `:` command line (dropped in M9; see Backlog).
- **Look** (LazyVim's default which-key, the helix preset, in tokyonight-moon): a rounded `#589ed7` box on `bg_dark` in the bottom-right corner, sitting on the mode line, as tall as its rows (cut off when the screen is shorter); the pending keys in the top border in orange (`<C-g>` for the leader); one `key ➜ icon desc` row per next key (key cyan and right-aligned, `➜` dim, a Nerd Font icon in its own colour, the description magenta, a group as blue `+name`) in which-key's order (letters and digits before symbols, lowercase before its capital); `esc close  ⌫ back` on the last row.

### Start screen

The right-hand area's start screen whenever no session whose panes orb holds is selected. It is LazyVim's dashboard (snacks.dashboard), the `lazy` look, in tokyonight-moon, centred in the area.

- Banner: ORB in ANSI Shadow block letters, the solid blocks fading from blue to violet row by row and their shadow in darker tones of the same, with four moons rising off the B and a few dim stars.
- Footer: `✳ N working · N sessions · N projects` (numbers magenta, words blue); removed projects aren't counted.
- Pane error: when a pane's client can't start, the reason shows in red one line under the footer until a pane starts. A missing zmx shows here.
- No menu, no keys and no block cursor.

### Picker (ported from jinn's `jinn-selection-widget`)

- Empty filter → ordered by **recency** (most recently used first).
- Typing → ordered by **fuzzy score**, recency breaks ties.
- `<C-j>`/`<C-k>` move the selection and wrap at the ends, as snacks does (new; jinn doesn't have these).
- **Look** (every picker but the session, worktree and search pickers; LazyVim's `vim.ui.select` in tokyonight-moon): a rounded `#589ed7` float on `bg_dark`, 44–72 columns wide (wider when its name needs it, up to the screen's width) and as tall as its rows plus 4 (at most 60% of the screen), its top fixed where the unfiltered list's would be; the picker's name centred in the top border; a cyan `>` prompt over an orange rule (a list picker's prompt is a `/` instead: dim with a dim `search projects`/`search worktrees`/`search sessions` placeholder, or the kept text dim, while listing; bright, with bright text, once `/` starts a search); numbered one-line rows with a Nerd Font icon, the selected row filled `#2d3f76`; matched characters blue and bold; `No results` when nothing matches; dim key hints right-aligned in the bottom border (`⏎ select · Esc close`; the directory picker has `⏎ add · Tab open`, the project filter `⏎ filter`, the confirms `⏎ confirm · Esc cancel`). A list picker's hints name its list keys while listing — the projects screen `↑↓/j/k rows · / search · a add · d remove · ⏎ new session · Esc close`, the worktree picker `↑↓/j/k rows · / search · d delete · Esc close`, the session picker `↑↓/j/k rows · / search · a/b/w/i/s filter · d delete · ⏎ open · Esc close` — and only `⏎ <verb> · Esc back` while searching (`Esc back` in the worktree picker, where `⏎` does nothing). The session and worktree pickers draw them left-aligned on the popup's bottom row, under both boxes.
- **`<C-g> n`, the project picker.** One-line rows: a folder in the project's badge colour, then its path with `~/` and the parent dimmed and the name bright. Order is T3's: the project whose sessions have the newest activity first, a project with no sessions by when it was added, ties by title. The filter matches name **and** path. `⏎` goes on to the workspace picker (New session flow).
- **`<C-g> P`, the projects screen.** A list-mode picker titled `Projects`, rows in `<C-g> n` order and looking like its rows. `⏎` opens the `New session` box for the highlighted project, `a` opens the directory picker over the list, `d` removes the highlighted project (below), `/` searches. The list refreshes when projects change.
- **The directory picker** (T3's browse mode), opened by `a` on the projects screen. Opens with `~/` typed; rows are a folder glyph and the name, sorted alphabetically ignoring case. Dot-directories are hidden unless the last part of the path starts with `.`. The last part is fuzzy-filtered. `Tab` opens the highlighted directory, and backspacing past a `/` goes up a level. `⏎` adds the highlighted directory as a project (or the typed directory when nothing is highlighted and the last part is empty); the new project sits first in `<C-g> n`, and the picker returns to the projects screen. `Esc` returns there too. `⏎` on a removed project's directory restores it (below).
- **`<C-g> f`, the project filter** (T3's project scope, sidebar only). Rows are `All projects`, then orb's `Research`, `Learn` and `Incognito` projects, which read just their names, then the projects in `<C-g> n` order, with the current filter highlighted when it opens. `⏎` filters the sidebar to one project, or back to all. The filter persists. Picking a project outside the filter in `<C-g> n` clears it.
- **Removing a project** (`d` on a row of the `<C-g> P` projects screen) opens a `Remove project?` confirm over the list with `No` selected, then `Yes`; either answer returns to the list. `Yes` soft-removes the project: it leaves `<C-g> n` and `<C-g> f`, while its sessions stay in the sidebar. A filter to it resets to All.
- **`<C-g> ␣`, the session picker.** It lists the sessions inside the project filter, the selected one included, newest chat first by the later of an agent pane's current turn start and its last turn end, leaving out sessions being deleted. It opens in list mode showing all but settled sessions; `a`/`b`/`w`/`i`/`s` show all, blocked (waiting on approval or input), working, idle or only settled sessions, pressing the active filter's key again goes back to all, and the title names the active filter (`Sessions · blocked`). `d` asks `Delete session?` (or the folder variant) over the list; `Yes` deletes it and drops the row, moving the sidebar cursor only when it was on that session. A row is the session's status icon, the label `<project>/title` with the prefix dimmed, and on the right the time since the last chat. Typed text fuzzy-matches the whole label. The rows and their order are fixed when it opens, while status, spinner and time draw live. `⏎` reveals the session and moves the keys into its panes, and records a jump. Its look is LazyVim's snacks picker: a list box titled `Sessions` and a preview box side by side from 120 columns, stacked below that. The preview shows the status, branch and model of the session's most recently active agent pane, then its latest exchanges (the prompt, the tools the session ran, and the end of its last reply as Markdown), or `No transcript yet`, and refreshes while the transcript grows.
- **`<C-g> W`, the worktree picker** (Worktree pruning). It lists every directory under `~/.orb/worktrees/<repo>/`: active ones first, then settled ones, then the ones nothing uses, most recently used first within each. A row is a status icon, a dim `<repo>/`, a bright `orb-<hex>`, an orange `●N` when N files are uncommitted, and on the right `active · <title>`, `settled <age>` or `no session`. The preview shows the path, branch, size, changes, last commit, last use, the sweep's verdict (`kept: a session in it is attached`, `… an agent in it is mid-turn`, `… a session in it is active`, `kept: uncommitted changes`, `prunes at next sweep`, `prunes in Nd`/`Nh`) and the sessions that use it. `⏎` does nothing. `d` asks `Delete worktree?` (or `Delete worktree and its uncommitted changes?`) with `No` selected; `Yes` force-removes the worktree and keeps its branch. It's refused, with the reason on the mode line, while a session in it isn't settled.
- **`<C-g> /`, the search picker** (Global search). It searches the prompts the user typed and the agents' text replies in every agent pane's transcripts, settled sessions included, ignoring the project filter; tool calls, tool output and thinking are left out. Every whitespace-separated term of 3 or more characters must match as a case-insensitive substring. Each row is one matching message, newest first, at most 200: a dim `<project>/`, the bright title, then a dim snippet with its matches lit. The preview shows the exchange the message is in, the match scrolled about a third down. `⏎` reveals the session and moves the keys into its panes, and records a jump.

### New session flow

1. `<C-g> n` → project picker → workspace picker (current checkout, a new worktree, or the project's previous worktree) → for a new worktree, the branch picker with the default branch first → create (fetch at most 15 s, `git worktree add` on `orb/<hex>`) → a session with one tab of one pane running `$SHELL` → the session is selected and its pane has the keys.
2. A project that isn't a git repository skips the workspace picker and gets its session in its checkout; `<C-g> w`/`<C-g> b` there offer `Initialize Git`.
3. Research/Learn: `<C-g> g r`/`<C-g> g l` → a name in the rename box → a folder copied from the template → a session of that kind with one shell.
4. Incognito: `<C-g> i` → a session in `/tmp/orb-incognito` with one shell.
5. A failure removes the worktree and branch orb made for the session and shows the reason on the mode line.
6. `<C-g> w` on a session before its first agent turn picks a new workspace the same way, kills every pane, and restarts them as shells in the new directory; the old worktree goes if orb made it and nothing else uses it.

The user starts `claude` or `pi` in the shell, and sets the model and permission inside it. There is still no orb-owned prompt composer (rejected: it would lose Claude's pickers/autocomplete).

### Sidebar & settle lifecycle (T3 semantics, MVP subset)

- Sections, in one list across projects: **Pinned** (newest pin first), **Active** (newest of created or un-settled first), **Settled** (collapsible shelf at the bottom, most recently settled first). Only the shelf has a visible header. A selected settled session stays visible while the shelf is collapsed.
- Manual settle / un-settle / pin (`s`, `p`). `s` asks `Settle <name>?` and `d` asks `Delete <name>?` (`Delete <name> and its folder?` for a Research or Learn session), with `No` selected; `Esc` or `No` does nothing. Un-settling needs no confirm.
- A manual settle is refused with `Can't settle while a session is working` while any agent pane has a turn underway, and removes the pin. Pinning a settled session un-settles it.
- Activity in an agent pane (a turn running, approval/input requested) **un-settles** its session.
- An unpinned session **auto-settles after 3 days** without turn activity in any of its agent panes, unless the user un-settled it since that activity or it is the selected session. A session with no agent pane never auto-settles.
- A **manual un-settle** blocks auto-settle until the next real activity (T3's `settled_override = 'active'`).
- **Settling kills every pane** of the session (`zmx kill`), keeping its tabs, splits and each pane's resume command. `⏎` on a settled session restores the layout with a shell in each pane and types `claude --resume <id>` or `pi --session-id <id>` into panes whose last program was an agent.
- **Delete** (`d`) kills every pane, deletes the session's tabs, panes and threads, and removes the row; a Research or Learn session's folder goes too. Claude transcripts and pi session files stay, and so does a worktree, which the sweep prunes later.
- **Selection**: after a settle or a delete, the selection moves to the next session below, else the one above. Un-settle and pin keep the selection.
- "Completed-unseen" = an agent pane's latest turn ended after the user last focused it.
- Status priority when several apply: approval > input > working > failed > completed-unseen > idle. A session's status is its most urgent agent pane's.

### Panes, tabs and splits

- Every pane runs its program under zmx, as the session `orb-p<id>` on the socket dir `~/.orb/zmx`, so quitting orb leaves it running and orb reattaches at start. A new pane runs `$SHELL` in its session's directory with `ORB_PANE_ID` set.
- Each tab's panes are a tree of right and down splits, ported from herdr's layout (Apache-2.0, `NOTICE`). Splitting, closing, zooming, resizing and moving between panes and tabs use the keys above; a click on a pane focuses it.
- A pane closes when its program exits, a tab closes with its last pane, and a session whose last pane closes gets a new shell tab in its directory in its place, with the keys in it, and stays where it was in the sidebar.
- Each pane keeps 10 000 lines of history, from what orb saw after it attached (zmx sends the screen on reattach, not the history). The wheel over a pane goes to its program while it tracks the mouse, sends arrow keys on the alternate screen, and otherwise scrolls the history.
- Copy on select follows zellij: a left drag in a pane whose program doesn't track the mouse selects text, and the release copies it through OSC 52. A double-click selects a word, a triple-click a line, and dragging past the top or bottom edge scrolls. A press on another pane starts a selection only when its program doesn't track the mouse; otherwise it only focuses.

### Storage & paths (T3 layout)

- orb state: `~/.orb/userdata/state.sqlite` (SQLite, plain migrations, jinn's `jinn-session-schema` pattern; no event sourcing). Since v11 it holds `sessions`, `tabs` and `panes`, and each thread row points at the pane it runs in. Before every upgrade orb copies the file to `state.sqlite.bak-v<old version>` and keeps the 3 newest copies; a database from a newer orb is refused with the name of the copy to put back; a test build (not run from `~/.cargo/bin`) refuses to upgrade unless started with `--upgrade-data`. The `groups`, `drafts` and `last_used` tables and the old thread columns stay, unread.
- Log: `~/.orb/orb.log`, one line per start, error, kill, worktree prune, branch delete, session delete, dropped pane row, database upgrade copy and pruned old copy; at 5 MB it moves to `orb.log.1`, replacing the old one.
- Search index: `~/.orb/userdata/search.sqlite` (Global search), an FTS5 `trigram` index of the prompts and text replies in the agent panes' transcripts (Claude's transcripts and pi's session files), owned by the `SearchActor` through its own connection. It's a projection: orb builds a fresh one when it's missing, and deletes and rebuilds it when it can't be read or holds another schema version, and `state.sqlite` never refers to it. Rows are keyed by the thread and leave the index once its session is deleted; a pane's transcripts from before a `/clear` stay.
- zmx: new panes' sockets are in `~/.orb/zmx`, named `orb-p<id>`. Panes migrated from pi threads keep their `orb-<32 hex>` session under `~/.orb/pi`.
- Integrations: the Claude hook script is `~/.orb/hooks/orb-agent-state.sh`, registered in Claude's `settings.json`, and the pi extension is `~/.pi/agent/extensions/orb-agent-state.ts`. Both write `~/.orb/panes/<pane id>.json` (research §21).
- Worktrees: `~/.orb/worktrees/<repo>/orb-<hex>` on branch `orb/<hex>`. After the first agent turn ends, the branch is renamed `orb/<slug>` from the session's title; the directory keeps its name (T3 does the same).
- Pruning: orb's `WorktreesActor` sweeps `~/.orb/worktrees/<repo>/` at start and then every hour. With a non-forced `git worktree remove`, which keeps the branch, it removes each worktree nothing uses, and each one whose sessions are all settled, the latest at least 3 days ago. It keeps a worktree with an attached session, an agent mid-turn, or an unsettled session, one with uncommitted or untracked files, and a directory git can't read as a linked worktree. Ignored files (`target/`, `node_modules/`) go with the worktree. When it removes any, the mode line shows `pruned N worktrees` (`pruned 1 worktree`) until the next key. Resuming a session whose worktree is gone recreates it at the same path on the session's branch, made with `-b` from the fetched default branch when that branch is gone; Claude keys its transcripts by the path, so the conversation resumes.
- Research and Learn: a session's folder is `~/.orb/research/<slug>/` or `~/.orb/learn/<slug>/`, made by copying `~/.orb/templates/<kind>/` (symlinks kept), which orb first writes from its built-in default when it's missing (Research: the research kit, an orchestrator `AGENTS.md`, `.claude/agents/{investigator,falsifier,simulator}.md`, `.pi/agents/{investigator,falsifier,simulator}.md`, `.research/{CONVENTIONS,REPORT_TEMPLATE}.md`, `SOURCES.md`, `.gitignore` and `.claude/settings.json`; Learn: the learn kit, an orchestrator `AGENTS.md`, `.claude/agents/saboteur.md`, `.pi/agents/saboteur.md`, `.learn/{STORYTELLER,CHALLENGES,TUTOR,CONVENTIONS}.md`, `.learn/ingest_epub.py` and `.gitignore`; both with `CLAUDE.md -> AGENTS.md`); an existing folder is refused, never overwritten. Those sessions belong to orb's `Research` and `Learn` projects (roots `~/.orb/research`, `~/.orb/learn`), added by the first `<C-g> g r`/`<C-g> g l`: `<C-g> n` doesn't list them, and `<C-g> f` lists them right after `All projects`.
- Incognito: orb's `Incognito` project (`ProjectKind::Incognito`) is rooted at `/tmp/orb-incognito/` and holds every incognito session. The `SessionsActor` adds it at every start, un-removing it if it was removed, and creates the folder then and again before each incognito session. `<C-g> n` doesn't list it, and `<C-g> f` lists it after Research and Learn. Its sessions settle, delete, rename and resume like any other, and `<C-g> w`/`<C-g> b` aren't bound on them.
- pi: pi writes its session file on the first prompt, into `~/.pi/agent/sessions/--<cwd, its / as ->--/`, or straight into `PI_CODING_AGENT_SESSION_DIR` when that's set (orb reads it at start); orb never deletes the file.


## Risks

- **Transcript format is undocumented** → lenient parsing + real-transcript fixtures; each line carries `version`.
- **zmx is required for every pane** → it sits behind the `Zmx` trait, and a pane whose client can't start shows the reason on the start screen.
- **orb's keys in panes**: `<C-g>` and `<C-[>`/`<C-]>` never reach a pane's program; `<C-g> <C-g>` sends Ctrl g.
- **The Claude hook needs `python3`**: without it orb sees no Claude identity.

## Backlog

**To plan** (scope decided in the planning session):
- Arena: one prompt run across several models and harnesses (Claude Code, pi), each in its own worktree, with the runs compared side by side. Dashboard round 3's `arena` variant (branch `dashboard-rework-opus-cc`) is a stub of the comparison view.
- Agents controlling orb: CLI subcommands or a socket for splitting panes, sending keys and reading output.
- A keyboard copy mode, like zellij's scroll and search modes.
- Opening several tabs automatically when a session starts.
- Agents started before `orb integration install` ran: they never report, so their panes read as shells until the agent is restarted.
- Workflow tooling. First candidate: the selected thread's task list, like jinn's todo list, read from the transcript; Claude Code already writes one, and pi has no todo tool. Other tools undecided.
