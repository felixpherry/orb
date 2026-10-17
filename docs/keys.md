# Keys and setup

## Setup

### Alt keys on macOS

orb's direct keys use Alt: h, j, k, l, the arrows, n, x, f, +, =, -, 1–9, [, ], i and o. Inside orb these go to orb, not to the pane's program.

- **Linux:** nothing to set up.
- **macOS, Option as Alt:** add `macos_option_as_alt yes` to kitty.conf.
- **macOS, Cmd as Alt:** the installer offers to add this block to kitty.conf. To add it yourself, paste it in and reload kitty (ctrl+cmd+,). kitty uses many of these Cmd keys itself, so the block hands them to orb.

```conf
map cmd+h send_key alt+h
map cmd+j send_key alt+j
map cmd+k send_key alt+k
map cmd+l send_key alt+l
map cmd+left send_key alt+left
map cmd+down send_key alt+down
map cmd+up send_key alt+up
map cmd+right send_key alt+right
map cmd+n send_key alt+n
map cmd+x send_key alt+x
map cmd+f send_key alt+f
map cmd+plus send_key alt+plus
map cmd+equal send_key alt+equal
map cmd+minus send_key alt+minus
map cmd+1 send_key alt+1
map cmd+2 send_key alt+2
map cmd+3 send_key alt+3
map cmd+4 send_key alt+4
map cmd+5 send_key alt+5
map cmd+6 send_key alt+6
map cmd+7 send_key alt+7
map cmd+8 send_key alt+8
map cmd+9 send_key alt+9
map cmd+[ send_key alt+[
map cmd+] send_key alt+]
map cmd+i send_key alt+i
map cmd+o send_key alt+o
```

Another terminal works if it speaks the kitty keyboard protocol and its font has Nerd Font symbols; kitty has both built in.

### Notifications and sounds

