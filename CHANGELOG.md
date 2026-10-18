# Changelog

All notable changes to orb are written here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and orb follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `<C-g> s` in a pane settles the session on screen, after the same confirm as the sidebar's `s`.
- The session, Colonists and search pickers keep their filter and typed text until orb quits.
- When typed text in the session picker matches only settled agents, the empty list says how many (`N settled matches · s to show`).
- A pane with no name and no agent title is called by the title its program sets (htop, vim, your shell's prompt) instead of `shell`.
- `<C-g> [` in a pane opens its history as plain text in a new tab running `$EDITOR`, else `less`, at the bottom in vi, vim, nvim and less; quitting the editor closes the tab and goes back to the tab it was opened from.
- `<C-g> y` in a pane lists the other agents of unsettled sessions; picking one pastes the pane's selection, else its screen, into that agent's prompt without sending it, and moves you there. An agent waiting for approval or an answer is refused.
- `<C-g> ?`, and `?` in the sidebar, list every key that works where you are, Alt and jump keys included; typing filters the list.
- `<C-g> a` takes you to the next agent that needs you: waiting for approval, then for input, then a finished chat you haven't read, longest waiting first. Press it again for the next one. It reaches agents the project filter hides, and says `No agent needs you` when nobody does.
- Agent pane borders show the agent's status icon while it works, waits, failed or has a finished turn you haven't read. Tabs show the icon of the agent that needs a look: waiting, failed or finished but unread, never the working spinner.
- The mode line counts the finished chats you haven't read, in green (`✓N`).
- While orb is focused, an agent in another tab or session that starts needing approval or input is named in the message box, such as `orb/fix-login needs approval`.
- Clicking a desktop notification opens that agent's pane with your keys in it, on macOS (with terminal-notifier) and on Linux.
- Agent rows in the sidebar show a time before the `✳` or `π` mark: the turn so far while working, the wait while waiting, else the time since the last turn.
- `orb integration install` also installs 17 workflow skills that take an approved plan to an open pull request.
- The plate: one list of what needs you across all of orb, grouped as Your move, Waiting on others, Parked and Done today, with a preview of each item and its links. orb adds its own row for each agent waiting for approval or an answer.
- `<C-g> o` opens the plate over the session on screen. Inside it, `j`/`k` move, `⏎` goes to the item's pane, `x` marks it done, `p` parks it or brings it back, `u` undoes, `z` shows Done today, and `gx` or `1`–`9` open its links: a web link (a full `https://…` address) or an `.html` file in the session's one terminal-browser tab, other files in `$EDITOR`, else `less`.
- `orb plate add|update|done|list` let agents add, change, close and list plate items. A colonist's add is refused with the new `colonist` code.
- The mode line reads `PLATE` while the plate has the keys, and counts its Your move items in orange.
- The orb skill teaches agents the plate. plan-with-me adds an item to approve the plan, and ship-plan one to review the PR. docs/keys.md has a line to add to your global agent instructions.

#### Control API

- `orb help` lists the nouns and common jobs; `orb <noun> --help` lists its verbs; `orb <noun> <verb> --help` shows the flags, their defaults, examples and the reply. Help needs no running orb and exits 0.
- A wrongly typed command names the bad word, suggests the closest match, and prints that command's one usage line, then exits 2.
- Every refusal carries a hint that names the command that fixes it.
- `orb` with no words inside an orb pane says which pane, tab and session you're in and where help is. Outside a pane it still starts orb.
- `orb project list` lists projects with their IDs, the ID `orb session create --project` takes.
- `orb pane type <pane> <text>` types text as keystrokes, with no paste and no Enter, so vim takes `:wq`.
- `orb pane keys` takes single characters, `space`, `delete`, `insert`, `f1`–`f12` and `alt+<char>`, and key names in any case.
- `orb tab create`, `pane split`, `pane stack` and `pane float` take `--name`, `--run` and `--cwd`, so one call makes a named pane running a command in a chosen folder.
- `orb agent start --split right|down` and `--tab` start the agent in a new pane or tab.
- `orb pane run --wait` prints the command's output and exit code. In fish it reads the shell's prompt marks; elsewhere it waits for the screen to go quiet. `--timeout`, `--tail` and `--json` go with it.
- `orb pane wait --text|--regex` returns once the text shows on the pane, or times out.
- `orb agent prompt --wait` prints the agent's answer.
- `orb pane read --head N` and `--tail N` read the first or last lines of the screen.
- `orb pane type --enter` presses Enter after the text.
- `--read` on `orb pane type` and `orb pane keys` waits until the screen changes and then stays the same for 300 ms (at most 3 s, or `--timeout`) and prints its last 20 lines (`--tail`), so one call types a line into python and shows the answer.
- `orb plate list --all` lists done items too.
- Every API command takes `--json`; on a command that already prints JSON it changes nothing.
- `orb agent reply --full` and `orb agent prompt --wait --full` print an answer of any length.
- The falsify skill argues against review findings before anyone fixes them. It tries to reproduce each one, weighs who hits it and how badly, and picks the cheapest answer: cut the feature, narrow it, fix it, leave it open in the PR, or dismiss it. It rejects defensive checks nothing shows are needed, and calls for reverting a fix that caused a new bug.

### Changed

- ship-plan runs falsify between review and fixes, and fixes only what it rules on. Its second round runs simplify and deslop on the first round's changes too.

- `⏎` in the search picker (`<C-g> /`) opens the found agent's own pane instead of its session, and resumes its chat in a new tab if it ended.
- `Alt x` and `<C-g> p x` ask before closing a pane that runs an agent; a shell pane still closes at once.
- A card's time counts how long its agent has waited for approval or input, instead of the time since its last turn. orb saves when each agent started waiting, so the times survive a restart; this upgrades orb's saved data, and orb keeps a copy of the old file first.
- The mode line's input count is magenta, as in the sidebar.
- pi's copy of orb's skills moves from `~/.pi/agent/skills` to `~/.agents/skills`; the install removes the old copy.
- Clearer which-key labels: `<C-g> e` toggle sidebar, `<C-g> h` go to sidebar, `<C-g> l` go to panes, `<C-g> t 1`–`9` tab 1–9, `<C-g> w` session's worktree, `<C-g> W` all worktrees.
- The start screen is the plate. The ORB banner and its counts are gone.
- A pane that can't start shows its reason in the message box instead of under the start screen.
- Plate items are saved, so they survive a restart. This upgrades orb's saved data, and orb keeps a copy of the old file first. Done items are cleared at the first start after midnight.

#### Control API

- `orb session list` shows open sessions and `orb agent list` live agents by default. Each reply says how many rows it hid and how to see them; `--all`, `--project`, `--session` and `--status` filter.
- Session and agent names in lists are cut to 60 graphemes, and agent list rows no longer carry the folder and branch. `orb agent get` still shows them in full.
- `orb pane read` and `orb agent reply` print plain text. Scripts that parse their JSON need `--json`.
- `orb agent reply` returns every answer since orb's last prompt, so a prompt sent mid-turn no longer hides the earlier answer.
- Right after `orb agent prompt`, `orb agent wait` waits for the turn to start and finish, and `orb agent get` reads `working`.
- `orb pane run` in a fish pane refuses with `busy` while a program runs there, naming it, unless `--force` is given. It clears text left on the prompt before typing.
- `orb pane close`, `tab close` and `session settle` reply with the panes and agents they ended.
- The settle refusal names the panes and agents still working.
- Unknown first words, such as `orb sessions`, and `orb --help` print help or a usage error instead of starting orb.
- The orb skill is a short set of rules and recipes that points to `orb help`, instead of a copy of every command.
- `orb plate list` lists only items that aren't done, and says how many done items it hid.
- `orb agent reply` and `orb agent prompt --wait` print at most an answer's last 12,000 characters, after a line saying how many were cut.
- The `busy` refusal's hint offers typing a line into the program (`orb pane type <pane> <text> --enter`) first, then quitting it, then `--force`. `orb pane run --help` warns that a program that never ends makes `--wait` run to its timeout.
- The orb skill teaches the REPL and vim recipes with `--enter --read`, what `not_run` means, `plate list --all`, `--json` everywhere and `--full`.
- The babysit and check-blast-radius skills name the unslop skill, if you have it, for the prose they write.

### Fixed

- On macOS, `orb agent wait` and `orb agent prompt --wait` stop once the caller hangs up, even when it hangs up right after asking. Before, such a wait ran to its timeout.

#### Control API

- A turn that ended in a failed model call refuses with `turn_failed` and the error text, for Claude and pi. Before, Claude's error read as its answer and pi's as no reply.
- `orb agent wait` on a failed agent ends with `turn_failed` instead of `no_agent`.
- Global search no longer indexes model-call error lines.
- `orb agent prompt --wait` catches an answer Claude writes just after its status goes idle.
- `orb pane run --wait` in a fresh fish pane, or one from before orb started, waits for the shell's first prompt marks instead of returning part of the output with `[exit ?]`.
- `orb pane run --wait` no longer says `not_run` when the shell was only slow to start the command; it says the command may still run.
- `orb agent prompt` reaches a Claude agent whose last turn failed, instead of saying there is no agent.
- `orb agent start --pane` refuses with `busy` while a program such as vim runs in the pane, unless `--force`.
- orb's log no longer writes agent names from a refused settle.
- `orb pane wait --text` in fish no longer matches the command line, only what the pane printed: the prompt marks show which lines are command lines. The shell's current prompt still matches, so `--text '~>'` waits for fish to be back. In other shells and in programs like python nothing is skipped, so wait for text only the output prints, or use `orb pane type --enter --read`.
- `orb pane run --wait` on a line fish took but didn't run, a rejected line it kept or an unfinished one it waits on, fails with `not_run` naming why. orb presses nothing; the hint says to clear the line with `orb pane keys <pane> ctrl+c`. A prompt still being drawn, or a shell never seen marking a command start, keeps "it may still run", and its hint says to read the pane and press ctrl+c only if the line still sits at the prompt.
- `orb agent wait` after a turn that failed on a model call fails with `turn_failed` and the error text, instead of `idle`.

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
