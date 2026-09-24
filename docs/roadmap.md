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
- **From T3 Code (product/UX)**: sidebar (projects → threads, status, elapsed time), drafts that hold setup only, worktree layout, settle rules.
- **From jinn (`~/dev/jinn`)**: architecture and coding conventions, telescope-style picker (`jinn-selection-widget`), block-style chat view, keymap + which-key popup.
- **Tools** (shell, lazygit, nvim) open as zellij floating panes in the thread's worktree — replacing T3's integrated terminal and "Open in VS Code" button — with de-duplication.

Deliberately **not** built, because native Claude already does them inside the attached pane: approvals, plan mode, checkpoints/rewind (`/rewind`), in-thread model picker, context meter.

## User & environment (design constraints)

- Always runs inside **zellij 0.45** inside **kitty** on macOS. Never vanilla kitty.
- **Modifier layering is taken**: `Ctrl` → neovim, `Option` → paneru (window manager), `Cmd` → zellij (kitty maps `cmd+X` → `super+X`). orb must not rely on Alt/Option or Cmd.
- Zellij defaults to **locked** mode; `Ctrl g` toggles it. So `Ctrl g` is unavailable to orb.
- Leader is **`<Space>`**, always.
- Installed tools: nvim, lazygit, yazi, gh. Doesn't use VS Code.

## UX specification

### Layout

Sidebar on the left, preview (or attached Claude) on the right, mode line at the bottom.

```
┌─ orb ───────────────────────┬─────────────────────────────────────────┐
│ ▾ orb              ~/dev/orb│ Fix sidebar pills · orb/fix-pills · opus│
│ ▸ ● Fix sidebar pills  2m14s│┃ you     make the pills show elapsed time│
│   ◐ Refactor keymap  approve│  claude  I'll start with sidebar.rs…    │
│   ✓ Worktree reuse       new│  ▸ Read  crates/orb-tui/src/sidebar.rs  │
│   ✎ draft                   │  ▸ Edit  sidebar.rs  +12 −3             │
│ ▾ jinn            ~/dev/jinn│  claude  Done — pills tick every 1s.    │
│   ● Compaction fix       45s│                                         │
│ ─ Settled (127) ──────────  │                                         │
├─────────────────────────────┴─────────────────────────────────────────┤
│ NORMAL   ⏎ attach · ␣ leader                               3 running  │
└───────────────────────────────────────────────────────────────────────┘
```

Status icons: `●` working (with elapsed time) · `◐` needs approval · `?` needs input · `✓` completed-unseen · `✗` failed · `■` stopped · `✎` draft.

### Keys

| Focus | Keys |
|---|---|
| Sidebar | `j`/`k` next/prev thread — preview follows instantly · `<C-l>` focus preview · `⏎` attach · `␣n` new draft · `q` quit orb |
| Preview | `j`/`k` next/prev block · `C-d`/`C-u` half page · `gg`/`G` top/bottom · `y` yank block raw text · `za`/`<Tab>` fold tool output · `<C-h>` back to sidebar · `⏎` attach |
| Picker | typing filters · `<C-j>`/`<C-k>` next/prev item (focus stays in the filter input) · `⏎` pick · `Esc` cancel |
| Attached | **every** key → Claude, except `<C-\>` → back to orb |

- Keys not listed here are defined by the user in that milestone's `/plan`. Agents don't invent bindings.
- No `h`/`l` in the sidebar; no `i` binding. Window moves are `<C-h>`/`<C-l>`, like neovim.
- `<C-\>` (single key, configurable) was chosen over nvim's `<C-\><C-n>`. Esc must reach Claude (it interrupts turns / is vim-mode Esc).
- After `<C-\>`: the right side switches to the **same thread's transcript preview**, focus stays right. The attach process stays alive until another thread is selected, so `⏎` re-enters instantly.
- Leader chords show a which-key popup. `q` quits orb; sessions keep running. There is no `:` command line (backlog).

### Preview behavior

- Blocks: You · Claude text (markdown + code highlighting) · Thinking (collapsed) · Tool call (one-line summary + ✓/✗, output folded) · System (compaction, API errors, local command output).
- Tool one-liners: Bash → `$ <command>`; Edit → `Edit <file> +a −d`; Read/Write → path; Grep/Glob → pattern; others → name + first arg.
- Follows the tail when scrolled to the bottom.
- Granularity: Claude writes one JSONL line per finished content block, so text appears per block, not per token. Liveness comes from the sidebar status.

### Picker (ported from jinn's `jinn-selection-widget`)

