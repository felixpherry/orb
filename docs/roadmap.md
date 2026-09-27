# orb — Roadmap

> Approved 2026-09-24. Each milestone is planned in its own session: run `/plan` for "Milestone N" and point it at this file plus [`research.md`](research.md).
> This file is the product + engineering source of truth for **why** and **what**. `research.md` holds the verified external facts (versions, CLI flags, file formats) the decisions rest on.
> At the end of milestone N, write the **MN** group of *Record updates* (bottom of this file) into `.agents/RECORD.md`. Don't skip this.

## Problem

T3 Code (pingdotgg/t3code, a GUI for coding agents) makes it very, very easy to monitor many agent sessions, switch worktrees/branches, and switch projects. Two things are missing for this user:

1. No vim keybindings / keyboard-first navigation.
2. Native Claude Code slash commands don't work: T3 drives Claude through the Agent SDK in headless mode, where `/status` is refused and `/context` output is discarded by T3's adapter.

## Solution

**orb** is a Rust terminal app (ratatui) — a vim-first T3 Code for the terminal, Claude Code only.

- **Sessions are hosted by Claude's own background supervisor** (`claude --bg`). orb reads status by polling `claude agents --json --all`. Sessions survive quitting orb.
- **Preview**: the right side renders the selected thread's Claude transcript JSONL as jinn-style navigable blocks. Instant; spawns no process.
- **Attach**: `⏎` runs the real `claude attach <id>` in a PTY, emulated by `alacritty_terminal`, rendered in the right-hand area. Because it's the real Claude TUI, every native command works (`/status`, `/context`, `/mcp`, `/config`, permission dialogs, Claude's vim mode, skills, pickers). `<C-\>` returns to orb.
- **From T3 Code (product/UX)**: sidebar (thread cards across projects, status, elapsed time), drafts that hold setup only, worktree layout, settle rules.
- **From jinn (`~/dev/jinn`)**: architecture and coding conventions, telescope-style picker (`jinn-selection-widget`), block-style chat view, keymap + which-key popup.
- **Tools** (shell, lazygit, nvim) open as zellij floating panes in the thread's worktree — replacing T3's integrated terminal and "Open in VS Code" button — with de-duplication.

Deliberately **not** built, because native Claude already does them inside the attached pane: approvals, plan mode, checkpoints/rewind (`/rewind`), in-thread model picker, context meter.

## User & environment (design constraints)