- **When orb isn't focused,** a desktop notification says an agent finished, needs approval or needs input. Clicking it brings orb forward and puts your keys in that agent's pane.
- **When orb is focused,** an agent outside the tab you're looking at (and its floats) that starts needing approval or input is named in the message box: `orb/fix-login needs approval` or `needs input`, with the agent's name after the session when the session runs two or more agents. A finished turn gets no message; its sound and the mode line's green `✓N` tell you.
- **macOS:** notifications go through [terminal-notifier](https://github.com/julienXX/terminal-notifier) when it's installed (allow it in System Settings → Notifications), else through `osascript`. For a click to bring kitty forward, turn on kitty's remote control in kitty.conf, then restart kitty (a reload doesn't apply these two):

  ```conf
  allow_remote_control socket-only
  listen_on unix:/tmp/kitty
  ```

  A click on a terminal-notifier notification also opens the agent's pane; an `osascript` one can't.
- **Linux:** notifications need a freedesktop notification server, which most desktops run. A click opens the agent's pane, and on niri it brings orb's window forward first.
- **Sounds:** a sound plays when an agent needs you, and when one finishes unless you're looking at it. macOS uses `afplay`; Linux the first of paplay, pw-play, ffplay, mpg123 or mpv it finds.

### Research and Learn kits under pi

The kits' subagents work under Claude Code as they are. Under pi they need pi's example subagent extension. For a pi installed with npm, these two lines install it once (in bash, zsh or fish):

```sh
mkdir -p ~/.pi/agent/extensions/subagent
ln -sf "$(dirname "$(dirname "$(dirname "$(readlink -f "$(command -v pi)")")")")"/examples/extensions/subagent/{index,agents}.ts ~/.pi/agent/extensions/subagent/
```

If you use pi-amplike, also change its `"npm:pi-amplike"` entry in `~/.pi/agent/settings.json` to `{"source":"npm:pi-amplike","extensions":["!extensions/subagent.ts"]}`. amplike has its own subagent tool, and pi won't start with both.

### Agents and the plate

`orb integration install` teaches agents the plate through the orb skill, and plan-with-me and ship-plan add an item when they finish. Agents follow it more reliably with one line in your global agent instructions: `~/.claude/CLAUDE.md` for Claude Code and `~/.pi/agent/AGENTS.md` for pi. Add it to your own files yourself; orb's install never writes them.

```text
When you end a turn that needs the user (a decision, a review, something to read or walk, an answer), add one plate item with `orb plate add`. Offer follow-up ideas with `--verb idea`. Close your item with `orb plate done <id>` once the user's step is done. If you're a colonist, report to your chief instead.
```

### Files

orb keeps its own files under `~/.orb/`: saved state and plate items in `userdata/state.sqlite`, the search index in `userdata/search.sqlite`, worktrees in `worktrees/`, and a log in `orb.log`.

## Keys

`<C-g>` is the leader in the sidebar and in panes. Press it and a popup lists the keys that can follow. While a pane has the keys, every key goes to its program except the Alt keys, `<C-g>`, `<C-[>` and `<C-]>`.

| Key | Action |
| --- | --- |
| `Alt h/j/k/l`, Alt arrows | Move between panes. At a tab's left or right edge, go to the previous or next tab (wrapping). With a float focused, move between floats. Nothing in the sidebar |
| `Alt n` | Add a pane (with a float focused, add a float) |
| `Alt x` | Close the pane (a pane running an agent asks first) |
| `Alt f` | Show / hide the session's floats; with none, open a shell float |
| `Alt +` / `Alt =`, `Alt -` | Grow / shrink the focused pane, float or the sidebar |
| `Alt 1` to `Alt 9` | Go to tab N |
| `Alt [` / `Alt ]` | Previous / next layout of the tab: vertical, horizontal, stacked, half-stacked (float layouts with a float focused) |
| `Alt i` / `Alt o` | Move the tab left / right |
| `<C-[>` / `<C-]>` | Back / forward through the jump list |
| `<C-g> h` / `<C-g> l` | Keys from a pane to the sidebar / from the sidebar into the panes |
| `<C-g> e` | Hide / show the sidebar |
| `<C-g> s` | Settle the session on screen, after a confirm (in a pane) |
| `<C-g> q` | Quit orb (every pane keeps running) |
| `<C-g> p d/r/s/f/x/c` | Pane: split down, split right, stack, zoom, close, rename |
| `<C-g> p w/e/i` | Floats: show / hide, pane to float or float to tab, pin a float |
| `<C-g> r` / `<C-g> m` | Resize mode / move mode for the focused pane (`h/j/k/l` or arrows; `Esc` leaves) |
| `<C-g> [` | Open the pane's history in `$EDITOR` (else `less`) in a new tab; quitting closes it (in a pane) |
| `<C-g> y` | Paste the pane's selection, or its screen, into a picked agent's prompt and go there (in a pane) |
| `<C-g> t n/x/r/b/1-9` | Tab: new, close, rename, break the pane out to a new tab (in a pane), go to N |
| `<C-g> Space` | Session picker |
| `<C-g> a` | Go to the next agent that needs you: approval, then input, then a finished chat you haven't read, longest waiting first; press again for the next, wrapping at the end. It reaches agents the project filter hides, leaves out failed agents and settled sessions, and with nobody waiting says `No agent needs you` |
| `<C-g> o` | The plate: what needs you, across all of orb. Over a shown session it opens as a float; on the start screen it moves the keys into it. Again, or `Esc`, closes |
| `<C-g> n` | New session |
| `<C-g> P` | Projects: `a` adds one, `d` removes one, `⏎` starts a session |
| `<C-g> f` | Show one project's sessions (sidebar only) |
| `<C-g> i` | Incognito session, for a quick question outside any project |
| `<C-g> w` / `<C-g> b` | Change the session's workspace / branch (not on Incognito sessions or colonies) |
| `<C-g> c` | New colony: pick a world, then name it |
| `<C-g> C` | Worlds: add a folder as a shared or fresh world, remove one, or start a colony |
| `<C-g> ,` | On a colony, list its colonists' agents |
| `<C-g> /` | Search every prompt and reply (`⏎` opens the found agent's pane, or resumes its chat in a new tab if it ended) |
| `<C-g> W` | Worktrees under `~/.orb/worktrees/`, with size, changes and users (`d` deletes one after a confirm) |
| `<C-g> ?` / `?` | List every key that works here; type to filter (`?` in the sidebar) |
| `<C-g> <C-g>` | Send Ctrl g to the pane |

In the sidebar:

| Key | Action |
| --- | --- |
| `j` / `k`, `gg` / `G`, `<C-d>` / `<C-u>` | Move |
| `⏎` | Put the keys in the session's panes, resuming it if it was settled. On an agent row: its pane |
| `p` | Pin or unpin |
| `s` | Settle (after a confirm) or un-settle |
| `d` | Delete the session (its transcripts stay), or close an agent row's pane, after a confirm |
| `r` | Rename the session, or name an agent row's pane |
| `/` or `i` | Search |
| `l` / `h` | Show / fold a card's agent rows or a colony's colonists, or open / close the Settled shelf |

In the plate:

| Key | Action |
| --- | --- |
| `j` / `k`, `gg` / `G`, `<C-d>` / `<C-u>` | Move |
| `⏎` | Go to the item's pane with the keys in it. With the pane gone, open its session (resuming it if settled). With the session deleted, orb says `the session is gone` |
| `gx`, `1`–`9` | Open the item's first link, or link N. A web link (a full `https://…` address) or an `.html` file opens in the session's one terminal-browser tab, opened the first time and reused after. Any other file opens in `$EDITOR`, else `less`, in a new tab. Without terminal-browser, the system opener takes it |
| `x` | Mark done (moves it to Done today) |
| `p` | Park, or bring a parked item back to Your move |
| `u` | Undo the last `x` or `p` |
| `z` | Open / fold Done today |
| `Esc` | Close; the keys go back where they were |

The plate lists every item agents added, grouped as Your move, Waiting on others, Parked and Done today, and ignores the project filter. orb adds its own row for each agent waiting for approval or an answer; the row leaves once the agent stops waiting, and `x` and `p` don't act on it.

Pickers: type to filter, `<C-j>`/`<C-k>` or arrows to move, `⏎` to pick, `Esc` to close. The projects, worlds, session, `Send to` and worktree pickers open as a list where letters are commands, listed at the bottom of the picker; `/` starts typing a filter there.

`<C-[>`/`<C-]>` work like neovim's jump list. Entering a session's panes, moving to another of its panes, `gg`/`G`, a search ended by `⏎`, a new session, a pick in the session, search or `Send to` picker, `⏎` in the plate, and `<C-g> a` are jumps; `j`/`k` aren't. orb keeps the newest 20 across restarts.

## Mouse

orb captures the mouse from the moment it starts.

- A click selects a sidebar or picker row. A double-click, two clicks on the same row within 500 ms, acts as `⏎`.
- A click on a pane focuses it, and the click doesn't reach the pane's program.
- A click on a tab in the tab bar shows it with the keys in its focused pane, a click on a `← +N` or `+N →` chip shows the nearest hidden tab on that side, and a click on `+` opens a tab. The wheel over the bar shows the next tab on a scroll up and the previous one on a scroll down, without wrapping.
- The wheel over a stack's list shows the next stacked pane on a scroll down and the previous one on a scroll up, without wrapping, and leaves the keys where they are.
- The wheel over a pane goes to its program if the program tracks the mouse. Otherwise it sends arrow keys on the alternate screen, and scrolls the pane's history everywhere else. History holds 10 000 lines, from what orb saw after it attached.
- A left drag in a pane whose program doesn't track the mouse (a shell, plain `claude`, plain `pi`) selects text, and the release copies it through OSC 52. A double-click selects a word and a triple-click a line. Dragging past the top or bottom edge scrolls while selecting.
- In a program that tracks the mouse, like lazygit or a fullscreen Claude, clicks and drags go to the program.
- A click on a float brings it to the top with the keys, dragging its frame moves it, and a click on a tiled pane hides the floats.
- With no picker or rename box open, the wheel over the sidebar scrolls only its view, 3 lines a notch. It never moves the selection. Over a picker's list it moves the picker's selection.
- A click outside a picker or the rename box closes it, as `Esc` does.
- While the plate has the keys, a click off it closes it, as `Esc` does. The start-screen plate takes no clicks.
- A click on the sidebar's search box starts a search, and a click in any input's text moves the cursor there.

kitty's Shift+drag still selects with kitty's own selection, past orb.