- Empty filter → ordered by **recency** (most recently used first).
- Typing → ordered by **fuzzy score**, recency breaks ties.
- `<C-j>`/`<C-k>` move the selection (new; jinn doesn't have these).

### New thread flow (drafts)

1. `␣n` → project picker.
2. A `✎` draft appears in the sidebar; the right side shows the **draft form** (orb UI, not Claude).
3. Fields: workspace (local checkout / new worktree / previous worktree) · base branch · model · permission mode. `j`/`k` between fields; `⏎` on a field opens a picker.
4. Fields prefill from **that project's last-used settings**. A brand-new project gets the last-used model + permission (from any project), local checkout, and the default branch.
5. **Start** → create worktree if needed → `claude --bg -n <name> …` (starts idle, no prompt) → attach. The **first prompt is typed natively in Claude** (with Claude's own slash/skill/@-file pickers).
6. Drafts persist across orb restarts.

There is no orb-owned prompt composer (rejected: it would lose Claude's pickers/autocomplete).

### Sidebar & settle lifecycle (T3 semantics, MVP subset)

- Sections: **Pinned**, **Active**, **Settled** (collapsible shelf).
- Manual settle / un-settle / pin.
- Activity (new prompt, session starts running, approval/input requested) **un-settles** automatically.
- Idle threads **auto-settle after 3 days**.
- A **manual un-settle** blocks auto-settle until the next real activity (T3's `settled_override = 'active'`).
- "Completed-unseen" = latest turn completed after the user last viewed the thread (orb tracks last-visited per thread).
- Status priority when several apply: approval > input > working > failed > completed-unseen > idle.

### Tool handoff (zellij)

| Key | Program |
|---|---|
| `␣t` | shell |
| `␣g` | lazygit |
| `␣e` | nvim |

- Opens a **full-screen floating** zellij pane with `cwd` = thread's worktree, named `orb:<thread>:<tool>`.
- **De-dupe**: before creating, `zellij action list-panes --json`; if a pane with that name exists, `zellij action focus-pane-id <id>` instead. Re-pressing `␣g` never creates a second lazygit (the problem the user hit with `cwt`).

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
| State source | Poll `claude agents --json --all` (~1 s, ~160 ms/call) | Screen-scraping (claude-squad/ccmanager style) — breaks on TUI changes. Hooks — backlog, for instant updates. |
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
- A temporary way to start a session in cwd (key chosen by the user; replaced by M5/M7).
- Open questions: mapping `status`/`waitingFor`/`state` → icons (see research.md); per-turn elapsed source (status transition vs transcript last user timestamp); poll interval.

### 3. Transcript preview
- Locate `~/.claude/projects/*/<sessionId>.jsonl` once; store path.
- Incremental tail by byte offset; buffer partial trailing line.
- Lenient decode: known line types, `#[serde(other)]` catch-all; bad line → warn + skip, never abort.
- Rebuild: merge lines sharing `message.id`; pair `tool_result` ↔ `tool_use` by `tool_use_id`; follow `parentUuid` from the newest leaf (rewinds create branches — file order ≠ conversation order); hide bookkeeping lines.
- Render with `ratatui-markdown` (crates.io); virtualized rendering + per-block line cache keyed by width (jinn's `line_count_cache.rs`); block navigation, fold, yank, follow-tail.
- Parse only the selected thread; cache a few recent.
- Tests: anonymized real transcript fixtures.
- Open questions: thinking-block display (content may be redacted); subagent sidechains (backlog).

### 4. Settle lifecycle
- Pinned / Active / Settled; manual settle/un-settle/pin; activity un-settles; 3-day auto-settle; manual un-settle blocks auto-settle until activity; last-visited → completed-unseen.
- Open questions: keys for settle / un-settle / pin (user defines); ordering within Active (T3 uses an `unsettled_at` re-entry stamp + `active_order_key`).

### 5. Projects & picker
- Port `jinn-selection-widget` + `<C-j>`/`<C-k>`; recency order → fuzzy score when typing.
- `orb [path]` registers a project; remove project.
- One-time import of T3 projects from `~/.t3/userdata/state.sqlite` → `projection_projects` (title, workspace_root); idempotent.
- `␣n` opens the picker and starts a local-checkout session (M7 replaces with drafts).

### 6. Worktrees & branches
- `git worktree add -b orb/<hex> ~/.orb/worktrees/<repo>/orb-<hex> <base>` from an up-to-date origin base (T3's `startFromOrigin` default).
- Previous-worktree reuse (T3: most recently updated non-archived thread in the project with a different worktree).
- Branch switch: reuse a worktree already on that branch, else checkout in the thread's worktree; refused while the session is running.
- Rename `orb/<hex>` → `orb/<slug>` after the first turn using Claude's session title.
- Open questions: Claude's workspace-trust dialog per new worktree dir — does trusting `~/.orb/worktrees` cover children?; how branch switching is triggered (user defines keys; was `:branch <name>`)

### 7. Drafts
- `␣n` → project picker → `✎` draft → form (workspace, base branch, model, permission) with pickers; per-project last-used defaults + global fallback; Start → worktree → `claude --bg -n <name> [--model] [--permission-mode]` idle → attach; drafts persist.

### 8. Tool handoff (zellij)
- `␣t` shell / `␣g` lazygit / `␣e` nvim as full-screen floating panes in the worktree, named `orb:<thread>:<tool>`, de-duped via `list-panes --json` + `focus-pane-id`.
- Open questions: floating size flags for "full screen"; whether `focus-pane-id` switches tabs or needs `go-to-tab-by-id` first.

### Backlog
PR status via `gh` + settle on merge · snooze · undo · notifications · quick-reply box (hidden attach + paste) · copy mode over the attached pane · remappable keys · project favicons (`ratatui-image`, zellij 0.45 supports kitty graphics) · Claude hooks for instant state · subagent transcript expansion · full-screen zoom of the attached pane · Codex behind `SessionHost` · `:` command line.

## Risks

- **Transcript format is undocumented** → lenient parsing + real-transcript fixtures; each line carries `version`.
- **Background supervisor is a research preview** → everything behind `SessionHost`.
- **Claude keys while attached**: `←` on an empty prompt opens Claude's agent view inside the pane; `Ctrl+Z` exits `attach` — orb treats the attach process exiting as "back to orb".
- **Workspace-trust dialog** may appear per new worktree → surfaces as "needs input"; investigated in M6.

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
| 16 | 5 | Picker, typing | Fuzzy-score order; recency tie-break |
| 17 | 5 | T3 import twice | Projects appear once |
| 18 | 6 | New worktree thread | `~/.orb/worktrees/<repo>/orb-<hex>` on `orb/<hex>` from origin base |
| 19 | 6 | Branch switch while running | Refused with reason |
| 20 | 7 | Draft in a used project | Prefilled with that project's last settings |
| 21 | 7 | Draft in a new project | Last model + permission; local; default branch |
| 22 | 7 | Quit with a draft | Draft restored |
| 23 | 8 | `␣g` twice on one thread | Second press focuses the existing pane |

## Record updates

At the end of milestone N, write the **MN** group into `.agents/RECORD.md` verbatim (after confirming each entry is true). Groups already written are marked *(written)*.

### M0 (written)

- (keybinds) Plain `q` quits orb.
- (keybinds) orb has no `:` command line.

### M2

- (identity) orb supports Claude Code as its only provider.
- (arch) User input flows through a `Keymap` that produces an `Intent`; the `IntentHandler` mutates `AppState` synchronously and returns commands routed to `kameo` actors.
- (sessions) Claude Code's background supervisor (`claude --bg`) hosts every session; quitting orb does not stop sessions.
- (sessions) Session status is read by polling `claude agents --json --all`.
- (pane) Attaching runs `claude attach <id>` in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area with the sidebar visible.
- (keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the thread's preview.
- (keybinds) `<C-h>`/`<C-l>` move focus between sidebar and preview, `j`/`k` move within the focused area, `⏎` attaches, and `<Space>` is the leader.
  - *Confirm against the user's M2 key decisions before writing.*
- (paths) orb persists its state to `~/.orb/userdata/state.sqlite`.

### M3

- (preview) The preview renders the selected thread from its Claude transcript JSONL as navigable blocks, without spawning a process.

### M4

- (sidebar) Threads appear in Pinned, Active, and Settled sections; activity un-settles a thread and idle threads auto-settle after 3 days.

### M5

- (picker) The picker is ported from jinn's `jinn-selection-widget`; it lists by recency until filter text is typed, then by fuzzy score, and `<C-j>`/`<C-k>` move the selection.

### M6

- (identity) **orb** is a terminal-based, vim-first manager for concurrent Claude Code sessions across projects and git worktrees, written in Rust (edition 2024).
- (worktrees) New worktrees are created with `git worktree add` at `~/.orb/worktrees/<repo>/orb-<hex>` on branch `orb/<hex>`.

### M7

- (drafts) A draft holds only session setup (project, workspace, base branch, model, permission); starting it launches an idle `claude --bg` session and attaches.
- (drafts) Draft settings default to the project's last-used values, falling back to the last-used model and permission for new projects.

### M8

- (zellij) Tool handoff opens shell, lazygit, and nvim as named floating zellij panes (`orb:<thread>:<tool>`) in the thread's worktree, focusing an existing pane of the same name instead of creating a duplicate.