- Always runs inside **zellij 0.45** inside **kitty** on macOS. Never vanilla kitty.
- **Modifier layering is taken**: `Ctrl` → neovim, `Option` → paneru (window manager), `Cmd` → zellij (kitty maps `cmd+X` → `super+X`). orb must not rely on Alt/Option or Cmd.
- Zellij defaults to **locked** mode; `Ctrl g` toggles it. So `Ctrl g` is unavailable to orb.
- Leader is **`<Space>`**, always.
- Installed tools: nvim, lazygit, yazi, gh, terminal-notifier 3.1.0 (Homebrew; M9's clickable notifications). Doesn't use VS Code.
- Setup orb needs (M9):
  - macOS "Move left a space" and "Move right a space" turned off (System Settings → Keyboard → Keyboard Shortcuts… → Mission Control). Until then, `<C-Left>`/`<C-Right>` never reach kitty.
  - terminal-notifier allowed in System Settings → Notifications. Until then, it prints `Notifications are not allowed for this application` and exits 3, and orb re-sends each notice through `osascript`, whose click opens Script Editor instead of going back to orb.
  - kitty's remote control on a socket (`allow_remote_control` and `listen_on` in `kitty.conf`, which set `KITTY_LISTEN_ON`), so that a notification click can bring kitty forward. Without it, the click only switches zellij tab and pane.

## UX specification

### Layout

Sidebar on the left, preview (or attached Claude) on the right, mode line at the bottom.

```
┌─ orb ─────────────────────────┬────────────────────────────────────────┐
│╭ Sessions  s  ──────────────╮ │ Plan Settle Lifecycle · main · opus    │
││>                      3/130│ │ ┃ you     make it look like snacks     │
│╰────────────────────────────╯ │   claude  I'll start with sidebar.rs…  │
│ ⠼ Plan Settle Lifecycle  ⚑ 2m │   ▸ Read  crates/orb-tui/src/sidebar.rs│
│ ├╴□ orb               working │   ▸ Edit  sidebar.rs  +48 −12          │
│ └╴⎇ main                    ✳ │   claude  Done — tree nodes in place.  │
│ ⚠ Fix pane resize          3h │                                        │
│ ├╴□ paneru           approval │                                        │
│ └╴⎇ main                    ✳ │                                        │
│ ✓ Worktree reuse           5m │                                        │
│ ├╴□ orb                  done │                                        │
│ └╴⎇ orb/worktree-reuse      ✳ │                                        │
│                               │                                        │
│ □ Settled                 127 │                                        │
├───────────────────────────────┴────────────────────────────────────────┤
│ NORMAL                                                       1 running │
└────────────────────────────────────────────────────────────────────────┘
```

- The sidebar is one T3-style list across projects, drawn like LazyVim's snacks.nvim explorer in tokyonight-moon (`bg_dark` behind it, one blank column on its right and no border). Icons are Nerd Font glyphs, drawn here as stand-ins (`⚑` the pin, `□` a folder, `⎇` the branch).
- An orange rounded input box heads it: ` Sessions ` and an ` s ` badge (lit blue while the Settled shelf is open) in its top border, then a cyan `>` and, on the right, `shown/total`, like snacks' match count: the drafts and threads listed out of all of them.
- Each draft or thread is a 3-line tree node, with no blank lines between nodes. A thread's is its status icon + title, with the pin and the time on the right (the elapsed time while working, else the time since its last turn); then `├╴` + the project's folder (in the badge colour hashed from its name) + the project, with a status word on the right; then `└╴` + the branch, with the Claude logo `✳` (Claude orange) on the right. A draft's is `✎ New thread` marked `draft`; the project; `└╴` + its workspace (`local` / `new worktree` / `worktree`) and base branch. The selected row's first line gets the explorer's cursorline (`bg_visual`), and the sidebar scrolls to keep the whole node in view.
- The Settled shelf sits at the bottom, a folder (open or closed) + `Settled` with its count on the right. While it's open, each settled thread is a one-line row on a `├╴`/`└╴` guide: a dim check (a red icon if it failed or is gone), the title, and the time since it settled.
- Status (icon and word, tokyonight colours): a braille spinner + `working` (blue; ten frames a second) · `⚠` + `approval` (yellow) · `?` + `input` (magenta) · `✓` + `done` (green; the turn ended while the user was on another thread) · `✗` + `failed` / `⊘` + `gone` (red) · `■` + `stopped` · otherwise `○` and no word.
- The sidebar is 32 columns wide until resized, and always between 24 and 80 (M9). `␣e` hides it, and the right-hand area, attached pane included, then takes the full width. The width persists; the hidden state doesn't.
- While a project filter is set (`␣f`, M9), the input box shows the project after the `>` prompt (its folder and name), and only that project's drafts, threads and Settled shelf are listed.
- The mode line shows only the mode's name on the left (`NORMAL`, `DRAFT`, `ATTACHED`, `PICKER`), with no key hints. The right side shows `starting session…`, the latest error, or `N running` (every running thread, including ones the filter hides).

### Keys

| Focus | Keys |
|---|---|
| Sidebar | `j`/`k` next/prev thread or draft — preview follows instantly · `gg`/`G` first/last row · `<C-d>`/`<C-u>` as many rows as fit in half the sidebar's height (at least one) · `<C-l>` focus the right-hand area (the preview, or back into the Claude pane while it's shown) · `<C-Right>`/`<C-Left>` widen/narrow the sidebar · `⏎` attach (on a draft: start it) · `p` pin/unpin · `ss` settle/un-settle · `xx` delete (on a draft: discard it) · on the Settled header: `⏎` open/close, `l` open, `h` close (`h` on a settled thread in the open shelf also closes it) · `␣n` project picker (opens the project's draft) · `␣p` add project · `␣f` project filter · `␣e` hide the sidebar · `␣w` workspace · `␣b` branch · `␣t` shell · `␣g` lazygit · `␣v` nvim (on a thread or draft) · on a draft: `␣m` model, `␣a` permission · `q` quit orb |
| Preview | `j`/`k` next/prev block · `C-d`/`C-u` half page · `gg`/`G` top/bottom · `y` yank block raw text · `za`/`<Tab>` fold tool output · `<C-h>` back to sidebar (nothing while it's hidden) · `<C-Right>`/`<C-Left>` widen/narrow the preview (the sidebar narrows/widens; nothing while it's hidden) · `␣e` hide or show the sidebar · `⏎` attach (on a draft's form: start it) · `␣n` project picker · `␣p` add project · `␣w` workspace (a thread: before the first prompt) · `␣b` branch · `␣t` shell · `␣g` lazygit · `␣v` nvim (on a thread or draft) · on a draft's form: `␣m` model, `␣a` permission |
| Picker | typing filters · `←`/`→` move the filter cursor · `Backspace`/`<C-w>` delete a char/word · `<C-j>`/`<C-k>` or `↑`/`↓` next/prev item (focus stays in the filter input) · `<C-d>`/`<C-u>` half page · `⏎` pick · `Tab` open the highlighted directory (directory picker) · `<C-x>` remove the highlighted project (project filter, after a `No`/`Yes` confirm) · `Esc` cancel |
| Attached | **every** key → Claude, except `<C-\>` → the thread's preview, and `<C-h>` → the sidebar with the Claude pane kept drawn (swallowed while the sidebar is hidden) |

- Keys not listed here are defined by the user in that milestone's `/plan`. Agents don't invent bindings.
- `h`/`l` in the sidebar only close/open the Settled shelf; no `i` binding. Window moves are `<C-h>`/`<C-l>`, like neovim.
- `<C-\>` (single key) was chosen over nvim's `<C-\><C-n>`. Esc must reach Claude (it interrupts turns / is vim-mode Esc).
- The right-hand area shows the selected thread's preview or its live Claude pane, and `<C-h>`/`<C-l>` move focus between the sidebar and whichever it shows (M9):
  - `<C-h>` while attached focuses the sidebar and keeps the Claude pane drawn on the right. Claude never gets that key; it can't bind `ctrl+h` anyway, because a legacy terminal sends it as Backspace. `<C-l>` goes back into the same pane.
  - Selecting another thread drops the pane and shows that thread's preview, and `<C-l>` then focuses the preview. The attach process stays alive until another thread is selected, so `⏎` re-enters instantly.
  - `<C-\>` switches the right side to the **same thread's transcript preview**, and focus stays right. `<C-l>` from the sidebar then goes to the preview, not the pane.
  - If Claude exits while its pane is shown under sidebar focus, the preview takes its place, and focus stays in the sidebar.
  - In the trust pane, `<C-h>` works like `<C-\>` (it retries the start) but lands on the sidebar.
- `␣e` works like LazyVim's explorer toggle (M9). Hiding the sidebar moves focus to the right-hand area, which takes the full width. If the Claude pane is shown, hiding re-enters it at full width. Showing the sidebar focuses it. While it's hidden, `<C-h>` and resizing do nothing (silently), and while attached, `<C-h>` is swallowed. `␣e` isn't bound while attached, so it goes to Claude.
- `<C-Right>` widens the focused side and `<C-Left>` narrows it, as LazyVim's window-width keys do, so the direction flips between the sidebar and the preview (M9). A step is 4 columns, and the sidebar stays between 24 and 80. The two keys are routed ahead of which-key, because `ratatui-which-key` can't parse `<c-left>`: the popup doesn't list them, and they cancel a pending sequence. macOS takes them for "Move left/right a space" until those shortcuts are turned off (see User & environment).
- Leader chords show a which-key popup. `q` quits orb; sessions keep running. There is no `:` command line (dropped in M9; see Backlog).
- Keys are bound by what's selected, so which-key never lists a key that does nothing there. `␣f` works only in the sidebar, `␣m`/`␣a` only on a draft, `␣w`/`␣b`/`␣t`/`␣g`/`␣v` only on a thread or a draft, and `p`/`ss` only on a thread. The preview's block keys (`j`/`k`, `C-d`/`C-u`, `gg`/`G`, `y`, `za`/`<Tab>`) work only on a thread's preview, not on a draft's form. Keys whose effect depends on runtime state stay bound and explain themselves on the mode line (`Workspace locked`, the busy refusal).

### Preview behavior

- Blocks: You · Claude text (markdown + code highlighting) · Thinking (collapsed) · Tool call (one-line summary + ✓/✗, output folded) · System (compaction, API errors, local command output).
- Tool one-liners: Bash → `$ <command>`; Edit → `Edit <file> +a −d`; Read/Write → path; Grep/Glob → pattern; others → name + first arg.
- Follows the tail when scrolled to the bottom.
- Granularity: Claude writes one JSONL line per finished content block, so text appears per block, not per token. Liveness comes from the sidebar status.

### Picker (ported from jinn's `jinn-selection-widget`)

- Empty filter → ordered by **recency** (most recently used first).
- Typing → ordered by **fuzzy score**, recency breaks ties.
- `<C-j>`/`<C-k>` move the selection (new; jinn doesn't have these).
- **Look** (LazyVim's `vim.ui.select` in tokyonight-moon): a rounded `#589ed7` float on `bg_dark`, 44–72 columns wide and as tall as its rows plus 4 (at most 60% of the screen), its top fixed where the unfiltered list's would be; the picker's name centred in the top border; a cyan `>` prompt over an orange rule; numbered one-line rows with a Nerd Font icon, the selected row filled `#2d3f76`; matched characters blue and bold; `No results` when nothing matches; dim key hints right-aligned in the bottom border (`⏎ select · Esc close`; the directory picker has `⏎ add · Tab open`, the project filter `⏎ filter · <C-x> remove`, the confirms `⏎ confirm · Esc cancel`).
- **`␣n`, the project picker.** One-line rows: a folder in the project's badge colour, then its path with `~/` and the parent dimmed and the name bright (the name on the right when the folder is named differently). Order is T3's: the project whose threads have the newest last activity (`last_activity_at`) first, a project with no threads by when it was added, ties by title. The filter matches name **and** path. `⏎` selects the project's `✎` draft, creating it if there is none, and focuses its form. It never starts a session (M7; M5 started a local session here).
- **`␣p`, the directory picker** (T3's browse mode). Opens with `~/` typed; rows are a folder glyph and the name, sorted alphabetically ignoring case. Dot-directories are hidden unless the last part of the path starts with `.`. The last part is fuzzy-filtered. `Tab` opens the highlighted directory, and backspacing past a `/` goes up a level. `⏎` adds the highlighted directory as a project (or the typed directory when nothing is highlighted and the last part is empty); it doesn't start a session, and the new project sits first in `␣n`. `⏎` on a removed project's directory restores it (below).
- **`␣f`, the project filter** (T3's project scope, sidebar only, M9). Rows are `All projects`, then the projects in `␣n` order as `␣n`'s one-line rows, with the current filter highlighted when it opens. `⏎` filters the sidebar to one project, or back to all. A cursor on a row the filter hides moves to the first listed row. The filter persists. Picking a project outside the filter in `␣n` clears it; picking the filtered one keeps it.
- **Removing a project** (`<C-x>` on a project row of `␣f`) opens a `Remove project?` confirm with `No` selected, then `Yes`. `Yes` soft-removes the project: it leaves `␣n` and `␣f` and loses its draft, while its threads stay in the sidebar. A filter to it resets to All. `<C-x>` on `All projects`, `No` and `Esc` do nothing. The confirm doesn't name the project yet (UI pass).

### New thread flow (drafts)

1. `␣n` → project picker.
2. The project's `✎` draft is selected, created if it has none (at most one per project). Drafts sit above pinned threads, newest first, as T3's draft card: `✎` + badge + project, then `New thread`. The right side shows the **draft form** (orb UI, not Claude), and focus moves to it. The mode line reads `DRAFT`.
3. Fields: Workspace · Base branch · Model · Permission. Each has a leader key that opens its picker, in the sidebar and in the form: `␣w` / `␣b` / `␣m` / `␣a`. The form shows no key hints, and there's no field cursor.
   - **Workspace** (T3's rows): `Current checkout`, `New worktree`, `Previous worktree (<branch>)`. On a draft already in a worktree the rows are `Current worktree` (keeps it), `New worktree`, and the other previous worktree. The worktree is created only at Start.
   - **Base branch** follows T3's per-workspace rules. On a new-worktree draft a pick only records the base. The form reads `From <ref>`, the ref Start will use as of the last fetch (`origin/<b>` when origin has it), or `Select ref`. On a local or worktree draft a pick runs `git checkout` there right away, refused while a thread in that directory is working or waiting. A branch checked out in the root or another worktree moves the draft there, and from a worktree the default branch takes it back to the root.
   - **Model**: `Default` (no flag), then T3 Code's current Claude models by name, then a `Legacy models` heading over its legacy ones. orb passes the full model ID as `--model`, and a stored alias shows its model's name.
   - **Permission**: `Default` (no flag), then the six `--permission-mode` choices.
   - A project that isn't a git repository has no Workspace or Base branch row and starts local. There, `␣w`/`␣b` offer `Initialize Git` (`git init`).
4. Fields prefill from **that project's last-used workspace, model and permission** (saved when a draft starts). A brand-new project gets the last-used model + permission (from any project) and a local checkout. The base branch is never remembered: a new worktree defaults to the default branch, and a local checkout shows the root's current branch.
5. **Start** (`⏎`) → create the worktree if needed (fetching `origin/<base>`) → `claude --bg [--model m] [--permission-mode p]` (no `-n`; starts idle, no prompt). The draft becomes a thread. orb attaches only if the draft is still selected when the session comes up. A failed start keeps the draft, shows the reason, and removes any worktree orb made for it. The **first prompt is typed natively in Claude** (with Claude's own slash/skill/@-file pickers).
6. Drafts persist across orb restarts. `xx` discards a draft (no `claude rm`).

There is no orb-owned prompt composer (rejected: it would lose Claude's pickers/autocomplete).

### Sidebar & settle lifecycle (T3 semantics, MVP subset)

- Sections, in one list across projects: **Pinned** (newest pin first), **Active** (newest of created or un-settled first), **Settled** (collapsible shelf at the bottom, most recently settled first). Only the shelf has a visible header. A selected settled thread stays visible while the shelf is collapsed.
- Manual settle / un-settle / pin (`ss`, `p`). `ss` and `xx` act on the second key: the first shows jinn's yellow banner (" Press s again to settle "), or a red one when the action would be refused (" Can't settle while Claude is working ").
- A manual settle is refused while a turn is running or waiting on the user, and removes the pin. Pinning a settled thread un-settles it.
- Activity (a turn running, approval/input requested) **un-settles** automatically.
- Unpinned idle threads **auto-settle after 3 days**. "Idle" counts from the last turn end orb saw, else the thread's creation. **Pinned threads are exempt**, and so is the thread orb is attached to.
- A **manual un-settle** blocks auto-settle until the next real activity (T3's `settled_override = 'active'`).
- **Settling stops the Claude session** (`claude stop`), for manual and auto settles. `⏎` on a settled or stopped thread resumes it (`claude attach` resumes a stopped session).
- **Delete** (`xx`) hides the thread at once (M9), runs `claude rm` on any thread, running or not, then removes orb's row; the transcript stays. If `rm` fails, the thread reappears and the reason shows. A `✗` gone thread is removed without running `rm`.
- **Selection**: after a settle or a delete, the selection moves to the next thread below, else the one above (a settle with no card left selects the Settled header). Un-settle and pin keep the selection.
- "Completed-unseen" = latest turn ended after the user last selected the thread (selecting counts as viewing; being selected when the turn ends counts as seeing it).
- Status priority when several apply: approval > input > working > failed > completed-unseen > idle.

### Tool handoff (zellij)

| Key | Program |
|---|---|
| `␣t` | `$SHELL` (else `sh`) |
| `␣g` | `lazygit` |
| `␣v` | `nvim .` |

- The keys work in the sidebar and the preview while a thread or a draft is selected. They aren't bound with nothing selected, and they don't reach orb while attached.
- **Directory**: a thread's `cwd`. A draft's worktree, else the project root (local and new-worktree drafts), the same rule the branch picker uses for drafts. A directory that no longer exists shows `<~dir> doesn't exist` on the mode line.
- Opens a **full-screen floating** zellij pane (`new-pane --floating -x 0 -y 0 --width 100% --height 100%`) that closes when the tool exits (`--close-on-exit`), named `orb:<~dir>:<tool>` (e.g. `orb:~/dev/orb:lazygit`). The name is keyed on the directory, so two threads in one checkout share one pane per tool.
- The tool runs with orb's own `NO_COLOR` (`env -u NO_COLOR <tool>`, or `env NO_COLOR=<value>` when orb has one), not the zellij server's (research §13).
- **De-dupe**: before creating, `zellij action list-panes --json`. If a terminal pane with that name exists, orb runs `go-to-tab-by-id <tab>` then `focus-pane-id terminal_<id>` instead, which switches to its tab and shows it if it was hidden. Re-pressing `␣g` never creates a second lazygit (the problem the user hit with `cwt`). orb stores no pane ids.
- Outside zellij (no `ZELLIJ_SESSION_NAME` at startup) the mode line shows `Not running inside zellij`. A failed zellij call shows its reason there. Each zellij call gets 2 s: after that it's killed and the mode line shows `zellij timed out (session renamed? restart orb)`, since after a session rename orb's session name is stale and zellij never answers.

### Storage & paths (T3 layout)

- orb state: `~/.orb/userdata/state.sqlite` (SQLite, plain migrations — jinn's `jinn-session-schema` pattern; no event sourcing).
- Worktrees: `~/.orb/worktrees/<repo>/orb-<hex>` on branch `orb/<hex>`. After the first turn, the branch is renamed `orb/<slug>` from Claude's session title; the directory keeps its name (T3 does the same).

## Architecture

jinn's unidirectional loop:

```
crossterm → Keymap (scope-aware, which-key) → Intent
  → IntentHandler (sync, one match; mutates AppState; returns commands)
  → bus → kameo actors (async; each AppState field has ≤1 owning actor)
  → AppState → renderer reads on redraw
```

Crates (kept small on purpose):

```
orb/
├── AGENTS.md  .agents/RECORD.md  justfile  Cargo.toml (jinn lints)
├── crates/
│   ├── orb-term/    # leaf: PTY + alacritty_terminal + key encoder + ratatui renderer
│   ├── orb-domain/  # AppState, Intent, IntentHandler, actors, SessionHost, SQLite store, transcript parser
│   └── orb-tui/     # crossterm loop, keymap, which-key, render
└── src/main.rs
```

`SessionHost` is a trait (jinn rule: every external dependency behind a trait). First impl: Claude supervisor. Fallback impl if the supervisor ever fails us: orb-hosted `claude --session-id` PTYs.

Redraw is event-driven (PTY output / actor state changes wake the loop), unlike jinn's 100 ms tick + 50 ms screen task.

## Decisions & rejected alternatives

| Decision | Chosen | Rejected — why |
|---|---|---|
| Claude integration | Real `claude` TUI via `claude attach` in a PTY | Agent SDK / `claude -p` stream-json — `/status`, `/config` UI, `/permissions`, `/plugin`, `/resume` refused in headless; T3's exact problem. orb as a T3-server client — same SDK limitation. |
| Session hosting | Claude's background supervisor (`--bg`, `attach`, `agents --json`) | orb-hosted PTYs — quitting orb kills agents, unofficial state files. orb daemon + client (tmux model) — biggest build. Kept as `SessionHost` fallback. |
| State source | Poll `claude agents --json --all` (~1 s, ~160 ms/call) | Screen-scraping (claude-squad/ccmanager style) — breaks on TUI changes. Hooks — dropped in M9 (see Backlog). |
| Preview | Transcript JSONL → blocks | Attach on hover — `claude attach` takes ~200 ms to first byte; `j` spam would spawn processes. `claude logs` snapshot — not block-navigable. |
| Emulator | `alacritty_terminal` + own renderer | `vt100` + `tui-term` — no query replies, no DEC 2026, upstream abandoned; jinn needed workarounds. `wezterm-term` — not a stable crates.io lib. libghostty-vt — `!Send`, Zig FFI, too young. Image/graphics support not needed (Claude falls back when its graphics query goes unanswered). |
| Typing | Natively in attached Claude (Claude's own vim `editorMode`) | orb composer — loses Claude's `/`, `@`, skill pickers. |
| Leave-Claude key | `<C-\>` | `<C-\><C-n>` — awkward. Esc — Claude needs it. `Ctrl g` — zellij. Alt/Option — paneru. Cmd — zellij. |
| Worktrees | orb runs `git worktree add` (T3 layout) | Claude `-w` — no control of path/branch, reuse is awkward. jinn shell templates — config burden, loses T3 UX. |
| Drafts | Setup-only form; first prompt typed in Claude | Draft with prompt composer — loses Claude pickers; `--bg` without a prompt works (idle). |
| Right panels | zellij floating panes | orb-owned terminal drawer / diff / PR panel — redundant with zellij + lazygit; backlog if missed. |
| Persistence | SQLite + plain migrations | T3-style event sourcing — overkill. |
| Providers | Claude Code only | Codex etc. — later behind `SessionHost`. |
| T3 import | Projects only | Threads too — T3 threads carry T3's injected system prompt; ~5 unsettled threads not worth it. |
| Terminal identity under `claude attach` | Child env strips outer-terminal vars and sets `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=WezTerm`, `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` → Claude pushes kitty keys and syncs every frame | Legacy keys + `ESC CR` Shift+Enter — loses Esc/Alt disambiguation. Inherited env — caps depend on how orb was launched. `kitty`/`ghostty`/`iTerm.app`/`tmux` names — notification/graphics/wrapping side effects. |
| XTVERSION | Not answered | Raw-byte scanner — no observed effect under attach. |
| Mouse | Forward all child-requested mouse events (SGR) while attached; capture only while attached; forward OSC 52 | Wheel only — capture without clicks. None — no scrolling. |
| Key encoder | `terminput` + `terminput-crossterm`, with orb fixes (flag mapping, legacy Enter/Tab/Backspace under kitty, DECCKM arrows) | Porting alacritty's encoder (~420 lines); own encoder. |
| Event loop | std threads + one mpsc channel; drain-then-draw, no tick or throttle; attached input written straight to the PTY | tokio/kameo pane actor — per-key hop (a jinn lag source). |
| Actor plumbing | tokio + kameo 0.22, std-`RwLock` `State`, one `SessionsActor`; the frontend `tell`s commands straight to it (unbounded mailbox, `try_send`); the pane stays in the loop | Full jinn port (MessageBus + kanal Bridge + tcaps + `ActorDeps` + root supervisor) — hundreds of lines for one actor. No actors — conflicts with AGENTS.md §3. Add the bus when a second actor needs broadcast events. |
| Poll cadence | 1 s while a thread is busy/waiting or orb is attached, else 5 s; immediate poll after create/attach/detach | Fixed 1 s — ~120 ms CPU per call ≈ 12% of a core all day. Fixed 2 s — elapsed would jump. |
| Elapsed source | orb stamps `turn_started_at` when a poll first sees the turn in progress; persisted so a relaunch mid-turn keeps it (T3 stamps `turn.startedAt` the same way) | Transcript prompt timestamp — pulls more of M3 forward. `~/.claude/jobs/<id>/timeline.jsonl` — unofficial supervisor internals. |
| Thread title | Transcript: latest `custom-title` (`/rename`, M3; its own column so a later `ai-title` can't override it) > latest `ai-title` > first real prompt (first line) > "New thread" (T3's rule) | `agents --json` `name` — stays the short id when the prompt is typed through `claude attach`. `-n <name>` — meaningless fixed names. |
| Store abstraction | `Store` wraps `rusqlite::Connection` directly; tests use `Connection::open_in_memory()` | A store trait — one implementation; in-memory SQLite is a cheap real test backend. |
| Markdown renderer | `tui-markdown` 0.3.9 — builds on ratatui-core 0.1 (ratatui 0.30's core, one ratatui-core in `cargo tree`); syntect highlighting | `ratatui-markdown` — every release requires ratatui `^0.29` (two ratatui type universes). Vendoring jinn's patched `ratatui-markdown` — a vendored crate in orb. `daat-locus-md` — unknown third-party 0.30 fork without jinn's fixes. |
| Preview view state | `AppState.preview` holds the cursor (`None` = following the tail), row offset, and open folds (written by the `IntentHandler` through plain domain functions) plus a layout snapshot (viewport rows, per-block heights) the frontend writes after each draw | Frontend-local `ListState`-style state with preview keys applied in orb-tui — a second bypass of the `IntentHandler` after the attached pane. Block-boundary scrolling — a block taller than the viewport couldn't be read. |
| Transcript branch rule | Newest uuid line → walk `parentUuid`; add lines sharing an included `message.id` and tool results paired by `tool_use_id`; missing parent → previous line; repeated uuid → first copy; `logicalParentUuid` at `compact_boundary` | Naive `parentUuid` walk — drops parallel tool calls (16/44 files). `last-prompt.leafUuid` — lags 1–9 lines in 6/44 files. |
| Compaction | Continue across `compact_boundary` via `logicalParentUuid` with a "Conversation compacted" divider (full history) | Stop at the boundary — the preview is where you read history; attached Claude already shows the compacted state. |
| Thinking | Hide empty thinking (1273/1367 are `""`); non-empty thinking is a folded block `za` opens | Show empty ones — nothing to expand. Hide all — loses real content. |
| Preview liveness | `PreviewActor` checks the selected transcript's size every 500 ms, reads only appended lines, wakes the loop only when blocks changed; caches the 4 most recent threads | The sessions poll cadence (1 s / 5 s) — needs actor-to-actor messaging (no bus yet). The `notify` crate — a new dependency, and FSEvents coalescing adds latency. |
| Yank | `Command::Yank(text)` → the frontend writes OSC 52 via `outer_terminal::copy_to_clipboard` (works through zellij, M1); a tool block copies its summary line plus output, others their raw text | `arboard` (jinn) — a new dependency. Output-only or input-only for tool blocks. |
| Sidebar layout | T3's flat list across projects; each thread is a 3-line entry (status + title + time · project + status word · branch + `✳`); no visible Pinned/Active labels (a pin glyph marks a pin), only a Settled shelf header | Project headers with threads grouped under them (the old mockup) — not T3. Pinned-first inside each project — drops the Pinned section. Visible section headers — T3 doesn't have them. |
| Sidebar look | snacks.nvim explorer style: each draft or thread a 3-line tree node (`├╴`/`└╴` guides), under an orange input box with an `s` shelf badge and a `shown/total` count; tokyonight-moon colours on `bg_dark`, Nerd Font status icons, the cursorline on the selected row's first line. Chosen from a prototype of three LazyVim-style variants (explorer / picker / buffer) on branch `orb/sidebar-redesign-lazyvim` | Picker — one-line rows drop the branch and cut titles short. Buffer — relative numbers and a lualine, noisier. T3's cards with the rounded outline — what orb drew before. |
| Picker look | LazyVim's vim.ui.select (snacks' `select` layout) for every picker: a small numbered float with no preview, key hints in the bottom border; tokyonight-moon on `bg_dark`. Chosen from a prototype of four LazyVim-style variants on branch `orb/picker-prototype-lazyvim` | Snacks default, ivy, telescope — each adds a preview the pickers don't need ("I just need selection"); may return if orb adds search. T3's command palette with footer chips — what orb drew before. |
| Sidebar ordering | Pinned by `pinned_at` desc; Active by `max(created_at, unsettled_at)` desc (T3's keyless rule); Settled by `settled_at` desc; ties go to the higher id | Most recent activity first — rows jump while a thread works, breaking `j`/`k` muscle memory. T3's `pin_order_key`/`active_order_key` — fractional keys written only by mouse drag. |
| Pinned + auto-settle | Pinned threads never auto-settle — a pin means "keep this in view" | T3's rule — its policy never checks `pinnedAt`, so it auto-settles and unpins pinned threads. |
| Last activity | The last turn end orb saw, else `created_at`; stamped when a poll sees a turn stop being in progress. A turn run entirely while orb is closed is missed; the next one orb sees un-settles the thread anyway | Transcript mtime — bookkeeping writes (titles, mode lines) would count as activity and could un-settle a thread. |
| Stop on settle | Manual and auto settles run `claude stop` on an idle session; `⏎` (`claude attach`) resumes a stopped one. Auto-settle skips the thread orb is attached to. Stops are awaited on the `SessionsActor` after its state write | Keeping sessions running — ~300–480 MB each. Resume on un-settle — attach already resumes. Spawned stop tasks — more plumbing for ~0.7 s. |
| Settle/delete confirm | which-key sequences `ss`/`xx`: while the first key is pending, the renderer draws jinn's banner instead of the popup — yellow when the action will run, red when the handler's own validator refuses it | An AppState prompt flag + `Intent::NoOp` for unmapped keys — new state for what which-key already tracks (accepted cost: the cancelling key is swallowed, so `x` then `j` doesn't move). A separate banner check — two rules that could drift. |
| Delete | `xx` runs `claude rm` on any thread, running or not, then deletes orb's row; the transcript stays. On failure the thread stays and the reason shows; a `✗` gone thread skips `rm` | Refusing while working — the confirm already guards it. Soft delete (`deleted_at`) — nothing would read it. Always removing the row — a live session would keep running unseen, because orb lists only sessions it started. |
| Branch source | The latest non-empty `gitBranch` on transcript `user` lines, found by the existing title scan and stored in `threads.branch` | Reading `.git/HEAD` each poll — worktree/detached-HEAD parsing that M6 owns. |
| Visits | Selecting a thread stamps `last_visited_at` (`Command::Visit`), and each poll marks the selected thread seen; unseen = `last_activity_at > last_visited_at` | Stamping only on polls — `✓` would linger up to 5 s on a selected thread, and a short visit wouldn't count. |
| Picker port scope | The list picker only, in `orb-domain` (state, `feat/picker/`) and `orb-tui` (widget), no new crate; the item is an enum (`PickerItem::Project`/`Directory`); fuzzy scoring via `fuzzy-matcher` 0.3 (SkimMatcherV2, multi-term AND, ties by list order) | The whole jinn crate (tree picker, preview widget, `PickerOps`) as `orb-picker`. A generic `T: PickerItem` with `render_row` — orb-domain has no ratatui, and the renderer can match one enum. |
| Picker key routing | A plain `keymap::picker_route(key)` match under `Focus::Picker`, like the attached fast path | A which-key scope with `catch_all` — Space is the leader and could be swallowed, and the picker has no sequences and no popup. |
| Project order | T3's default `updated_at` rule: the newest `last_activity_at` among the project's threads, else the project's `created_at`, ties by title then id. orb has no user-message times, so it uses the last turn end it saw (else the thread's creation). No new column | A `last_used_at` column — more code, and not T3's rule. |
| Adding projects | Only from the `␣p` directory picker (T3's browse mode): prefilled `~/`, `Tab` opens, `⏎` adds without starting a session. No `orb [path]`, and orb doesn't register its launch directory | Registering the cwd on every launch — junk projects (`~`, wherever orb starts). `orb <path>` — a CLI surface nobody asked for once `␣p` exists. Exact-path entry only — tedious. T3's `⏎` opens / `⌘⏎` adds — `⌘` is zellij's. Add and start — `␣n` is the start path, and M7's drafts will own starting. |
| Directory filter | Fuzzy on the last part of the path, the same matcher as `␣n`; dot-directories hidden unless that part starts with `.` | T3's prefix-only rule — `front` wouldn't find `itemku-frontend-next-v2`. |
| Directory listing | The frontend loop runs `Command::ListDirectories(dir)` synchronously (as it does for `Yank` and `Attach`) and writes the names into `AppState.picker`, only when the directory part of the path changes | The `IntentHandler` reading the directory — it must not do I/O. A kameo actor — an async hop plus stale-reply handling for a ~1 ms `read_dir`. |
| T3 import method | A one-off SQL statement (`ATTACH` T3's DB read-only, `INSERT … ON CONFLICT (root) DO NOTHING`) run by hand in M5's manual check | Import code that runs on every launch or once behind a flag — it's seeding, not a feature. |
| Project removal | Deferred to the backlog project filter (T3's project-scope modal): `<C-x>` with a `No`/`Yes` confirm picker, soft remove (threads stay). Built in M9 (see Project filter) | In M5's `␣n` picker — the user wants removal in the filter modal. Hard delete — refused while any thread exists, so an old Settled shelf would block it. |
| Workspace trust | M5 shows Claude's "Workspace not trusted" refusal on the mode line. M6 adds an in-orb trust flow: on the refusal, orb opens an interactive `claude` in that directory in a pane, the user accepts Claude's own prompt and exits, and orb retries the start when that `claude` exits or the user presses `<C-\>`. The pane opens automatically on the refusal; there is no trust key. A retry refused again shows the error and doesn't reopen the pane. In practice only new projects need it: a worktree takes its trust from its main repo (research §11) | Writing `hasTrustDialogAccepted` into `~/.claude.json` — an undocumented format that every running Claude rewrites, and it silently skips a security prompt. Headless, as T3 does — rejected with the Agent SDK (see Claude integration). |
| Workspace change (M6, before drafts) | `␣w` on the selected thread, only before its first prompt: a pick starts a new `claude --bg` in the target, and only once that succeeds `claude rm`s the old prompt-less session; the thread keeps its row and sidebar place. After the first prompt the mode line shows `Workspace locked · Worktree`/`Local checkout` (T3's `canOverrideServerThreadEnvMode` exception) | A second picker step after `␣n`. Temporary keys M7 would delete. T3's strict lock ("session not stopped") — every orb thread has a live idle session, so `␣w` would always be locked. Moving a started thread (`--resume` in a new cwd) — unverified. |
| "No prompt yet" | The thread has no transcript file (a prompt-less session writes none); the actor re-polls before `claude rm` | The `agents --json` status — idle looks the same before and after a prompt. |
| Workspace rows | T3's: `Current checkout`/`Current worktree`, `New worktree`, `Previous worktree (<branch>)` when the project has a seed (the other thread with the latest activity whose cwd is a worktree other than this one) | Listing every worktree — not T3. |
| New worktree base | The default branch (`origin/HEAD`'s, else the root's current), fetched from `origin` and started from `origin/<b>`; the local branch when there's no `origin` or origin lacks it. A failed fetch fails the start with git's reason (T3) | Falling back to the last fetched ref with a warning — more code, and not T3. A base-branch choice — M7's draft form. |
| Branch switching | `␣b` any time, refused with a mode-line error while any thread whose cwd is the same directory is working or waiting; `⏎` runs `git checkout` (`--track` for a remote ref) | Only the selected thread's status — local-checkout threads share the root, so a checkout would change files under a sibling's turn. No guard, like T3. |
| Branch checked out in another worktree | After the first prompt the row is disabled: dimmed, `in <path>` on the row, skipped by selection. Before it, the pick moves the thread there, and the default branch from a worktree moves it back to the root (T3's `resolveBranchSelectionTarget`) | Refusing on pick — the reason should be visible up front. Always moving, like T3 — after the first prompt the transcript is tied to the cwd. |
| Branch list | T3's refs, dedupe, order and badges (research §11); fuzzy filter with the shared picker matcher; no paging | Local branches only. T3's substring match — the only picker that would work differently. T3's 100-ref pages. |
| Git plumbing | A sync `Git` trait (`GitCli` over `std::process::Command`) in `Services`; the `SessionsActor` runs worktree/fetch/checkout/rename inline, and the frontend loop lists refs synchronously like `ListDirectories` | An async trait like `SessionHost` — the frontend couldn't call it. The actor listing refs — an async hop plus stale-reply handling for a ~10 ms call. |
| Worktree cleanup | A failed start removes the worktree (`--force`) and branch (`-D`) orb just made. A move out of an orb worktree no thread uses removes it with a non-forced `git worktree remove` (the branch stays). `xx` leaves worktrees | Keeping failed-start worktrees — junk `orb/<hex>` branches. Forced removal after a move — could lose work. Removing on delete — "previous worktree" can still offer it. |
| Branch rename | On a poll that sees a turn end, only if the branch is exactly `orb/<hex>` of its `orb-<hex>` directory and the thread has an `ai-title` or `custom-title`: `git branch -m` to `orb/<slug>` (lowercase `[a-z0-9-]`, ≤40). A clash keeps the old name silently | Retrying every poll — a clash would run `git branch -m` every second. The first-prompt title — long, and T3 renames from a generated title. |
| Mode-line refusals | The `IntentHandler` writes `sessions.error` for the lock and busy refusals (a user-approved exception to "validation failure = no-op"); every error stays until the next key, and polls write it only when a save failed | A no-op — the user wants to know why. Clearing on the next poll — gone within 1 s while any turn runs. A notification system — YAGNI. |
| Draft storage (M7) | A `drafts` table keyed by `project_id`, so at most one per project; `Project.draft`; a `Draft` sidebar item and row | A `threads` row with `short_id` NULL — a table rebuild (the column is `NOT NULL UNIQUE`) and a "no session" guard in polling, attach, settle and delete. |
| Drafts per project | One. `␣n` on a project with a draft selects it | Unlimited — T3 keeps a second draft only when the first has prompt text, and orb drafts never hold a prompt. |
| Draft base branch | T3's per-workspace meaning. **New worktree**: the base is only recorded, then fetched and started from `origin/<b>` at Start (M6's rule, generalised to the picked branch). **Local or existing worktree**: `git checkout` there right away, refused while a thread in that directory is working or waiting. A branch checked out in the root or another worktree moves the draft there, and from a worktree the default branch takes it back to the root (T3's `resolveBranchSelectionTarget`) | A base only for new worktrees, with local read-only — the user chose T3's immediate checkout. Deferring the local checkout to Start — offered; the user chose T3. A read-only base on an existing-worktree draft (the pre-walk spec) — the user asked to copy T3. |
| Draft worktree creation | Lazily at Start; the mode line shows `starting session…` meanwhile | Eager on pick — the user prefers the delay at Start. |
| Draft `From <ref>` | A new-worktree draft's Base branch reads `From <ref>` (T3's label). The ref is worked out by Start's own start-point rule against origin's refs as of the last fetch, by the sessions actor when it creates, saves or restores the draft, so rendering does no git I/O. No branch reads `Select ref` | T3's "start from origin" toggle — orb always starts from origin when origin has the branch. T3's `From origin/<b>` for any local base — T3's server then quietly falls back to the local branch, so the label can be wrong. |
| Existing-worktree drafts | T3's: the workspace rows are `Current worktree` (selected; keeps it), `New worktree`, and the other previous worktree, with no root row; re-picking the draft's own workspace changes nothing; `␣b` lists the worktree's refs | `Current checkout` first, which moved the draft back to the root on an immediate `⏎` (the pre-walk spec). |
| Non-git projects | T3's: no Workspace or Base branch row; Start is always local; `␣w`/`␣b` open a one-row `Initialize Git` picker that runs a bare `git init` in the root; a failure shows `Git initialization failed: <reason>`. orb learns it from `git for-each-ref` failing when the draft is created, saved or restored | Offering `New worktree`, which failed only at Start, and a `␣b` that showed git's `fatal: not a git repository`. |
| Last-used | Columns on `projects` (`last_workspace`, `last_model`, `last_permission_mode`, `last_used_at`), written on a successful Start. The global fallback is the project with the newest `last_used_at`, and gives only model and permission. A remembered previous worktree with no seed falls back to Local. The base branch is never remembered | Deriving it from the newest thread — lost on delete, and NULL for pre-M7 threads. |
| Model list | `Default` (no flag), then T3 Code's current Claude models in T3's order and names (Claude Opus 5.5, Claude Fable 5.1, Claude Opus 5, Claude Sonnet 5), then a flat `Legacy models` heading over its seven legacy models (research §12). orb stores and passes the **full ID** (`claude-opus-5-5`) as `--model`. A stored alias (`opus`) shows T3's name for it and still passes as is. Any other stored value shows raw. The preview header names a transcript's model the same way | The `--help` aliases `opus`/`sonnet`/`fable`/`haiku` (the first spec) — the user didn't recognise them. T3's collapsible legacy row — the user kept the flat list. `[1m]` variants — unverified as `--model` values; `Default` covers the user's `opus[1m]` setting. |
| Permission list | `Default` (no flag) + the six `--permission-mode` choices from `claude --help` 2.1.283 | — |
| Draft keys | `⏎` starts; `␣w`/`␣b`/`␣m`/`␣a` open the field pickers, the same keys in the sidebar and the form (user-defined in M7's plan); `xx` discards | A field cursor with `j`/`k` and a Start row — dropped once `⏎` starts and each field has a leader key. |
| Keys by selection | The which-key scope is focus × selection (thread, draft, nothing): a key that does nothing for the selection isn't bound, so the popup doesn't list it (`␣m`/`␣a` only on a draft; `␣w`/`␣b` only on a thread or draft; `p`/`ss` not on a draft; no block keys on a draft's form). No new `Focus` variant: the form is what the preview side shows while a draft is selected | Binding every key everywhere as a no-op — the user saw `m model` and `a permission` on a thread. A `Focus::Draft` — with no field cursor it would duplicate every binding. |
| Draft form and card | Form: a header, then the field rows (label and value only; the value cut from the left when too wide), then `⏎ start`. Card: `✎` + badge + project, then `New thread`, then an empty line (T3's draft row) | Right-aligned `␣w`/`␣b`/`␣m`/`␣a` hints and a `workspace · branch` card line — the user removed both in the walk. |
| Draft Start | `claude --bg [--model m] [--permission-mode p]` with no `-n`. A local or worktree draft's thread takes its branch from git in that directory at Start. orb attaches only if the cursor is still on the draft when the session comes up. The actor sets `Sessions.attach`, and the frontend loop takes it and runs `Intent::Attach` if that thread is still selected and no picker or pane has focus | `-n <project>` — the thread-title decision rejected fixed names, and `-n` stays in Claude's prompt box. The draft's cached branch — stale after a checkout outside orb or a restart. Always attaching — steals focus after the user moved on. The actor writing `focus` — focus is frontend state. |
| Draft place in the sidebar | Drafts above Pinned, newest first (T3's `SidebarDraftBlock`) | Inside Active. |
| Model/permission on threads | Saved on the thread row too, so M6's `␣w` move of a prompt-less thread restarts with the same flags | Dropping them — a moved plan-mode thread would silently restart in default mode. |
| Tool handoff | `␣t` `$SHELL` (read at startup, else `sh`), `␣g` `lazygit`, `␣v` `nvim .`, on a thread or a draft (a draft's worktree, else the project root). Each is a full-screen floating pane (`-x 0 -y 0 --width 100% --height 100%`, `--close-on-exit`) named `orb:<~dir>:<tool>`; an existing pane of that name is focused with `go-to-tab-by-id <tab>` then `focus-pane-id terminal_<id>`. A sync `Zellij` trait (`ZellijCli` over `zellij action`, inheriting orb's environment) run by the frontend loop like `ListBranches`, passed to `Frontend::run` as `Option<ZellijService>`, built only when `ZELLIJ_SESSION_NAME` is set at startup. The tool gets orb's own `NO_COLOR`, and each zellij call is limited to 2 s | `␣e` for nvim — clashes with the backlog `␣e` sidebar toggle. `orb:<thread-id>:<tool>` — two threads on one checkout would get two lazygits, and `␣w` changes a prompt-less thread's `cwd`. Threads only — the draft rule already exists (branch picker). Plain `nvim` — the user chose `nvim .`. `new-pane` with no command for the shell — zellij ignores `--cwd` without one. The default floating size — a centered 50% pane. Tiled + `toggle-fullscreen` — the roadmap chose floating panes. `focus-pane-id` alone — it doesn't switch tabs. Held panes — they linger as "EXIT CODE … ENTER to re-run". Storing pane ids — the name already survives restarts. Letting `zellij action` fail outside zellij — it exits 0 there. The `SessionsActor` — a `␣g` would queue behind a `git fetch`. A new actor — a second writer of `sessions.error`. In `Services` — no actor uses zellij. `child_env` — it strips `ZELLIJ*`. The server's `NO_COLOR` — a server started from a `NO_COLOR=1` terminal turned lazygit monochrome. No time limit — against a renamed session zellij never exits, freezing orb (research §13). |
| Attached ↔ sidebar (M9) | `<C-h>` while attached focuses the sidebar and keeps the Claude pane drawn (`AppState.pane_shown`), and `<C-l>` goes back into it, just like between the sidebar and the preview. `<C-l>` re-enters only while the shown pane's thread is selected and can still attach; otherwise it focuses the preview. `<C-\>` still switches the right side to the preview. `<C-\>`, selecting another thread, or Claude exiting stops showing the pane. `<C-h>` costs Claude nothing: Claude's keybinding table says `ctrl+h` can't be rebound (it's Backspace in a legacy terminal), and orb tells the two apart through zellij (M1). In the trust pane `<C-h>` retries the start like `<C-\>` and lands on the sidebar | `<C-\>` always landing on the sidebar. `<C-\>` returning to wherever the user attached from. A new Ctrl key — Claude binds nearly all of them (`ctrl+]` is "open artifact"). Two keys (`<C-\>` then `<C-h>`) — what orb did before M9. |
| Sidebar toggle (M9) | `␣e` (LazyVim's explorer toggle) in all six sidebar and preview scopes. Hiding moves focus to the right-hand area, which takes the full width (a shown Claude pane is re-entered at full width). Showing focuses the sidebar. While it's hidden, `<C-h>` and resizing do nothing. The hidden state isn't persisted | `<C-h>` revealing a hidden sidebar — the user rejected it. A separate full-screen zoom of the attached pane — hiding the sidebar already gives it the full width. Persisting the hidden state — it's a transient zoom. |
| Sidebar resize (M9) | `<C-Right>` widens the focused side and `<C-Left>` narrows it (LazyVim's window width), so the direction flips between the sidebar and the preview. A step is 4 columns, and the sidebar stays within 24–80, starting at 32. The keys are routed by `keymap::layout_route` ahead of which-key, cancelling a pending sequence. The width persists in a single-row `ui` table (migration 6) with typed columns (`sidebar_width`, `project_filter`), and is clamped on restore. The user turns off macOS's "Move left/right a space" shortcuts, which take these keys | which-key bindings — `ratatui-which-key` 0.14 can't parse `<c-left>`/`<c-right>`. A key/value `ui` table (the approved plan's wording) — typed columns need no string parsing and let `project_filter` reference `projects(id)`. Terminal-width logic — ratatui's `Length` already handles a terminal narrower than the sidebar. |
| Mode line (M9) | Only the mode's name on the left (`NORMAL`, `DRAFT`, `ATTACHED`, `PICKER`). The right side keeps `starting session…`, errors and `N running` | The key hints (`⏎ attach · ␣ leader`, `⏎ start · ␣ leader`, `<C-\> back`) — the user doesn't need them. |
| Optimistic delete (M9) | `xx` hides the thread at once. The `IntentHandler` picks the neighbour, then adds the id to `Sessions.deleting`, which `sidebar()`, `N running` and the poll's busy check skip. The `SessionsActor` runs `claude rm` and removes the id on every path, in the same write that shows the result, so a failed `rm` brings the card back with the reason. A thread being deleted never notifies | Removing the thread from state in the handler — the actor's `delete` reads its status from state and would return without running `rm`. A flag on `Thread` — each poll rebuilds `Thread` from its row, wiping it. Waiting for `rm` (~0.7 s) — what orb did before M9. |
| Project filter (M9) | `␣f` in the sidebar: `All projects`, then the projects in `␣n` order. One project or All (T3's single-select project scope). It filters drafts, cards and the Settled shelf, with the project shown after the sidebar's `>` prompt, and persists in `ui.project_filter`, restored only for a shown, unremoved project. `N running`, the poll cadence and notifications still see every thread. `␣n` on a project outside the filter clears it. Removal follows the Project removal row: `<C-x>` → `No`/`Yes` sets `removed_at` and deletes the draft in one transaction, the threads stay, and a filter to the project resets to All. `␣p` on the same directory clears `removed_at` | `␣f` in the preview — the user: filtering there "doesn't make sense". Several projects at once — T3's scope is one project. Keeping a removed project's draft — the user chose to discard it. |
| Notifications (M9) | The `SessionsActor` queues a notice in `Sessions.notices` when a thread it has polled since launch finishes a turn, starts needing approval, or starts needing input. It never queues one on the first poll, for a failed thread, or for a thread being deleted. The frontend drains the queue every loop iteration. **Focus rule:** announce while orb's pane isn't focused. That means orb's last focus event was a focus-out, or, while orb seems focused, `zellij action list-clients` shows no client on orb's `terminal_<ZELLIJ_PANE_ID>` (zellij 0.45 sends no focus-out on a tab switch). A batch is dropped while orb is focused, and when zellij fails or times out. **Delivery:** `terminal-notifier` when it's on `PATH` at startup, titled `<project> · <thread title>` with the body `Finished`, `Needs approval` or `Needs input`. `-group orb-<thread>` makes a newer notice replace an older one. The `-execute` line runs `kitten @ focus-window` on the kitty window titled `<session> \|`, then zellij `go-to-tab-by-id` and `focus-pane-id` for orb's pane (research §14). Without terminal-notifier, or when it fails (it exits non-zero, e.g. not allowed to notify), orb sends the notice through `osascript display notification`, with the text as argv. Notifications are always on | OSC 99/9/777 from orb's pane — zellij 0.45 drops them. OSC 99 written to the zellij client's tty, and terminal-notifier's `-activate` — a click only raises kitty, not orb's tab or pane. alerter — click-stealing bugs, and it posts as Terminal. Hammerspoon — the user declined it. `osascript` alone — a click opens Script Editor. The attached-thread fallback (notify for every thread except the attached one) — it notifies while you're looking at orb. `!focused` alone — it misses zellij tab switches. The actor notifying directly — only the frontend knows the outer terminal's focus. Failed sessions — the user chose finished, approval and input. |

## Milestones

Each milestone is planned in a fresh session. Open questions listed per milestone are for that milestone's `/plan` to resolve.

### 0. Bootstrap
- Cargo workspace (`orb-term`, `orb-domain`, `orb-tui`, root bin `orb`), edition 2024.
- Copy from jinn: `[workspace.lints]` block (root `Cargo.toml`), `.cargo/config.toml` (`RSTEST_TIMEOUT = "10"`), `.debtmap.toml`, `justfile` recipes `check`/`test`/`lint`/`clippy`/`fmt`/`fmt-fix`/`lint-testattr`/`commit` (drop sandcastle/npm/fossil/plugin recipes).
- `AGENTS.md`: the generic-Rust parts of jinn's `AGENTS.md` (see "AGENTS.md port" below). Point it at `docs/roadmap.md` and `docs/research.md`.
- `.agents/RECORD.md`: jinn's header (format rules, templates, absence, editing) with no entries.
- Empty ratatui app that quits on `q`.

**AGENTS.md port** (from `~/dev/jinn/AGENTS.md`):
- Keep: §2 error handling (`wherror` + `error_stack::Report`, errors colocated), trait usage + service wrapper, module system, block scoping; §4 tests (one behavior per test, Given/When/Then, rstest, async tests); §5 docs; §8 misc.
- Keep, renamed jinn→orb: validator pattern, actor naming, `Services` DI, §3 architecture (data flow, command/event rules, one-writer-per-field, "sync sibling" anti-pattern), §6 modification guide.
- Adapt: §7 tooling table (git, no Fossil/sandcastle).
- Drop: plugin API policy, `vendor/` rule, TOML `DocumentPatcher` section (re-add when orb has user-editable TOML), sandcastle/fossil notes, jinn issue tracker/triage, `RecordingSink` path.

### 1. Terminal pane (`orb-term`) — riskiest first
- portable-pty + `alacritty_terminal`; renderer drawing the grid into a ratatui buffer (cell-by-cell like jinn's `terminal_tab.rs`).
- Key encoder: kitty keyboard protocol when the child enabled it, legacy otherwise; Shift+Enter, Shift+Tab (BackTab), bracketed paste, SGR mouse wheel, focus in/out (`CSI I`/`CSI O`).
- Answer startup queries (DA1, kitty `CSI ? u`, DECRQM 2026) via alacritty's `PtyWrite` events; XTVERSION is not answered (see Decisions).
- Resize propagation; event-driven redraw on PTY output.
- Scrub `CLAUDE*` env vars (list in research.md) before spawning.
- `<C-\>` intercepted before forwarding.
- Verify Ctrl+H ≠ Backspace through zellij (kitty keyboard protocol on in orb's outer terminal).
- Resolved in M1's plan: `alacritty_terminal` 0.26; `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` is set; the mouse is forwarded in full while attached (see Decisions).

### 2. Sessions & sidebar
- `SessionHost` trait + Claude-supervisor impl: create (`claude --bg -n …`), poll `agents --json --all`, attach, stop, rm.
- SQLite store at `~/.orb/userdata/state.sqlite` (projects, threads ↔ Claude short id + sessionId, title, cwd/worktree, branch, settle fields, last-visited).
- Sidebar project → thread with status icons + elapsed time.
- Attach in the right area; `<C-h>`/`<C-l>`; `<C-\>` flow; which-key.
- `␣n` starts a session in orb's cwd (temporary; M5/M7 replace it).
- Transcript locator + byte-offset tailer for thread titles (moved from M3).
- Resolved in M2's plan: status mapping — `busy` → `●` working; `waiting` on a permission prompt / sandbox request / worker request → `◐` approve; any other `waiting` → `?` input; `state: failed` → `✗`; `status` present otherwise → idle (blank); no `status` → `■` stopped; an orb thread missing from `agents --json` → `✗` gone. Elapsed source — orb stamps the turn start when a poll first sees it in progress, keeps it through `waiting`, clears it when the turn ends, and persists it. Poll cadence — 1 s while a thread is busy/waiting or orb is attached, else 5 s, plus an immediate poll after create/attach/detach (see Decisions).

### 3. Transcript preview
- Reuse M2's transcript locator and byte-offset tailer; extend it to every line type.
- Lenient decode: known line types, `#[serde(other)]` catch-all; bad line → warn + skip, never abort.
- Rebuild: start at the newest line with a uuid and walk back along `parentUuid` (rewinds create branches — file order ≠ conversation order); add every line sharing an included `message.id` and every `tool_result` paired by `tool_use_id` (parallel tool calls point at their own `tool_use`); at a `compact_boundary` continue via `logicalParentUuid`; a missing parent continues at the previous line; a repeated uuid keeps its first copy; hide bookkeeping lines.
- Render with `tui-markdown` 0.3.9 (crates.io; `ratatui-markdown` needs ratatui 0.29); virtualized rendering + per-block line cache keyed by width (jinn's `line_count_cache.rs`); block navigation, fold, yank, follow-tail.
- Parse only the selected thread; cache the 4 most recent; the `PreviewActor` checks the selected transcript every 500 ms.
- Tests: anonymized real transcript fixtures.
- Resolved in M3's plan: thinking — empty thinking is hidden, non-empty thinking is a folded block; compaction — the preview continues across `compact_boundary` with a "Conversation compacted" divider; yank — `y` sends the block's raw text through OSC 52; `/rename` titles — `custom-title` beats `ai-title`, stored in its own column; subagent sidechains stay in the backlog (see Decisions).

### 4. Settle lifecycle
- Pinned / Active / Settled; manual settle/un-settle/pin; activity un-settles; 3-day auto-settle; manual un-settle blocks auto-settle until activity; last-visited → completed-unseen.
- T3-style sidebar: one list of cards across projects (monogram badge, project, status or time, title, branch, `✳`) in T3's truecolor palette, a Settled shelf at the bottom, and scrolling. Delete (`claude rm`); settling stops the session (`claude stop`). Branch from the transcript's latest `gitBranch`.
- Resolved in M4's plan: keys — `p` pin/unpin, `ss` settle/un-settle, `xx` delete, and `⏎`/`l`/`h` on the Settled header (see Keys); Active order — `max(created_at, unsettled_at)` desc, T3's keyless rule, with no drag order keys (see Decisions).

### 5. Projects & picker
- Port the list picker from `jinn-selection-widget` + `<C-j>`/`<C-k>`; recency order → fuzzy score when typing.
- `␣n` opens the project picker and starts a local-checkout session (M7 replaces with drafts).
- `␣p` opens the directory picker and adds a project.
- Resolved in M5's plan: no `orb [path]` and no launch-directory registration — projects are added only from `␣p`; remove project moves to the backlog project filter; the T3 projects import is a one-off SQL statement run by hand in the manual check, not code (see Decisions).

### 6. Worktrees & branches
- `git worktree add -b orb/<hex> ~/.orb/worktrees/<repo>/orb-<hex> <base>` from an up-to-date origin base (T3's `startFromOrigin` default).
- Previous-worktree reuse (T3: most recently updated non-archived thread in the project with a different worktree).
- Branch switch: reuse a worktree already on that branch, else checkout in the thread's worktree; refused while the session is running.
- Rename `orb/<hex>` → `orb/<slug>` after the first turn using Claude's session title.
- In-orb trust flow (see Decisions: Workspace trust). Resolved in M5's manual check: trust is inherited from a parent directory, so trusting `~/.orb/worktrees` once covers every worktree (research §10). *Corrected in M6: that holds only for plain directories; a worktree takes its trust from its main repo (research §11).*
- Resolved in M6's plan: `␣w` (workspace, before the first prompt) and `␣b` (branch) in preview focus; branch switching is refused only while a thread in the same directory is working or waiting, and branches checked out elsewhere are disabled after the first prompt; the trust flow opens on Claude's refusal with no key, and worktrees need no trust of their own (research §11); a failed fetch fails the start (see Decisions).

### 7. Drafts
- `␣n` → project picker → `✎` draft → form (workspace, base branch, model, permission) with pickers; per-project last-used defaults + global fallback; Start → worktree → `claude --bg -n <name> [--model] [--permission-mode]` idle → attach; drafts persist.
- Resolved in M7's plan: keys — `⏎` starts a draft and `␣w`/`␣b`/`␣m`/`␣a` open its pickers, in the sidebar and the form. `␣w`/`␣b` also work on threads in the sidebar. Keys are bound only for a selection they act on (see Keys). Drafts: one per project, in a `drafts` table. The worktree is made only at Start. A local or worktree draft's branch pick checks out at once (T3). Last-used is saved per project, with a model + permission fallback from any project. Start passes no `-n`, and orb attaches only if the draft is still selected. The model list is T3's manifest with full IDs, not the `--help` aliases. Existing-worktree drafts and non-git projects follow T3 (`Current worktree`, `Initialize Git`). `--bg` honours `--model` and `--permission-mode` (research §12). See Decisions.

### 8. Tool handoff (zellij)
- `␣t` shell / `␣g` lazygit / `␣e` nvim as full-screen floating panes in the worktree, named `orb:<thread>:<tool>`, de-duped via `list-panes --json` + `focus-pane-id`.
- Resolved in M8's plan: full screen is a floating pane with `-x 0 -y 0 --width 100% --height 100%` (zellij's default is a centered 50% pane); `focus-pane-id` doesn't switch tabs, so orb runs `go-to-tab-by-id` first (research §13); nvim is `␣v`, running `nvim .`, which leaves `␣e` to the backlog sidebar toggle; panes are keyed on the directory (`orb:<~dir>:<tool>`), not the thread; drafts get the tools too, in their worktree or the project root (see Decisions).

### 9. Backlog I
- Sidebar `gg`/`G` and `<C-d>`/`<C-u>`; `␣e` hides or shows the sidebar; `<C-Left>`/`<C-Right>` resize it; `<C-h>` goes from the attached pane to the sidebar with the pane kept drawn, and `<C-l>` goes back; the mode line without key hints; `xx` hides a thread at once; the `␣f` project filter with `<C-x>` project removal; macOS notifications when a thread needs the user while orb's pane isn't focused.
- Resolved in M9's plan: of the backlog's 21 items, these seven were built, and the rest were deferred, dropped or left for the UI pass (see Backlog). Attached → sidebar is `<C-h>` and back is `<C-l>`, as between the sidebar and the preview, and `<C-\>` keeps its job. `␣e` works like LazyVim's explorer toggle: no auto-reveal, and the hidden state isn't persisted. Resize follows LazyVim (4 columns a step, 24–80). The width and the filter persist in a single-row `ui` table (migration 6). `␣f` is sidebar-only and filters to one project or All. Removal is soft: the draft is discarded, the threads stay, and `␣p` restores the project. Delete hides the thread through `Sessions.deleting`. Notifications cover finished, needs approval and needs input, not failed (see Decisions).
- Resolved in M9's research and walk (research §14): zellij 0.45 drops OSC 99, OSC 9 and OSC 777, and sends no focus-out to the tab the user leaves, so orb asks `zellij action list-clients` before dropping a notice while it seems focused. An `osascript` notification opens Script Editor on click, so orb posts through `terminal-notifier` when it's installed, with an `-execute` line back to orb's kitty window, zellij tab and pane, and through `osascript` when it isn't or when it fails. The user's kitty → zellij walk passed AC1–AC9. Three pre-check UI points were kept as they are: the `Remove project?` confirm doesn't name the project (UI pass); `<C-h>` in a trust pane left untrusted retries once and shows `Workspace not trusted`; `Yes` on the filtered project, with its draft selected, moves the cursor within the full list.

### Backlog
**Deferred** (the user doesn't need them for now, M9): subagent transcript expansion · PR status via `gh` + settle on merge · snooze · undo.

**Dropped in M9:**
- Codex behind `SessionHost` — orb supports Claude Code as its only provider (RECORD `(identity)`), and another provider is a milestone of its own.
- `:` command line — orb has none (RECORD `(keybinds)`), and there are no commands for it to run.
- Remappable keys — the only user already chooses every key; it would need a config format and string → `Intent` parsing.
- Claude hooks for instant state — the poll already runs every second while a thread is busy, hooks reach only sessions launched with `--settings`, and whether a `claude attach` resume keeps them is unverified.
- Copy mode over the attached pane — the pane keeps no scrollback and Claude redraws full-screen; the preview's `y` already yanks any block.
- Quick-reply box — `claude` has no send command, so it would be a hidden `claude attach` plus a timed paste that Claude's vim mode could eat.
- Drag reorder of pinned/active threads — orb captures the mouse only while attached, and a keyboard reorder would reopen "Sidebar ordering".

**UI pass** (after the last milestone): project favicons (`ratatui-image`; zellij 0.45 supports kitty graphics) · the Settled header sticks to the bottom of the sidebar while the list scrolls · the `Remove project?` confirm names the project · a `<C-x>` chip in the project filter's footer.

**Open ideas:**
- Rethink the preview: the user questions its value (2026-09-27); options: a passive glance with no focus, live panes attached after dwelling on a thread, or a summary view.

## Risks

- **Transcript format is undocumented** → lenient parsing + real-transcript fixtures; each line carries `version`.
- **Background supervisor is a research preview** → everything behind `SessionHost`.
- **Claude keys while attached**: `←` on an empty prompt opens Claude's agent view inside the pane; `Ctrl+Z` exits `attach` — orb treats the attach process exiting as "back to orb".
- **Workspace trust**: `claude --bg` refuses an untrusted directory (a new project, or `~/.orb/worktrees` before its first trust) → M6's in-orb trust flow; trust is inherited, so worktrees need it once.

## Acceptance criteria (MVP)

- `/status`, `/context`, `/mcp`, `/config`, permission dialogs, and Claude's vim mode all work when attached inside orb, inside zellij.
- Quitting and relaunching orb leaves sessions running with correct status.
- Every thread shows a live status icon and elapsed time without being attached.
- `j`/`k` through threads updates the preview instantly and spawns no `claude` processes.
- The preview renders transcripts as navigable blocks and shows new blocks as they're written.
- A new thread goes project picker → prefilled draft → Start → attached; it never skips the draft.
- Settle, un-settle, and pin work, and the automatic rules apply.
- Tool handoff never creates a duplicate zellij pane.

## Test cases

| # | M | Scenario | Expected |
|---|---|---|---|
| 1 | 0 | `just lint` on a fresh checkout | Passes; a bare `#[test]` fails `lint-testattr` |
| 2 | 1 | Shift+Enter while attached | Claude inserts a newline; doesn't submit |
| 3 | 1 | Paste multi-line text while attached | Arrives as one bracketed paste |
| 4 | 1 | Claude's startup queries | Answered; Claude starts without hanging |
| 5 | 1 | `<C-\>` while attached | orb Normal; Claude never receives it |
| 6 | 2 | `agents --json` shows `waiting` + `permission prompt` | Sidebar shows needs-approval |
| 7 | 2 | Quit orb mid-turn, relaunch | Thread still working; elapsed preserved |
| 8 | 2 | Hold `j` across 10 threads | Preview follows; zero `claude attach` spawned |
| 9 | 3 | Assistant lines sharing a `message.id` | One message block |
| 10 | 3 | Transcript with a rewind branch | Newest branch only |
| 11 | 3 | Unknown line type / malformed line | Skipped with warning; rest renders |
| 12 | 3 | Partial last line completed next poll | Parsed once; no duplicate |
| 13 | 4 | New prompt in a settled thread | Moves to Active |
| 14 | 4 | Manually un-settled, idle 3 days | Not auto-settled until activity |
| 15 | 5 | Picker, empty filter | Recency order; `<C-j>`/`<C-k>` move selection |
| 16 | 5 | Picker, typing | Fuzzy score over name + path; recency tie-break |
| 17 | 5 | T3 import SQL run twice (manual) | Projects appear once |
| 18 | 5 | `␣p`, `Tab` into `~/dev`, `⏎` on a repo | Added; first in `␣n` |
| 19 | 6 | New worktree thread | `~/.orb/worktrees/<repo>/orb-<hex>` on `orb/<hex>` from origin base |
| 20 | 6 | Branch switch while running | Refused with reason |
| 21 | 7 | Draft in a used project | Prefilled with that project's last workspace, model and permission; a new worktree's base is the default branch |
| 22 | 7 | Draft in a new project | Last model + permission from any project; local checkout, showing the root's current branch |
| 23 | 7 | Quit with a draft, relaunch | Draft restored with its settings |
| 24 | 8 | `␣g` twice on one thread | Second press focuses the existing pane |

## Record updates

At the end of milestone N, write the **MN** group into `.agents/RECORD.md` verbatim (after confirming each entry is true). Groups already written are marked *(written)*.

### M0 (written)

- (keybinds) Plain `q` quits orb.
- (keybinds) orb has no `:` command line.

### M1 (written)

- Amends M0: ``(keybinds) Plain `q` quits orb.`` → ``(keybinds) Plain `q` in Normal mode quits orb.``
- (pane) The terminal pane runs its child in a PTY (`portable-pty`) emulated by `alacritty_terminal` and drawn cell by cell into the ratatui buffer.
- (pane) orb redraws when input, PTY output, or child exit wakes the loop; there is no fixed tick or frame throttle.
- (pane) While attached, keys, paste, mouse, and focus events are encoded for the child's current terminal modes and written straight to the PTY, bypassing the `IntentHandler`.
- (pane) The pane's child runs with `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=WezTerm`, and `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1`, with Claude session variables and the outer terminal's identity variables removed.
- (pane) orb captures the mouse only while attached and forwards the child's OSC 52 clipboard writes to its outer terminal.
- (cli) `orb -- <cmd…>` sets the command the terminal pane runs.
- (keybinds) `⏎` in Normal mode attaches to the terminal pane; without a pane command it does nothing.
- (keybinds) While attached, every key goes to the child except `<C-\>`, which returns to Normal mode.

### M2 (written)

**Remove**
- ``(cli) `orb -- <cmd…>` sets the command the terminal pane runs.``
- ``(keybinds) `⏎` in Normal mode attaches to the terminal pane; without a pane command it does nothing.``

**Amend**
- ``(keybinds) Plain `q` in Normal mode quits orb.`` → ``(keybinds) Plain `q` in the sidebar quits orb.``
- ``(keybinds) While attached, every key goes to the child except `<C-\>`, which returns to Normal mode.`` → ``(keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the thread's preview.``
- ``(pane) orb redraws when input, PTY output, or child exit wakes the loop; there is no fixed tick or frame throttle.`` → ``(pane) orb redraws when input, PTY output, child exit, or an actor's state change wakes the loop, and once a second while a thread is working; there is no other tick or frame throttle.``

**Add**
- `(identity) orb supports Claude Code as its only provider.`
- ``(arch) User input flows through a `Keymap` that produces an `Intent`; the `IntentHandler` mutates `AppState` synchronously and returns commands.``
- ``(arch) Domain commands go to the `kameo` actor that owns them; pane commands are carried out by the frontend loop.``
- ``(sessions) Claude Code's background supervisor (`claude --bg`) hosts every session; quitting orb does not stop sessions.``
- ``(sessions) Session status is read by polling `claude agents --json --all` every second while a thread is busy or waiting or orb is attached, and every 5 seconds otherwise.``
- `(sessions) A thread's elapsed time counts from when orb first saw its turn running.`
- ``(sessions) A thread's title is its transcript's latest `ai-title`, else its first prompt, else "New thread".``
- `(sidebar) The sidebar lists only sessions orb started, grouped by project.`
- ``(pane) Attaching runs `claude attach <id>` in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area with the sidebar visible.``
- ``(keybinds) `<C-h>`/`<C-l>` move focus between sidebar and preview, `j`/`k` move between threads in the sidebar, `⏎` attaches, and `<Space>` is the leader with a which-key popup.``
- ``(keybinds) `␣n` starts a Claude session in orb's working directory.``
- ``(paths) orb persists its state to `~/.orb/userdata/state.sqlite`.``

### M3 (written)

**Amend**
- ``(sessions) A thread's title is its transcript's latest `ai-title`, else its first prompt, else "New thread".`` → ``(sessions) A thread's title is its transcript's latest `custom-title` (from `/rename`), else its latest `ai-title`, else its first prompt, else "New thread".``

**Add**
- `(preview) The preview renders the selected thread from its Claude transcript JSONL as navigable blocks, without spawning a process.`
- `(preview) The preview shows only the transcript's newest branch and continues across compaction boundaries.`
- `(preview) The preview reads new transcript lines within half a second and follows the tail while scrolled to the bottom.`
- ``(keybinds) In the preview, `j`/`k` move between blocks, `<C-d>`/`<C-u>` move half a page, `gg`/`G` jump to the top/bottom, `za`/`<Tab>` fold or unfold a block, and `y` yanks its raw text.``
- `(preview) Yanked text goes to the outer terminal's clipboard via OSC 52.`
- `(pane) orb draws the Claude pane only while attached; otherwise the right-hand area shows the selected thread's preview.`

### M4 (written)

**Amend**
- ``(sidebar) The sidebar lists only sessions orb started, grouped by project.`` → `(sidebar) The sidebar lists only sessions orb started, as one list across projects where each thread is a card showing its project, status, title, and branch.`

**Add**
- `(sidebar) Pinned threads come first, then Active threads, then a collapsible Settled shelf at the bottom of the sidebar.`
- `(sidebar) A thread shows ✓ Completed when its latest turn ended after the user last selected it.`
- `(settle) Any new turn, approval request, or input request un-settles a thread.`
- `(settle) An unpinned thread auto-settles after 3 days without turn activity, unless the user un-settled it since that activity or orb is attached to it.`
- ``(settle) Settling a thread stops its Claude session (`claude stop`); attaching resumes it.``
- ``(sessions) Deleting a thread runs `claude rm` and removes it from orb; its transcript stays in Claude's projects directory.``
- ``(keybinds) In the sidebar, `p` pins or unpins the selected thread, `ss` settles or un-settles it, and `xx` deletes it.``
- ``(keybinds) On the sidebar's Settled header, `⏎` opens or closes the shelf, `l` opens it, and `h` closes it; `h` on a settled thread closes the shelf.``

### M5 (written)

**Amend**
- ``(keybinds) `␣n` starts a Claude session in orb's working directory.`` → ``(keybinds) `␣n` opens the project picker; picking a project starts a Claude session in its directory.``

**Add**
- ``(picker) The picker is ported from jinn's `jinn-selection-widget` and ranks typed filter text by fuzzy score, breaking ties by list order.``
- `(projects) The project picker lists projects by their threads' latest activity, else when they were added, until filter text is typed.`
- `(projects) The project picker filters on each project's name and path.`
- ``(projects) Projects are added only from the `␣p` directory picker.``
- ``(keybinds) In a picker, typing filters, `<C-j>`/`<C-k>` or `↑`/`↓` move one item, `<C-d>`/`<C-u>` move half a page, `⏎` picks, and `Esc` cancels.``
- ``(keybinds) `␣p` opens a directory picker at `~/`; `Tab` opens the highlighted directory and `⏎` adds it as a project.``
- ``(tui) orb paints `#222436` under every cell that has no background of its own, including the attached pane's default-background cells.``
- `(tui) The mode line cuts a long status message at its end, so the mode's key hints and a 2-cell gap stay visible.`
- `(picker) The picker popup is only as tall as its rows, at most 90 columns wide, and keeps its top edge fixed while filtering.`

### M6 (written)

**Add**
- `(identity) **orb** is a terminal-based, vim-first manager for concurrent Claude Code sessions across projects and git worktrees, written in Rust (edition 2024).`
- ``(worktrees) New worktrees are created with `git worktree add` at `~/.orb/worktrees/<repo>/orb-<hex>` on branch `orb/<hex>`.``
- ``(worktrees) A new worktree starts from the project's default branch fetched from `origin`, or from the local branch when there is no `origin` or the branch isn't on it; a failed fetch fails the start.``
- `(worktrees) A session start that fails removes the worktree and branch orb created for it.`
- ``(worktrees) After a thread's turn ends, orb renames its `orb/<hex>` branch to `orb/<slug>` from Claude's title; the directory keeps its name.``
- `(worktrees) Deleting a thread leaves its worktree on disk.`
- `(worktrees) A thread's workspace can change only before its first prompt; orb then starts a new session in the new workspace and removes the old one.`
- ``(keybinds) `␣w` in the preview opens the workspace picker: current checkout or worktree, a new worktree, or the project's previous worktree.``
- ``(keybinds) `␣b` in the preview opens a branch picker of local branches and remote refs; `⏎` checks the branch out in the thread's directory.``
- `(branches) After a thread's first prompt, the branch picker disables branches checked out in another worktree and shows where.`
- `(branches) Switching branch is refused while any thread in the same directory is working or waiting.`
- ``(trust) When Claude refuses an untrusted directory, orb opens an interactive `claude` in the pane and retries the start when it exits or the user presses `<C-\>`.``

### M7 (written)

**Amend**
- ``(keybinds) `␣n` opens the project picker; picking a project starts a Claude session in its directory.`` → ``(keybinds) `␣n` opens the project picker; picking a project opens its draft, creating it if needed.``
- `(sidebar) The sidebar lists only sessions orb started, as one list across projects where each thread is a card showing its project, status, title, and branch.` → `(sidebar) The sidebar lists orb's drafts and the sessions orb started, as one list across projects where each thread is a card showing its project, status, title, and branch.`
- ``(worktrees) A new worktree starts from the project's default branch fetched from `origin`, or from the local branch when there is no `origin` or the branch isn't on it; a failed fetch fails the start.`` → ``(worktrees) A new worktree starts from its draft's base branch (the default branch for `␣w`) fetched from `origin`, or from the local branch when there is no `origin` or the branch isn't on it; a failed fetch fails the start.``
- `(worktrees) A thread's workspace can change only before its first prompt; orb then starts a new session in the new workspace and removes the old one.` → `(worktrees) A thread's workspace can change only before its first prompt; orb then starts a new session in the new workspace, with the same model and permission, and removes the old one.`
- ``(keybinds) `␣w` in the preview opens the workspace picker: current checkout or worktree, a new worktree, or the project's previous worktree.`` → ``(keybinds) `␣w` in the sidebar or preview opens the workspace picker: current checkout or worktree, a new worktree, or the project's previous worktree.``
- ``(keybinds) `␣b` in the preview opens a branch picker of local branches and remote refs; `⏎` checks the branch out in the thread's directory.`` → ``(keybinds) `␣b` in the sidebar or preview opens a branch picker of local branches and remote refs; `⏎` checks the branch out in the thread's directory.``

**Add**
- ``(drafts) A draft holds only session setup (project, workspace, base branch, model, permission); starting it launches an idle `claude --bg` session with those settings and attaches while the draft is still selected.``
- `(drafts) Each project has at most one draft; drafts persist to orb's store and sit above pinned threads in the sidebar.`
- `(drafts) Draft settings default to the project's last-used workspace, model and permission, falling back to the last-used model and permission from any project and a local checkout; a new worktree's base branch defaults to the project's default branch.`
- ``(drafts) A new-worktree draft creates its worktree only when started; its form shows the ref it will start from as `From <ref>`.``
- `(drafts) In a draft on the project's root or in an existing worktree, picking a branch checks it out there right away; a branch checked out in the root or another worktree moves the draft there instead, and in a worktree the default branch takes the draft back to the root.`
- ``(drafts) A draft of a project that isn't a git repository has no workspace or base branch, starts in the project's directory, and `␣w`/`␣b` offer to initialize git.``
- ``(drafts) The model picker lists `Default`, then T3 Code's current Claude models by name, then its legacy models under a `Legacy models` heading; orb passes the picked model's full ID to `--model`.``
- ``(keybinds) On a draft, in the sidebar or its form, `⏎` starts it and `␣w`/`␣b`/`␣m`/`␣a` pick its workspace, base branch, model, and permission.``
- ``(keybinds) Keys that do nothing for the selected row are not bound, so which-key doesn't list them: `␣m`/`␣a` only on a draft, `␣w`/`␣b` only on a thread or draft, and `p`/`ss` only on a thread.``
- ``(preview) The preview header names the thread's model as the model picker does (e.g. Claude Opus 5.5), else shows its ID without `claude-`.``

### M8 (written)

**Amend**
- ``(keybinds) Keys that do nothing for the selected row are not bound, so which-key doesn't list them: `␣m`/`␣a` only on a draft, `␣w`/`␣b` only on a thread or draft, and `p`/`ss` only on a thread.`` → ``(keybinds) Keys that do nothing for the selected row are not bound, so which-key doesn't list them: `␣m`/`␣a` only on a draft, `␣w`/`␣b`/`␣t`/`␣g`/`␣v` only on a thread or draft, and `p`/`ss` only on a thread.``

**Add**
- ``(keybinds) `␣t` opens a shell, `␣g` lazygit, and `␣v` `nvim .` in the selected thread's or draft's directory, in the sidebar or preview.``
- ``(zellij) Tool handoff opens each tool as a full-screen floating zellij pane named `orb:<directory>:<tool>`, which closes when the tool exits.``
- `(zellij) Tool handoff focuses an existing pane of the same name, switching to its tab, instead of opening a second one.`
- `(zellij) A draft's tools open in its worktree, or in the project root for a local or new-worktree draft.`
- ``(zellij) Tool panes run with orb's own `NO_COLOR`, not the zellij server's.``
- ``(zellij) A zellij call that runs longer than 2 s is killed, and the mode line shows `zellij timed out (session renamed? restart orb)`.``

### M9 (written)

**Amend**
- ``(keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the thread's preview.`` → ``(keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the thread's preview, and `<C-h>`, which focuses the sidebar and leaves the Claude pane shown, or does nothing while the sidebar is hidden.``
- ``(pane) orb draws the Claude pane only while attached; otherwise the right-hand area shows the selected thread's preview.`` → ``(pane) orb draws the Claude pane while attached and after `<C-h>` leaves it for the sidebar, until `<C-\>` is pressed, another thread is selected, or Claude exits; otherwise the right-hand area shows the selected thread's preview.``
- ``(pane) Attaching runs `claude attach <id>` in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area with the sidebar visible.`` → ``(pane) Attaching runs `claude attach <id>` in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area.``
- ``(keybinds) `<C-h>`/`<C-l>` move focus between sidebar and preview, `j`/`k` move between threads in the sidebar, `⏎` attaches, and `<Space>` is the leader with a which-key popup.`` → ``(keybinds) `<C-h>`/`<C-l>` move focus between the sidebar and the right-hand area (the preview, or the Claude pane while it's shown), `j`/`k` move between threads in the sidebar, `⏎` attaches, and `<Space>` is the leader with a which-key popup.``
- `(tui) The mode line cuts a long status message at its end, so the mode's key hints and a 2-cell gap stay visible.` → `(tui) The mode line shows only the mode's name on its left and cuts a long status message at its end, so the name and a 2-cell gap stay visible.`
- ``(trust) When Claude refuses an untrusted directory, orb opens an interactive `claude` in the pane and retries the start when it exits or the user presses `<C-\>`.`` → ``(trust) When Claude refuses an untrusted directory, orb opens an interactive `claude` in the pane and retries the start when it exits or the user presses `<C-\>` or `<C-h>`.``
- ``(zellij) A zellij call that runs longer than 2 s is killed, and the mode line shows `zellij timed out (session renamed? restart orb)`.`` → ``(zellij) A zellij call that runs longer than 2 s is killed; when it was opening a tool, the mode line then shows `zellij timed out (session renamed? restart orb)`.``

**Add**
- ``(keybinds) In the sidebar, `gg`/`G` jump to the first/last row and `<C-d>`/`<C-u>` move half its visible height.``
- ``(keybinds) `␣e` hides or shows the sidebar; while it's hidden the right-hand area takes the full width, and `<C-h>` and resizing do nothing.``
- ``(keybinds) In the sidebar or preview, `<C-Right>` widens the focused side and `<C-Left>` narrows it, 4 columns a step, with the sidebar kept between 24 and 80 columns.``
- `(sidebar) The sidebar's width and project filter persist across restarts.`
- ``(keybinds) `␣f` in the sidebar opens the project filter: `All projects`, then the projects in `␣n` order.``
- `(sidebar) While a project filter is set, the sidebar lists only that project's drafts and threads, under a header naming it.`
- ``(sidebar) Picking a project in `␣n` outside the project filter clears the filter.``
- ``(keybinds) `<C-x>` in the project filter removes the highlighted project after a `No`/`Yes` confirm.``
- ``(projects) A removed project is hidden from `␣n` and the project filter and loses its draft; its threads stay, and adding it again with `␣p` restores it.``
- ``(sessions) Deleting a thread hides it at once; if `claude rm` fails, it reappears and the mode line shows the reason.``
- `(notify) While orb's pane isn't focused, orb sends a macOS notification when a thread finishes a turn, needs approval, or needs input.`
- ``(notify) While its last focus event says it's focused, orb still notifies if `zellij action list-clients` shows no client on its pane, because zellij sends no focus-out on a tab switch.``
- ``(notify) Notifications are delivered through `terminal-notifier` when it's on `PATH` at startup, and through `osascript` otherwise, or when terminal-notifier fails.``
- ``(notify) Clicking a `terminal-notifier` notification focuses orb's zellij tab and pane, and brings orb's kitty window forward if kitty's remote control is on (`KITTY_LISTEN_ON`).``
