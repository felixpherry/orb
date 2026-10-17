# Changelog

All notable changes to orb are written here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and orb follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `<C-g> s` in a pane settles the session on screen, after the same confirm as the sidebar's `s`.
- The session, Colonists and search pickers keep their filter and typed text until orb quits.
- When typed text in the session picker matches only settled agents, the empty list says how many (`N settled matches · s to show`).
- A pane with no name and no agent title is called by the title its program sets (htop, vim, your shell's prompt) instead of `shell`.
- `<C-g> [` in a pane opens its history as plain text in a float running `$EDITOR`, else `less`, at the bottom in vi, vim, nvim and less; quitting the editor closes the float.
- `<C-g> y` in a pane lists the other agents of unsettled sessions; picking one pastes the pane's selection, else its screen, into that agent's prompt without sending it, and moves you there. An agent waiting for approval or an answer is refused.
- `<C-g> ?`, and `?` in the sidebar, list every key that works where you are, Alt and jump keys included; typing filters the list.
- `<C-g> a` takes you to the next agent that needs you: waiting for approval, then for input, then a finished chat you haven't read, longest waiting first. Press it again for the next one. It reaches agents the project filter hides, and says `No agent needs you` when nobody does.
- Agent pane borders show the agent's status icon while it works, waits, failed or has a finished turn you haven't read. Tabs show the icon of the agent that needs a look: waiting, failed or finished but unread, never the working spinner.
- The mode line counts the finished chats you haven't read, in green (`✓N`).
- While orb is focused, an agent in another tab or session that starts needing approval or input is named in the message box, such as `orb/fix-login needs approval`.
- Clicking a desktop notification opens that agent's pane with your keys in it, on macOS (with terminal-notifier) and on Linux.
- Agent rows in the sidebar show a time before the `✳` or `π` mark: the turn so far while working, the wait while waiting, else the time since the last turn.

### Changed

- `⏎` in the search picker (`<C-g> /`) opens the found agent's own pane instead of its session, and resumes its chat in a new tab if it ended.
- `Alt x` and `<C-g> p x` ask before closing a pane that runs an agent; a shell pane still closes at once.
- A card's time counts how long its agent has waited for approval or input, instead of the time since its last turn. orb saves when each agent started waiting, so the times survive a restart; this upgrades orb's saved data, and orb keeps a copy of the old file first.
- The mode line's input count is magenta, as in the sidebar.
- Clearer which-key labels: `<C-g> e` toggle sidebar, `<C-g> h` go to sidebar, `<C-g> l` go to panes, `<C-g> t 1`–`9` tab 1–9, `<C-g> w` session's worktree, `<C-g> W` all worktrees.

## [0.1.0] - 2026-10-16

The first release.

### Added

#### Sessions and worktrees

- Sessions across all your projects, each in the project's checkout or its own git worktree under `~/.orb/worktrees/`, made from a freshly fetched base branch.
- A new worktree's `orb/<hex>` branch is renamed after the agent's title once its first turn ends.
- The projects screen (`<C-g> P`) adds and removes projects; the workspace and branch pickers (`<C-g> w`, `<C-g> b`) move a session.
- Settling a session (`s`) stops its panes and moves it to the Settled shelf. `⏎` brings back its tabs and splits and resumes each agent with `claude --resume` or `pi --session-id`.
- Worktrees nothing uses, or whose sessions were all settled at least 3 days ago, are pruned at start and every hour. The worktree picker (`<C-g> W`) shows each worktree's size, changes and users, and deletes one.
- Incognito sessions (`<C-g> i`) in `/tmp/orb-incognito/` for a quick question outside any project.

#### Panes, tabs and layouts

- Tabs of split panes. Every pane runs under zmx, so quitting or updating orb leaves its programs running, and orb reattaches them at start.
- Alt keys move between panes and tabs, add and close panes, grow and shrink them, and switch the tab's layout (vertical, horizontal, stacked, half-stacked).
- `<C-g> p` splits, stacks, zooms and renames panes; `<C-g> t` manages tabs.
- Resize mode (`<C-g> r`) and move mode (`<C-g> m`).
- Floating panes per session (`Alt f`), with staggered, enlarged and spread layouts.
- 10 000 lines of history per pane, and dragging over text copies it to the clipboard.
- The mouse: click to focus panes, pick rows and switch tabs, and scroll history with the wheel.
- Images that a pane's program draws with kitty's graphics protocol show inside orb when orb runs in kitty.

#### Agents

- Claude Code and pi agents started in any pane show in the sidebar with their status: working, needs approval, needs input, idle or done.
- `orb integration install` sets up the Claude Code hook, the pi extension and the `orb` skill.
- A desktop notification when an agent finishes or needs you while orb isn't focused, on macOS and Linux.
- Sounds when an agent needs you and when one finishes.

#### Sidebar

- One card per session with a row per agent, each with its status, title and a mark for its tool (`✳` Claude Code, `π` pi).
- Pin, rename, delete, fold and search (`/`) sessions; show one project's sessions (`<C-g> f`); hide the sidebar (`<C-g> e`) or resize it.
- A jump list (`<C-[>`, `<C-]>`) that keeps the newest 20 places across restarts.

#### Pickers and search

- A session picker (`<C-g> Space`) with a preview of each chat and filters for blocked, working, idle and settled agents.
- Transcript search (`<C-g> /`) over every prompt and reply from Claude Code and pi, newest first; `⏎` jumps to the session.
- A which-key popup that lists the keys after `<C-g>`.

#### Control API and colonies

- `orb session`, `orb tab`, `orb pane` and `orb agent` commands let an agent list, create, split, focus, prompt, wait on and read panes and agents, with JSON replies.
- Worlds and colonies: `<C-g> c` starts a colony, a session for an orchestrator agent. Sessions it makes with `orb session create` become its colonists and fold under it in the sidebar.
- Built-in Research and Learn worlds, where each colony gets its own folder copied from a kit.

#### Look

- A LazyVim look in tokyonight-moon: an explorer-style sidebar, a lualine-style mode line, which-key and select pickers, a start screen, and a message box for errors and notices.

#### Install and packaging

- A one-command installer that puts orb and zmx in `~/.local/bin` after checking their checksums, sets up the integrations, and offers to install kitty and add orb's Cmd keys to kitty.conf.
- Prebuilt binaries for macOS and Linux, on aarch64 and x86_64, on every release.
- `orb --version`.
- orb backs up its saved data before upgrading it, and won't open data from a newer orb.
- A log in `~/.orb/orb.log`. A crash restores the terminal and leaves the panes running.

[0.1.0]: https://github.com/zqccer/orb/releases/tag/v0.1.0
