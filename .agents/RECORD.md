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

- (keybinds) Plain `q` in the sidebar or the dashboard quits orb.
- (keybinds) orb has no `:` command line.
- (pane) The terminal pane runs its child in a PTY (`portable-pty`) emulated by `alacritty_terminal` and drawn cell by cell into the ratatui buffer.
- (pane) orb redraws when input, PTY output, child exit, or an actor's state change wakes the loop, and every 100 ms while a thread is working (the spinner's frame); there is no other tick or frame throttle.
- (pane) While attached, keys, paste, focus events, and mouse events over the pane are encoded for the child's current terminal modes and written straight to the PTY, bypassing the `IntentHandler`.
- (pane) The pane's child runs with `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=WezTerm`, and `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1`, with Claude session variables and the outer terminal's identity variables removed.
- (pane) orb forwards the child's OSC 52 clipboard writes to its outer terminal.
- (keybinds) While attached, every key goes to Claude except `<C-\>`, which returns to the dashboard, `<C-h>`, which focuses the sidebar and leaves the Claude pane shown, or does nothing while the sidebar is hidden, `<C-b>`, which hides or shows the sidebar and keeps the keys in the pane, `<C-Right>`/`<C-Left>`, which resize the pane as on the dashboard, `<C-o>`/`<C-i>`, which move through the jump list, and `<C-Space>`, which opens the session picker.
- (keybinds) While attached, Claude's background-task shortcut works only as `Ctrl+X Ctrl+B`, because orb takes `<C-b>`.
- (identity) orb runs each thread in one of two harnesses, Claude Code or pi, fixed when its draft starts.
- (arch) User input flows through a `Keymap` that produces an `Intent`; the `IntentHandler` mutates `AppState` synchronously and returns commands.
- (arch) Domain commands go to the `kameo` actor that owns them; pane commands are carried out by the frontend loop.
- (sessions) Claude Code's background supervisor (`claude --bg`) hosts every Claude thread.
- (sessions) Thread status is polled every second while a thread is busy or waiting or orb is attached, and every 5 seconds otherwise; a Claude thread's comes from `claude agents --json --all`.
- (sessions) A thread's elapsed time counts from when orb first saw its turn running.
- (sessions) A Claude thread's title is the name given with `r` in the sidebar, else its transcript's latest `custom-title` (from `/rename`), else its latest `ai-title`, else its first prompt, else "New thread".
- (sidebar) The sidebar lists orb's drafts and the sessions orb started, as one list across projects where each thread outside a group is a three-line tree node showing its status icon, title, and time, then its project and status word, then its branch.
- (sidebar) The sidebar is drawn like LazyVim's snacks explorer in tokyonight-moon: an input box titled Sessions with an i badge lit while the sidebar search has the keys and a shown/total count of drafts and threads, and the selected row's first line highlighted.
- (pane) Attaching runs `claude attach <id>` for a Claude thread, or `zmx attach <id> pi --session-id <id>` for a pi thread, in a PTY emulated by `alacritty_terminal`, rendered in the right-hand area.
- (keybinds) `<C-h>`/`<C-l>` move focus between the sidebar and the right-hand area (the dashboard, or the Claude pane while it's shown), `j`/`k` move between threads in the sidebar, `⏎` attaches, and `<Space>` is the leader with a which-key popup.
- (keybinds) `␣n` opens the project picker; picking a project opens its draft, creating it if needed.
- (paths) orb persists its state to `~/.orb/userdata/state.sqlite`.
- (pane) The right-hand area shows the selected thread's Claude pane while orb is attached to that thread, except while the dashboard has the keys; otherwise it shows the dashboard.
- (sidebar) Pinned rows (lone threads and groups) come first, then active ones, then a collapsible Settled shelf at the bottom of the sidebar.
- (sidebar) A thread shows a green check and "done" when its latest turn ended after the user last selected it.
- (settle) Any new turn, approval request, or input request un-settles a lone thread, or the group of a thread in one.
- (settle) An unpinned lone thread or group auto-settles after 3 days without turn activity in any of its threads, unless the user un-settled it since that activity or orb is attached to one of them.
- (settle) Settling a thread stops its session (`claude stop`, or `zmx kill` for a pi thread); attaching resumes it.
- (sessions) Deleting a Claude thread runs `claude rm` and removes it from orb; its transcript stays in Claude's projects directory.
- (keybinds) In the sidebar, `p` pins or unpins and `s` settles or un-settles the selected lone thread or group, and `d` deletes a thread or group (on a draft, discards it); settling, deleting and discarding ask a `No`/`Yes` confirm first.
- (keybinds) On the sidebar's Settled header, `⏎` opens or closes the shelf, `l` opens it, and `h` closes it; `h` on a settled thread closes the shelf.
- (picker) The picker is ported from jinn's `jinn-selection-widget` and ranks typed filter text by fuzzy score, breaking ties by list order, except in the search picker, which asks the search index.
- (projects) The project picker lists projects by their threads' latest activity, else when they were added, until filter text is typed.
- (projects) The project picker filters on each project's name and path.
- (projects) Projects are added only from the `␣p` directory picker, except orb's `Research` and `Learn` projects, which the first `␣gr`/`␣gl` adds, and its `Incognito` project, which orb adds at start.
- (keybinds) In a picker, typing filters, `<C-j>`/`<C-k>` or `↑`/`↓` move one item, wrapping from the last to the first and back, `<C-d>`/`<C-u>` move half a page, stopping at the ends, `⏎` picks, and `Esc` cancels.
- (keybinds) `␣p` opens a directory picker at `~/`; `Tab` opens the highlighted directory and `⏎` adds it as a project.
- (tui) orb paints `#222436` under every cell that has no background of its own, including the attached pane's default-background cells.
- (tui) The mode line is drawn like LazyVim's lualine in tokyonight-moon: the mode in a block of its colour, the selected thread's or draft's branch and project (`<project>/<group>` in a group), and the latest error in red on the left; `N running`, `fetching origin/<base>…` while Start fetches, or `starting session…` with a spinner, the approval and input counts, the selected row's `at/shown` position, and the local time on the right.
- (tui) The mode line's running, approval and input counts include every thread, even ones the project filter hides.
- (tui) When the mode line is too narrow, its right side stays whole while it fits, and its left side is cut at its end.
- (picker) Every picker but the session, worktree and search pickers is drawn like LazyVim's vim.ui.select in tokyonight-moon: a rounded popup 44–72 columns wide (wider when its name needs it) with its name centred in the top border, a > prompt over an orange rule, numbered one-line rows, and the selected row filled; it is only as tall as its rows, at most 60% of the screen, and keeps its top edge fixed while filtering.
- (picker) A picker shows its keys dim in its bottom border: `⏎ add · Tab open · Esc close` when adding a project, `⏎ filter · <C-x> remove · Esc close` in the project filter, `⏎ confirm · Esc cancel` in the remove, settle, delete, discard, delete-worktree, trust and Initialize Git confirms, and `⏎ select · Esc close` otherwise; the session, worktree and search pickers show no keys.
- (identity) **orb** is a terminal-based, vim-first manager for concurrent Claude Code and pi sessions across projects and git worktrees, written in Rust (edition 2024).
- (worktrees) New worktrees are created with `git worktree add` at `~/.orb/worktrees/<repo>/orb-<hex>`, on branch `orb/<hex>`, or on the group's slug branch for a Feature group.
- (worktrees) A new worktree starts from its draft's base branch (the default branch for `␣w` and for a Feature group) fetched from `origin`, or from the local branch when there is no `origin` or the branch isn't on it; a failed fetch fails the start.
- (worktrees) Start's `git fetch` from `origin` is bounded by 15 s, after which git is killed and the start fails with `git fetch origin <base> timed out after 15 s`.
- (worktrees) A session start that fails removes the worktree and branch orb created for it.
- (worktrees) After a thread's turn ends, orb renames its `orb/<hex>` branch to `orb/<slug>` from the thread's title; the directory keeps its name.
- (worktrees) Deleting a thread, in a group or not, leaves its worktree on disk; only deleting a whole Feature group removes one.
- (worktrees) A thread's workspace can change only before its first prompt; orb then starts a new session in the new workspace, with the same model and permission, and removes the old one.
- (keybinds) `␣w` in the sidebar or dashboard opens the workspace picker: current checkout or worktree, a new worktree, or the project's previous worktree.
- (keybinds) `␣b` in the sidebar or dashboard opens a branch picker of local branches and remote refs; `⏎` checks the branch out in the thread's directory.
- (branches) After a thread's first prompt, the branch picker disables branches checked out in another worktree and shows where.
- (branches) Switching branch is refused while any thread in the same directory is working or waiting.
- (trust) When Claude refuses an untrusted directory, orb asks `Trust <path>?` in a `No`/`Yes` confirm with `No` selected, naming Claude's project path (the git root, the main repo for a worktree, or the folder itself outside git).
- (drafts) A draft holds only session setup (project, harness, workspace, base branch, model, permission); starting it launches an idle session in the draft's harness with those settings and attaches while the draft's thread is still selected; if a picker, the rename box, the search or a pane has the keys when it comes up, orb attaches once the keys are back in the sidebar or dashboard, leaving them there.
- (drafts) Each project has at most one draft of its own, and each group has at most one; drafts persist to orb's store and a project's draft sits above pinned threads in the sidebar.
- (drafts) Draft settings default to the project's last-used workspace, harness, model and permission, falling back to the last-used harness, model and permission from any project and a local checkout, and to Claude Code with `Default` model and permission when there is no last-used record or its harness is gone; a new worktree's base branch defaults to the project's default branch.
- (drafts) A new-worktree draft creates its worktree only when started; the dashboard's Branch item shows the ref it will start from as `From <ref>`.
- (drafts) In a draft on the project's root or in an existing worktree, picking a branch checks it out there right away; a branch checked out in the root or another worktree moves the draft there instead, and in a worktree the default branch takes the draft back to the root.
- (drafts) A draft of a project that isn't a git repository has no workspace or base branch, starts in the project's directory, and `␣w`/`␣b` offer to initialize git.
- (drafts) On a Claude draft, the model picker lists `Default`, then T3 Code's current Claude models by name, then its legacy models under a `Legacy models` heading; orb passes the picked model's full ID to `--model`.
- (keybinds) On a draft, `⏎` in the sidebar or on the dashboard's Start item starts it, and `␣h`/`␣w`/`␣b`/`␣m`/`␣a` in either pick its harness, workspace, base branch, model, and permission.
- (keybinds) Keys that do nothing for the selected row are not bound, so which-key doesn't list them: `␣h`/`␣m`/`␣a` and the dashboard's `h`/`m`/`a` only on a draft, a group's draft or a group's card, with `␣a` and `a` there only while the selection's harness has permission modes, which pi doesn't; `␣w` and the dashboard's `w` only on a lone thread or a project's draft, outside the Incognito project; `␣b` and the dashboard's `b` only there and on a started Feature group's card; `␣t`/`␣gg`/`␣v` and the dashboard's `t`/`g`/`v` only with a thread, draft or group selected; the dashboard's `o` only on a thread or draft; `p`/`s` in the sidebar only on a lone thread or a group's card; `r` only on a thread; and `n` only on a group's card or a thread in it.
- (keybinds) `␣t` opens a shell, `␣gg` lazygit, and `␣v` `nvim .` in the selected thread's, draft's or group's directory, in the sidebar or dashboard.
- (zellij) Tool handoff opens each tool as a full-screen floating zellij pane named `orb:<directory>:<tool>`, which closes when the tool exits.
- (zellij) Tool handoff focuses an existing pane of the same name, switching to its tab, instead of opening a second one.
- (zellij) A draft's tools open in its worktree, or in the project root for a local or new-worktree draft.
- (zellij) Tool panes run with orb's own `NO_COLOR`, not the zellij server's.
- (zellij) A zellij call that runs longer than 2 s is killed; when it was opening a tool, the mode line then shows `zellij timed out (session renamed? restart orb)`.
- (keybinds) In the sidebar, `gg`/`G` jump to the first/last row and `<C-d>`/`<C-u>` move half its visible height.
- (keybinds) In the sidebar, `j`/`k` wrap from the last row to the first and back.
- (keybinds) `␣e` hides or shows the sidebar; while it's hidden the right-hand area takes the full width, and `<C-h>` and resizing do nothing.
- (keybinds) In the sidebar, the dashboard or the attached pane, `<C-Right>` widens the focused side and `<C-Left>` narrows it, 4 columns a step, with the sidebar kept between 24 and 80 columns.
- (sidebar) The sidebar's width and project filter persist across restarts.
- (keybinds) `␣f` in the sidebar opens the project filter: `All projects`, then Research and Learn once `␣gr`/`␣gl` has added them, then Incognito, then the projects in `␣n` order.
- (sidebar) While a project filter is set, the sidebar lists only that project's drafts and threads, and its input box shows the project after the > prompt.
- (sidebar) Picking a project in `␣n` outside the project filter clears the filter.
- (keybinds) `<C-x>` in the project filter removes the highlighted project after a `No`/`Yes` confirm.
- (projects) A removed project is hidden from `␣n` and the project filter and loses its draft; its threads stay, and adding it again with `␣p` restores it.
- (sessions) Deleting a thread hides it at once; if `claude rm` fails, it reappears and the mode line shows the reason.
- (notify) While orb's pane isn't focused, orb sends a desktop notification on macOS and Linux when a thread finishes a turn, needs approval, or needs input.
- (notify) While its last focus event says it's focused, orb still notifies if `zellij action list-clients` shows no client on its pane, because zellij sends no focus-out on a tab switch.
- (notify) On macOS, notifications are delivered through `terminal-notifier` when it's on `PATH` at startup, and through `osascript` otherwise, or when terminal-notifier fails.
- (notify) Clicking a `terminal-notifier` notification focuses orb's zellij tab and pane, and brings orb's kitty window forward if kitty's remote control is on (`KITTY_LISTEN_ON`).
- (notify) On Linux, notifications are delivered over D-Bus to the freedesktop notification server, and nothing is shown when there is none.
- (notify) On Linux, approval and input notifications are sent at critical urgency and finished-turn notifications at normal urgency.
- (notify) On Linux, a later notification about the same thread replaces the earlier one.
- (notify) On Linux, clicking a notification focuses orb's niri window when niri is running, then orb's zellij tab and pane.
- (sessions) An `r` name is kept only in orb's store; Claude's own session name doesn't change.
- (sessions) A `/rename` to a name different from the thread's previous `custom-title` replaces its `r` name.
- (keybinds) In the sidebar, `r` opens a Rename Session box filled in with the thread's title; `⏎` saves, an empty `⏎` clears the `r` name, and `Esc` cancels.
- (keybinds) In the sidebar, `/` or `i` moves the keys to its input box; typing filters, `<C-j>`/`<C-k>` or `↓`/`↑` move between matches, wrapping from the last to the first and back, `⏎` clears the text and keeps the cursor on the match (or, with no match, acts as `Esc`), and `Esc` clears it and puts the cursor back where it was.
- (sidebar) While searching, the sidebar lists drafts and threads whose title fuzzy-matches the typed text, and every thread of a group whose name matches, in sidebar order and including settled ones, with matched characters highlighted.
- (sidebar) The input box shows the search text after the > prompt, after the filtered project if there is one.
- (sidebar) While the sidebar has the keys, a steady block cursor sits on the first cell of the selected row.
- (tui) The rename box, the sidebar search and the pickers show a steady bar cursor.
- (tui) The mode line shows `INSERT` while typing in the rename box or the sidebar search.
- (keybinds) The which-key popup is drawn like LazyVim's default which-key (helix) in tokyonight-moon: a rounded box in the bottom-right corner on the mode line, the pending keys in its top border (`␣` for Space), one `key ➜ icon desc` row per next key with groups as `+name`, in which-key's order (letters and digits before symbols, lowercase before its capital), and `esc close  ⌫ back` on its last row; rows that don't fit are cut off.
- (dashboard) While no Claude pane is shown, the right-hand area shows a LazyVim-style dashboard: a gradient ORB banner, a context line, a menu of the selection's actions, and a footer counting working threads, threads and projects.
- (dashboard) The dashboard's menu lists `o` Open session, `w` Workspace and `b` Branch on a lone thread, and only `o` on a thread in a group or in the Incognito project; `o` Start session, `w` and `b` (only in a git repository, and not on the Incognito draft), `h` Harness, `m` Model and `a` Permission with their current values on a project's draft; `o`, `h`, `m` and `a` on a group's draft; `b` Branch on a started Feature group's card, then `h`, `m` and `a` (the group's defaults) on any group's card; `a` only while the selection's harness has permission modes; `t` Shell, `g` Lazygit and `v` Neovim on any thread, draft or group; and `n` New session, `i` Incognito, `p` Add project, `f` Filter projects and `q` Quit always.
- (keybinds) On the dashboard, `j`/`k` or `↓`/`↑` move the menu cursor, wrapping from the last item to the first and back, `⏎` runs the highlighted item, and an item's letter runs it directly.
- (dashboard) The menu cursor starts on the first item (Open/Start session on a thread or draft, Branch on a started Feature group's card, Harness on any other card, else New session) and goes back there whenever the selection changes.
- (dashboard) While the Settled shelf's header is selected, the dashboard's context line shows the shelf hint.
- (dashboard) When the Claude pane fails to start, the reason shows in red under the dashboard's footer; other errors, such as a failed `claude --bg` or `claude agents`, show on the mode line.
- (dashboard) While the dashboard has the keys, a steady block cursor sits on the first cell of the highlighted item's label.
- (pane) orb keeps a separate attach pane (`claude attach`, or `zmx attach` for pi) for each attached thread, and selecting another thread leaves it running.
- (pane) orb is attached to a thread from `⏎` into its pane until `<C-\>`, settling or deleting the thread or its group detaches it, or its Claude exits.
- (keybinds) In the sidebar, `<C-\>` on an attached thread detaches it and keeps the keys in the sidebar.
- (sidebar) An idle thread orb is attached to shows a filled `FG` (`#c8d3f5`) circle in place of the hollow idle circle; with unseen output it still shows the green done check.
- (sidebar) While the list overflows the sidebar, the Settled header stays on its bottom row until scrolling brings it into view.
- (sidebar) While a search lists settled matches, the Settled header shows the open folder.
- (tui) Typed text too long for the sidebar search, a picker's input or the rename box shows its end, keeping the cursor in view.
- (groups) A group is a Feature, Research or Learn group whose threads all run in one directory: a Feature group's worktree, or `~/.orb/research/<name>/` or `~/.orb/learn/<name>/`.
- (keybinds) `␣gf` picks a project and a name and adds a Feature group; `␣gr`/`␣gl` take a name and add a Research/Learn group; each starts with a draft.
- (keybinds) The group name box is the rename box titled `New Feature group`, `New Research group` or `New Learn group`; after `⏎` it stays open until orb has made the group, and a refused name leaves it open with the reason on the mode line.
- (groups) A group name is refused when the project already has a group of that kind with its slug (`Group <slug> already exists`), when a Feature group's branch already exists (`branch <slug> already exists in <project>`), or when a Research/Learn folder already exists (`~/.orb/<kind>/<slug> already exists`).
- (groups) A group's slug is its name with each run of whitespace turned into `-`, case kept; a name using `/ \ ~ ^ : ? * [` or `..`, or starting with `-` or `.`, is refused with `Name can't use <char>`.
- (groups) A Feature group's worktree is created when its first draft starts, on a branch named for the group's slug, which orb never renames.
- (groups) Research and Learn groups belong to orb's `Research` and `Learn` projects, which `␣n` doesn't list.
- (groups) A new Research or Learn folder is copied from `~/.orb/templates/<kind>/`, which orb writes from its built-in default when it's missing.
- (sidebar) Making a group outside the project filter clears the filter; a refused name leaves it set.
- (sidebar) Outside the Settled shelf, a group is a three-line card (status, slug, time; kind, project, roll-up status; branch or folder, one icon per thread, fold chevron) with its threads as one-line rows under it, newest first.
- (sidebar) On the Settled shelf, a group is one line (kind icon, slug, thread count and time since it settled), with its threads under it while it's open.
- (keybinds) In the sidebar, `l` opens a group, `h` closes it from any of its rows and puts the cursor on its card, and `⏎` on a group's card toggles it; on a settled group `l` also opens the Settled shelf, and `h` on a closed one closes the shelf.
- (sidebar) Active groups start open and settled groups closed; fold state is kept in memory only.
- (keybinds) `n` on an unsettled group's card or a thread in it moves the cursor to the group's draft, creating it at the top of the group if there is none and opening the group if it's folded.
- (groups) `n` does nothing on a settled group.
- (keybinds) On a group's card, `␣h`/`␣m`/`␣a` pick the group's default harness, model and permission, leaving running threads alone.
- (zellij) A group card's and group draft's tools open in the group's directory, or in the project root before a Feature group's worktree exists.
- (groups) Settling a group is refused with `Can't settle while a session is working` while any of its threads has a turn underway, and it stops the group's idle sessions.
- (groups) Deleting a group runs `claude rm` for each of its threads, then deletes the group and its directory: its own folder under `~/.orb/<kind>/`, or its worktree (`git worktree remove --force`) and then the slug branch orb made for it (`git branch -d`).
- (groups) Deleting a Feature group whose slug branch has commits `git branch -d` calls unmerged (not in its upstream if it has one, else not in `HEAD`) is refused before anything is touched or hidden, with `branch <slug> has unmerged commits`; a branch the group was switched to with `␣b` is never deleted.
- (groups) Deleting a Feature group keeps its worktree and branch, showing `kept the worktree: another thread works in it`, while a thread outside the group still works in that worktree.
- (groups) If a thread's `claude rm` fails while its group is being deleted, that thread and the group stay, with the reason on the mode line.
- (picker) `d` on a group's card asks `Delete group and its worktree?` for a started Feature group, `Delete group and its folder?` for a Research or Learn group, and `Delete group?` for a Feature group that never started.
- (groups) A group always keeps a thread once it has one, and its draft before that; `d` on its last thread, or on the draft of a group with no thread, is refused with `Group needs at least one draft or thread`.
- (keybinds) `␣b` (and the dashboard's `b`) on a started Feature group's card switches its worktree's branch for the group and every thread in it; `␣b` isn't bound on a group's other rows, and `␣w` is bound on none.
- (groups) Groups and each thread's group persist to orb's store (store migration v8 added the `groups` table and `threads.group_id`).
- (keybinds) In the sidebar, the dashboard or the attached pane, `<C-o>`/`<C-i>` move back/forward through the jump list, as in neovim.
- (jumps) A jump is entering a thread's pane (`⏎`, `<C-l>`, a double-click on its row, a click into its pane, or a draft starting), `gg`/`G`, a search ended by `⏎` or a click, a `␣n` pick, or a session or search picker pick; it records the row it leaves and the row it lands on, except that entering the pane of the row the last `<C-o>`/`<C-i>` landed on records nothing.
- (jumps) A jump back or forward shows the target's pane only while orb is attached to it, with the keys in the pane only when pressed from one; it never attaches, starts a draft, or clears the project filter.
- (jumps) Deleted rows, rows hidden by the project filter, and the Settled header are skipped, and a folded group opens on arrival.
- (jumps) orb persists the newest 20 jump-list rows to its store (store migration v9 added the `jumps` table); a row already in the list moves to the newest slot.
- (groups) orb's built-in Research template is a research kit: an orchestrator `AGENTS.md`, investigator, falsifier and simulator subagents for Claude Code and for pi, conventions its agents follow, a report template and an empty `SOURCES.md`.
- (groups) The Research kit's pi subagents run through pi's `subagent` tool, from pi's example extension, which the user installs (filtering out pi-amplike's own `subagent` tool) and orb doesn't.
- (groups) The Research kit's pi subagents run on the thread's model and thinking level.
- (groups) A Research folder shares no files with other Research folders.
- (groups) orb's built-in Learn template is a learning kit: an orchestrator `AGENTS.md`, a saboteur subagent, the storyteller, challenge, tutor and conventions files under `.claude/learn/`, an epub ingest script and a `.gitignore` that keeps `source/` out of any repository.
- (groups) A Learn folder's threads share one campaign: each thread appends its event to `EVENTS.md`, writes learning records for every challenge attempted, and leaves syllabus status and notes for the next thread.
- (groups) A Learn folder's raids are planted by the `saboteur` subagent on a fresh `raid/R<n>` branch of the proving ground and recorded in `raids/R<n>.md`, which the orchestrating thread reads only after the learner declares a fix; on Claude Code it is the named agent, on pi a `subagent` task carrying the agent file.
- (trust) `Yes` on the trust confirm marks that path trusted in Claude's `.claude.json`, keeping every other key, and retries the start once; if that write fails, the start fails with `couldn't trust the folder`.
- (trust) `No` or `Esc` on the trust confirm, or `⏎` while its filter hides both rows, ends the start as a failed one, keeping the draft and showing `Workspace not trusted`.
- (incognito) orb's `Incognito` project runs its threads in `/tmp/orb-incognito/`, which orb creates at start and before each incognito start.
- (keybinds) `␣i` in the sidebar or dashboard, and `i` on the dashboard, open the Incognito draft, creating it from its default settings when it has none; `⏎` starts it.
- (incognito) `␣n` doesn't list orb's `Incognito` project.
- (incognito) Incognito threads settle, delete, rename and resume like any other thread.
- (keybinds) `␣␣` in the sidebar or dashboard, and `<C-Space>` in the attached pane, open the session picker.
- (picker) The session picker lists the threads inside the project filter, the selected thread included, newest chat first by the later of a thread's current turn start and its last turn end, leaving out threads being deleted, threads Claude no longer knows, and settled threads (a settled group's included) unless `<C-s>` shows them.
- (keybinds) In the session picker, `<C-s>` shows or hides settled threads and keeps the typed text; settled threads start hidden on every opening, and its other keys are every picker's.
- (picker) A session picker row is labelled `<project>/title`, or `<group>/title` for a thread in a group, with `New thread` as the title while it has none, and typed text fuzzy-matches the whole label.
- (picker) The session picker's rows and their order are fixed when it opens, while their status, spinner and time update live, and `⏎` on a thread deleted since, or one Claude no longer knows, only closes it.
- (picker) `⏎` in the session picker reveals the thread in the sidebar and attaches to it like `⏎` on its row.
- (picker) The session and worktree pickers are drawn like LazyVim's snacks picker in tokyonight-moon: a list box titled Sessions or Worktrees with `shown/total` on its input row (and a lit `s` in the session picker while settled threads show), beside a preview box, side by side from 120 columns and stacked below that, the list on top.
- (picker) The session picker's preview shows the selected thread's status, branch and model, then its latest exchanges from the transcript (the prompt, the tools the session ran, and the end of its last reply as Markdown), or `No transcript yet` when there is none, and refreshes while the transcript grows.
- (mouse) orb captures the mouse from startup until it exits.
- (mouse) A click on a sidebar row or the sidebar's input box moves the keys to the sidebar, and a click on the right-hand area moves them to the dashboard or into the Claude pane shown there; the click that moves them into the pane isn't forwarded to Claude.
- (mouse) A double-click is two clicks on the same sidebar or picker row within 500 ms.
- (mouse) In the sidebar, a click selects a row and a double-click acts as `⏎` on it.
- (mouse) The wheel over the sidebar moves the selection one row without wrapping while the sidebar has the keys, and otherwise, with no picker or rename box open, scrolls only its view, 3 lines a notch; the view goes back to the selection once the selection moves or the sidebar takes the keys.
- (mouse) Clicking the sidebar's input box starts a search; during a search, clicking a row ends it as `⏎` does on that row, and clicking the right-hand area ends it as `⏎` does and moves the keys there.
- (mouse) A click on a dashboard menu item moves the menu cursor to it without running it.
- (mouse) In a picker, a click selects a row, a double-click picks it, the wheel over its list moves the selection one row without wrapping (over the session, worktree or search picker's preview it does nothing), a click outside it cancels it like `Esc`, and a click on a heading or a disabled row does nothing.
- (mouse) A click outside the rename box cancels it like `Esc`.
- (mouse) A click on the input line of the sidebar search, a picker or the rename box moves its text cursor to the grapheme under it, to the first shown grapheme on the prompt, or to the end past the text.
- (worktrees) A worktree under `~/.orb/worktrees/` is pruned once nothing uses it, or once no draft uses it and every thread and Feature group in it has been settled, the latest for at least 7 days.
- (worktrees) Pruning skips a worktree with uncommitted changes or untracked files, and one whose thread is attached or has a turn underway.
- (worktrees) Pruning removes the worktree's directory and keeps its branch.
- (worktrees) orb sweeps for worktrees to prune at start and then every hour, showing `pruned N worktrees` on the mode line when it removes any.
- (worktrees) Attaching to a thread whose worktree is gone first recreates it at the same path on the thread's branch, or on a new branch of that name from the project's default branch when the branch is gone.
- (groups) Deleting a Feature group whose worktree was pruned deletes the group and its slug branch without the worktree removal.
- (keybinds) `␣sw` in the sidebar or dashboard opens the worktree picker, under the `+search` which-key group.
- (picker) The worktree picker lists every directory under `~/.orb/worktrees/<repo>/`, active ones first and unused ones last, most recently used first within each, each with its state: `active · <title>`, `settled <age>` or `no thread`.
- (picker) The worktree picker's preview shows the worktree's path, branch, size, uncommitted changes, last commit, last use, sweep verdict, and the threads, Feature groups and drafts that use it.
- (picker) The worktree picker's sweep verdict comes from the same rule the sweep applies.
- (keybinds) In the worktree picker, `⏎` does nothing and `<C-x>` deletes the selected worktree after a `No`/`Yes` confirm drawn over the list, which either answer returns to; it is refused, with the reason on the mode line, while a thread in it is attached or has a turn underway.
- (worktrees) Deleting a worktree from the worktree picker force-removes it, uncommitted changes included, and keeps its branch.
- (paths) orb persists its transcript search index to `~/.orb/userdata/search.sqlite`, which it rebuilds from the transcripts when the file is missing, can't be read, or holds another schema version.
- (search) The search index holds the prompts the user typed and Claude's text replies from every orb thread's transcripts, settled threads included; tool calls, tool output and thinking are left out.
- (search) At startup orb indexes, newest chat first and in the background, the lines each thread's current transcript gained since the search index last read it.
- (search) Before each search picker query, orb indexes the lines added to each transcript since it last read it, so results include replies written while the picker is open.
- (search) A thread's earlier transcripts stay indexed after `/clear` starts a new one, and a deleted thread's messages leave the index.
- (keybinds) `␣sg` in the sidebar or dashboard opens the search picker, labelled grep under the `+search` which-key group.
- (picker) The search picker matches typed text as case-insensitive substrings across all orb threads, ignoring the project filter, with every whitespace-separated term of at least 3 characters required.
- (picker) The search picker lists one row per matching message, newest first, at most 200, labelled `<project>/title` (or `<group>/title`) followed by a snippet of the message with its matches lit.
- (picker) The search picker is drawn like the session picker, with its list box titled Search, `indexing n/total` beside the title while transcripts are still being indexed, `200+` as its count when more than 200 messages matched, and `search unavailable: <error>` in place of the rows when the index can't be opened.
- (picker) The search picker's preview shows the thread's status, branch and model, then the exchange the matching message is in, naming the speaker only where it changes; the matching message is plain wrapped text with its matches lit and the first one in view, and the other messages render as Markdown.
- (picker) The search picker loads the first row's preview along with the results.
- (picker) `⏎` in the search picker reveals the thread in the sidebar and attaches to its current session, and on a thread deleted since, or one Claude no longer knows, it only closes the picker.
- (arch) Everything orb does differently per harness (hosting, transcripts, models, permission modes, trust) lives in that harness's own implementation of one shared interface, picked by the harness id each draft, group and thread stores.
- (sessions) A thread whose stored harness orb doesn't know shows as gone.
- (sessions) zmx hosts every pi thread, running `pi --session-id <id>` behind a socket under `~/.orb/pi/`.
- (sessions) A pi session starts on the first attach to its thread, with orb's pane as zmx's first client.
- (sessions) A pi thread's attach passes `--model` only until pi has written its session file.
- (sessions) Quitting orb does not stop Claude or pi sessions.
- (sessions) A pi thread's status is read from the tail of its pi session file and whether its zmx socket answers, on the same schedule as Claude's poll.
- (sessions) A pi thread counts as working only while its session file's last conversation entry is mid-turn and newer than its zmx socket.
- (sessions) A pi thread is only ever working, idle or stopped, because pi never waits for approval or input and writes its session file on the first prompt.
- (sessions) A pi thread's title is the name given with `r`, else its latest `/name`, else its first prompt, else "New thread".
- (sessions) A pi thread's branch is read from git in its directory after each turn.
- (sessions) Deleting a pi thread ends its zmx session and removes its socket; its session file stays in pi's sessions directory.
- (pane) An attach pane whose program exits within about a second of starting shows `session exited at start`.
- (drafts) On a pi draft, the model picker lists `Default`, then the models `pi --list-models` printed at orb's start under a heading per provider, labelled `provider/model`, which orb passes to `--model`.
- (drafts) When `pi --list-models` fails at orb's start, the mode line shows its reason once and pi's model picker lists only `Default`.
- (drafts) Picking another harness resets a draft's model and permission to `Default`.
- (drafts) The harness picker lists Claude Code and pi, each disabled as `checking` until orb's startup probe of it answers, and pi disabled as `pi not found` or `zmx not found` when either was missing from `PATH` at orb's start.
- (keybinds) On a group's draft, `⏎` starts it, and `␣h`/`␣m`/`␣a` override the group's default for that draft alone; it has no workspace or base-branch pick.
- (keybinds) `d` on a group's draft discards it after the `Discard draft?` confirm once the group has a thread, and does nothing while a session is starting.
- (groups) A group's draft follows the group's current default harness, model and permission, except for each one picked on the draft itself.
- (groups) A setting picked on a group's draft stays its own even when it matches the card's, `Default` included, and picking a different harness there also sets its model and permission to `Default`.
- (groups) Changing a group card's harness drops the model and permission picked on a draft that follows the card's harness.
- (groups) A group's draft starts in the group's directory once it has one, a Feature group's on the branch checked out there, and starting it removes the draft.
- (groups) A new group's default harness, model and permission come from the project's last-used record, else the latest from any project, else Claude Code with `Default` model and permission.
- (trust) orb's trust confirm applies only to Claude threads; it never asks for a pi thread.
- (sidebar) A lone pi thread's node shows a dim `pi` before its status word on its second line, or the tag alone while it's idle; a thread inside a group shows no tag.
- (sidebar) A lone thread's node ends its third line with its harness's icon: `✳` for Claude, nothing for pi.
- (picker) The session and search pickers' previews head each reply with its harness's icon and name, `✳ Claude Code` for Claude and `π pi` for pi.
- (search) `␣sg` indexes pi threads' session files alongside Claude transcripts.
