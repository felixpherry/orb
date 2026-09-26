# The Record

A curated list of factual, scoped statements asserting the application's **current** state. Authoritative for the present, never the future.

The planner consults this file before proposing a plan. If a feature **contradicts** an entry here, the contradiction is surfaced before the plan proceeds. If a feature **establishes a new high-level fact**, a verbatim entry is proposed for human approval as part of the plan.

## Format Rules

- **Factual.** Assert how things are _now_. Never future intent ("we will...", "should..."). Each entry is the current state of the application.
- **Scoped.** Name what each entry applies to — repo, app, frontend, or a named subsystem. An unscoped fact (e.g. "uses Fossil") is ambiguous: is that the repo, or the app's supported VCS list? Always disambiguate.
- **High-level.** One-liners (a few sentences at most). Capture decisions and facts a planner needs, not implementation minutiae.
- **Single tag.** Each entry carries exactly one subsystem tag as a `(tag)` prefix: `- (tools) The bash tool runs...`. One entry, one tag — this keeps tag usage a meaningful coverage metric (a tag growing large signals over-specification or a tag that should split). If you cannot decide between two tags for an entry, that is a signal to **re-evaluate the entry itself**, not to assign both. Use `(tag)` rather than `[tag]` to avoid colliding with markdown task-list (checkbox) syntax.
- **Singular concept.** Each entry should be a single sentence and only concerned with a single concept. Prefer multiple entries versus combining many things into one.

## Templates

| Pattern     | Form                                                             | Example                                                                                 |
| ----------- | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| State       | `[Scope] currently [does X / is Y].`                             | "The TUI's first screen at startup is the chat screen."                                 |
| Persistence | `[Scope] persists [what] to [where].`                            | "Sessions persist to SQLite."                                                           |
| Flow        | `[Input/event] is handled by [actor/subsystem], which [action].` | "File edits route through the `edit` tool, which requires a unique match or `replace_all`."        |
| Boundary    | `[Scope] is bounded by [constraint].`                            | "Project discovery walks ancestors until a VCS root or `$HOME`, whichever comes first." |

## Absence

A missing record, or an un-recorded area, simply means the list has no entry there yet. Absence is not a constraint — it is an open question, and a feature that fills a gap may establish the first entry for that area (proposed for human approval as part of the plan).

## Editing

Entries are added or amended **only with human approval**.

---

- (keybinds) Plain `q` in the sidebar quits orb.
- (keybinds) orb has no `:` command line.
- (pane) The terminal pane runs its child in a PTY (`portable-pty`) emulated by `alacritty_terminal` and drawn cell by cell into the ratatui buffer.
- (pane) orb redraws when input, PTY output, child exit, or an actor's state change wakes the loop, and once a second while a thread is working; there is no other tick or frame throttle.
- (pane) While attached, keys, paste, mouse, and focus events are encoded for the child's current terminal modes and written straight to the PTY, bypassing the `IntentHandler`.
- (pane) The pane's child runs with `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=WezTerm`, and `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1`, with Claude session variables and the outer terminal's identity variables removed.
- (pane) orb captures the mouse only while attached and forwards the child's OSC 52 clipboard writes to its outer terminal.
- (keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the thread's preview.
- (identity) orb supports Claude Code as its only provider.
- (arch) User input flows through a `Keymap` that produces an `Intent`; the `IntentHandler` mutates `AppState` synchronously and returns commands.
- (arch) Domain commands go to the `kameo` actor that owns them; pane commands are carried out by the frontend loop.
- (sessions) Claude Code's background supervisor (`claude --bg`) hosts every session; quitting orb does not stop sessions.
- (sessions) Session status is read by polling `claude agents --json --all` every second while a thread is busy or waiting or orb is attached, and every 5 seconds otherwise.
- (sessions) A thread's elapsed time counts from when orb first saw its turn running.
- (sessions) A thread's title is its transcript's latest `custom-title` (from `/rename`), else its latest `ai-title`, else its first prompt, else "New thread".
- (sidebar) The sidebar lists only sessions orb started, as one list across projects where each thread is a card showing its project, status, title, and branch.
- (pane) Attaching runs `claude attach <id>` in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area with the sidebar visible.
- (keybinds) `<C-h>`/`<C-l>` move focus between sidebar and preview, `j`/`k` move between threads in the sidebar, `⏎` attaches, and `<Space>` is the leader with a which-key popup.
- (keybinds) `␣n` opens the project picker; picking a project starts a Claude session in its directory.
- (paths) orb persists its state to `~/.orb/userdata/state.sqlite`.
- (preview) The preview renders the selected thread from its Claude transcript JSONL as navigable blocks, without spawning a process.
- (preview) The preview shows only the transcript's newest branch and continues across compaction boundaries.
- (preview) The preview reads new transcript lines within half a second and follows the tail while scrolled to the bottom.
- (keybinds) In the preview, `j`/`k` move between blocks, `<C-d>`/`<C-u>` move half a page, `gg`/`G` jump to the top/bottom, `za`/`<Tab>` fold or unfold a block, and `y` yanks its raw text.
- (preview) Yanked text goes to the outer terminal's clipboard via OSC 52.
- (pane) orb draws the Claude pane only while attached; otherwise the right-hand area shows the selected thread's preview.
- (sidebar) Pinned threads come first, then Active threads, then a collapsible Settled shelf at the bottom of the sidebar.
- (sidebar) A thread shows ✓ Completed when its latest turn ended after the user last selected it.
- (settle) Any new turn, approval request, or input request un-settles a thread.
- (settle) An unpinned thread auto-settles after 3 days without turn activity, unless the user un-settled it since that activity or orb is attached to it.
- (settle) Settling a thread stops its Claude session (`claude stop`); attaching resumes it.
- (sessions) Deleting a thread runs `claude rm` and removes it from orb; its transcript stays in Claude's projects directory.
- (keybinds) In the sidebar, `p` pins or unpins the selected thread, `ss` settles or un-settles it, and `xx` deletes it.
- (keybinds) On the sidebar's Settled header, `⏎` opens or closes the shelf, `l` opens it, and `h` closes it; `h` on a settled thread closes the shelf.
- (picker) The picker is ported from jinn's `jinn-selection-widget` and ranks typed filter text by fuzzy score, breaking ties by list order.
- (projects) The project picker lists projects by their threads' latest activity, else when they were added, until filter text is typed.
- (projects) The project picker filters on each project's name and path.
- (projects) Projects are added only from the `␣p` directory picker.
- (keybinds) In a picker, typing filters, `<C-j>`/`<C-k>` or `↑`/`↓` move one item, `<C-d>`/`<C-u>` move half a page, `⏎` picks, and `Esc` cancels.
- (keybinds) `␣p` opens a directory picker at `~/`; `Tab` opens the highlighted directory and `⏎` adds it as a project.
- (tui) orb paints `#222436` under every cell that has no background of its own, including the attached pane's default-background cells.
- (tui) The mode line cuts a long status message at its end, so the mode's key hints and a 2-cell gap stay visible.
- (picker) The picker popup is only as tall as its rows, at most 90 columns wide, and keeps its top edge fixed while filtering.
