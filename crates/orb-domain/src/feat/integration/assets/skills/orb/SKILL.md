---
name: orb
description: Drive orb, the terminal multiplexer you run in. List sessions, panes and agents; split or stack a pane; run a command in a pane and read its screen, wait for a command to finish, type into programs like vim; start another Claude or pi agent in a pane, prompt it, wait for it and read its reply; start a new session, or colonists from a colony. Use when the user mentions orb, asks to split a pane or run something in another pane, asks you to start, prompt, wait on or collect work from another agent, or asks you to spawn colonists, or to add, update or close an item on the user's plate (`orb plate`). Works only inside an orb pane (ORB_PANE_ID set).
---

# orb

- **You run in a pane of orb,** a terminal multiplexer for coding agents.
- **The `orb` command talks to the running orb.** One command sends one request and prints one reply.
- **`orb help` is the command reference.** This skill gives the rules and the recipes.

## First, check you're inside orb

- **Run `echo "$ORB_PANE_ID"`.** If it prints nothing, tell the user you're not running inside an orb pane, so you can't control orb, and stop.
- **Any command that fails with `not_running`** ("orb is not running") means no orb answers. Say so and stop.
- **Bare `orb` prints where you are:** your pane, tab and session.

## How commands answer

- **Exit 0: done.** Data comes as JSON on stdout. `pane read`, `pane run --wait`, `pane wait`, `pane type --read`, `pane keys --read`, `agent prompt --wait` and `agent reply` print plain text; add `--json` to get JSON. A change with nothing to report prints `{}`.
- **Every command takes `--json`.** On one that already prints JSON it changes nothing.
- **Exit 1: refused.** `{"code", "message", "hint"}` on stderr. **Do what the hint says;** it names the command that fixes it.
- **Exit 2: typed wrong.** One line names the bad word, then come the command's usage line and its help command. Run that help.
- **IDs are numbers,** plate item IDs too. A `[<pane>]` or `[<session>]` left out means your own.
- **Lists show open sessions, live agents and plate items that aren't done.** `hidden` says how many they left out; `--all` shows them.

## Rules

- **Never move the user's keys.** Every command leaves them alone except `orb pane focus` and `orb session focus`. Use those only when the user asks to be shown something.
- **Don't close, rename or settle what you didn't create,** unless the user asks.
- **Mind the defaults.** `orb tab close` with no `--pane` closes your own tab, and `orb session settle` with no ID settles your own session.
- **Don't close the last pane of the session the user is looking at.** orb opens a fresh shell there and moves the user's keys into it.
- **Settled sessions are read-only.** The lists, `agent get` and `agent reply` still work; everything else is refused. Don't try to un-settle one; that's the user's call.
- **There is no delete or quit.** orb can't delete a session or worktree, remove a project or quit through these commands. Don't look for a way.
- **Wait, don't poll.** Use `--wait`, `orb pane wait` or `orb agent wait`. Never loop on sleep and read.
- **Don't `--force` into a program you didn't start.** `busy` means a program runs in that pane. To talk to it (a REPL), type a line with `orb pane type <pane> <text> --enter`; to quit it, `orb pane keys`; or ask the user.

## Run a command and get its result

```sh
orb pane split --direction right --name tests   # {"pane": 12}
orb pane run 12 "just test" --wait              # its output, then [exit 0]
```

- **`--wait` prints the output's last 200 lines, then `[exit n]`.** `--tail <n>` keeps a different number. `--timeout <ms>` caps the call.
- **`busy`: a program already runs in the pane,** named in the message.
- **`not_run`: the shell took the line but didn't run it.** It kept a line it rejected (a syntax error), or it waits for more of the line (an unfinished quote or bracket). orb presses nothing: clear the line with `orb pane keys <pane> ctrl+c`, then fix the command and run it again. If it showed a new prompt without running the line, there is nothing to clear.
- **A `timeout` saying it may still run** means orb couldn't tell: the shell was still drawing its prompt, or it hasn't shown a command start yet. `orb pane read` the pane and press ctrl+c only if the line still sits at the prompt. Don't run the line again.
- **A program that never ends makes `--wait` run to its timeout.** Start a REPL, server or editor with plain `run` (or `--run` on a new pane), then use `pane wait`, or talk to it with `pane type --enter --read`.
- **Exit codes come from the shell's prompt marks.** fish sends them. In a shell that doesn't (bash and zsh usually), `--wait` returns once the screen stays quiet for 1 s, no sooner than 2 s after the line is typed, and prints `[exit ?]`, and `busy` is never checked.
- **Without `--wait`, `run` types the line and Enter and replies `{}`.** A pane not drawn yet gets it once it draws, so no sleep is needed after `split`.
- **The command is one quoted word,** with `--` before it if it starts with `--`.

## A server in a named tab

```sh
orb tab create --name server --run "just run"            # {"pane": 14}
orb pane wait 14 --text "listening on" --timeout 60000   # the first matching line
orb pane read 14 --tail 40                               # the screen's last 40 lines
```

- **`--name`, `--run` and `--cwd` work on `tab create`, `pane split`, `pane stack` and `pane float`.** None of them moves the user's keys.
- **`pane wait` counts text already on screen, and what was typed.** So wait for text only the output prints. `--regex` takes a pattern instead.
- **Only fish's command lines are skipped.** Its prompt marks show which lines they are, so waiting for `ZEBRA42` after `run "sleep 3; echo ZEBRA42"` returns once it prints. Its current prompt does match, so `--text '~>'` waits for fish to be back. In other shells and in programs like python, nothing is skipped: for a program that echoes what you type, use `type --enter --read`.
- **`pane read` is a snapshot.** `--lines <n>` adds history above the screen; `--head <n>` and `--tail <n>` keep the first or last lines.

## A helper agent

```sh
orb agent start --kind claude --split right --name reviewer   # {"pane": 15, "agent": 41, "harness": "claude", "status": "idle"}
orb agent prompt 15 "Review the diff on this branch and list bugs." --wait   # the answer
orb agent start --kind pi --split down -- --model sonnet      # words after -- go to pi
```

- **`--split right|down` or `--tab` makes its pane.** `--pane <id>` uses a shell pane you already have. Leave all three out and it means your own pane, where you already run, so from a shell prompt it's refused with `busy` naming orb: use `--split right` or `--tab`. `--force` goes only with an existing pane.
- **`prompt --wait` prints what the agent wrote since your prompt.** `--timeout <ms>` caps the call. `agent_blocked`: it stopped to ask the user; tell them. `turn_failed`: its model call failed and the message is the error; tell the user. `agent wait` gives `turn_failed` the same way.
- **Long answers are cut to their last 12,000 characters.** A first line says how many were cut. `--full` on `agent reply` or `prompt --wait` prints it all.
- **Prompt an agent that is idle.** Sent mid-turn, your prompt may be folded into the turn already running, so `--wait` ends with that turn. `orb agent reply` still gives everything written since your prompt. If the answer doesn't cover your prompt, `orb agent wait` and read the reply again.
- **The prompt is one quoted word.** If it starts with `-`, put the flags first and `--` before it: `orb agent prompt 15 --wait -- "--force is broken"`.
- **`timeout` on start:** orb never saw it start: either `orb integration install` hasn't run, or Claude is asking whether to trust a new folder. `orb pane read` the pane to see which, and tell the user.
- **The agent is named by its pane.** Use the pane ID for every `agent` command.

## Fan out

```sh
orb agent prompt 15 "Check the API docs."   # {"submitted": true}
orb agent prompt 16 "Check the tests."
orb agent wait 15 --timeout 600000          # {"status": "idle"}
orb agent reply 15
```

- **They work at the same time.** Wait on each in turn, then read its reply.
- **A prompt orb sent under 5 s ago counts as work,** so `agent wait` right after it waits for the turn to start.

## Programs like vim and REPLs

```sh
orb pane run 12 "python3"
orb pane type 12 "print(6*7)" --enter --read   # the screen's last lines, showing 42
```

```sh
orb pane keys 12 i
orb pane type 12 "hello"
orb pane keys 12 esc
orb pane type 12 ":wq" --enter --read          # the screen once vim is done
```

- **`type` presses each character, with no paste.** `--enter` presses Enter after it. `send` pastes, which a program like vim inserts as text. Both take one quoted word, as is.
- **`keys` presses key names and single characters,** like `ctrl+c`, `alt+x`, `f5`, `space` or `q`. `orb pane keys --help` lists the names. An unknown name sends nothing.
- **`--read` on `type` and `keys` prints the screen once it settles.** It waits until the screen changes and then stays the same for 300 ms, at most 3 s (`--timeout <ms>`), and prints its last 20 lines (`--tail <n>`). No sleeps needed. A screen that keeps changing (top, a spinner), or never changes, prints at the timeout.

## Find things

```sh
orb agent list --status needs_approval,needs_input
orb session list --all
orb project list
orb pane list --session 7
orb agent get 15
```

- **`pane list` and `tab list` cover one session,** yours unless you add `--session <id>`. Your own pane row has `"caller": true`; tabs count from 1.
- **`agent get` has the full name, `cwd` and `branch`,** which list rows leave out. Names in lists are cut to 60 characters.

## Drive a browser in a known pane

```sh
orb tab list
orb pane browser 42
terminal-browser action --browser 90107-1 -- snapshot
terminal-browser action --browser 90107-1 done
```

- **Pick the browser's pane from the tab list.** `orb tab list` lists every tab in your orb session, not only the user's visible tab. Add `--session <id>` for another session; your session need not be the one the user sees.
- **Use the returned key, not the example key.** `orb pane browser 42` requires an explicit pane and replies `{"key":"…"}` only for exactly one browser there. No match or multiple matches are refused; it never opens a browser or moves the user's keys.
- **Default browser scope is an orb session, not its visible tab.** Stock terminal-browser selects within a terminal tab; orb reports the session as that tab. Always pass the returned `--browser` key to target the chosen pane, including when releasing control with `done`. There is no `--` before `done`.
- **Run browser actions from your own command tool.** Never send `orb pane run` into a browser pane; it types shell text into the browser instead of running a command. Use stock terminal-browser commands with the explicit key, with no aliases or background execution.

## A whole new session

```sh
orb project list                                   # {"projects": [{"id": 4, "name": "orb", "dir": "...", "kind": "repo"}]}
orb session create --worktree --name "fix login"   # {"session": 7, "pane": 30}
orb agent start --kind claude --pane 30            # its pane is a shell; start the agent there
orb agent prompt 30 "Fix the login bug."
```

- **It defaults to your session's project and its checkout.** `--worktree` makes a new worktree from the default branch (can take ~15 s). `--project <id>` picks another project. **Find a project ID with `orb project list`.**
- **It shows in the sidebar but doesn't open,** so the user's view stays put.
- **One start at a time.** `starting` means another session is being made. Try again in a few seconds.

## Worlds, colonies and colonists

- **A world is a folder colonies run in.** In a shared world every colony works in the world's one folder. In a fresh world each colony gets its own copy of the world's folder. Research and Learn are orb's built-in fresh worlds (`~/.orb/research/<name>/`, `~/.orb/learn/<name>/`); the user's fresh worlds put theirs in `~/.orb/worlds/<world>/<name>/`.
- **A colony is a session in a world; its agent is the chief.** Only the user makes colonies, with `<C-g> c`. You can't.
- **A colonist is a session the chief, or a colonist, makes with `orb session create`.** orb links it under the session of the pane that ran the command (from `ORB_PANE_ID`), folded behind a `colonists` row in the sidebar, at any depth. From a normal session the new session is top-level.
- **"Spawn colonists" means sessions, never panes.** Run `orb session create` once per colonist from your own pane, then `orb agent start --kind claude --pane <pane>` and `orb agent prompt` on each returned pane. Splits, stacks and tabs stay in your session and never show as colonists; they're for helpers working beside you.

```sh
orb session create --name "scout 1"                              # fresh world: in your own folder
orb session create --project 4 --worktree --name "fix payout"    # a worktree of project 4's repo
```

- **Without `--project` it takes your session's project.** In a fresh world it runs in your own folder, with no worktree; in a shared world, in the world's folder; in a repo, in its checkout (add `--worktree` for its own).
- **For a repo, name it:** `--project <id> --worktree` gives the colonist its own worktree there.
- **Two refusals (`bad_request`):** `--worktree` without `--project` from a fresh world (its folders aren't git repositories), and `--project` naming a fresh world, from anywhere (its colonies come from `<C-g> c`).
- **Settling a colony doesn't settle its colonists.** `orb session settle` acts on one session only.

## The plate

```sh
orb plate add "Review PR #42: fix login" --verb review --link https://github.com/o/r/pull/42   # {"item": 7}
orb plate add "Try the new picker" --verb walk --what "Built on orb/picker; run just run." --link ./notes.md
orb plate add "Add a retry to payouts?" --verb idea
orb plate update 7 --waiting         # the next move is someone else's
orb plate done 7                     # {}
orb plate list                       # {"items": [...], "hidden": N}: items that aren't done
orb plate list --all                 # done items too
```

- **The plate is the user's one list of what needs them,** across all of orb. orb shows it on the start screen and with `<C-g> o`.
- **Add one item when you end a turn that needs the user:** a decision, a review, something to read or try, a pick, an answer. Don't add items for work in progress, or for a permission or question prompt you're stuck on. orb adds its own row for an agent waiting for approval or an answer.
- **The title is the user's next step, as an action** ("Review PR #42: fix login"). `--what` says what happened in a sentence or two.
- **Pick the verb that fits.** Anything else is refused with the list.

| Verb | For |
| --- | --- |
| `decide` | Pick a direction. |
| `review` | A diff or PR. |
| `read` | A report or doc. |
| `walk` | Try it by hand. |
| `pick` | Choose among options. |
| `approve` | A plan or a step. |
| `answer` | A question you asked. |
| `reply` | Someone else's message. |
| `merge` | A branch that's ready. |
| `note` | An FYI. |
| `idea` | A follow-up you offer. |

- **Put the ticket, PR and page links on the item** with `--link`, which you can repeat. A web link needs its full address (`https://…`). Anything without `://` is a file, opened in `$EDITOR` (else `less`), except `.html` files, which open in the browser. A relative path starts at your session's folder.
- **One item per thing.** Update yours (`orb plate update <id>`) instead of adding a second. Find an ID with `orb plate list`. `--link` on update replaces all links; left out, they stay.
- **`--waiting`** when the next move is someone else's (CI, a reviewer). `--your-move` moves it back.
- **`--verb idea`** offers a follow-up. It lands in Parked until the user takes it up. An idea can't be `--waiting`.
- **`--urgent`** only when it blocks other work. Urgent items sort first.
- **Close your own item** with `orb plate done <id>` once the user's step is done. Nothing closes items by itself, not even settling or deleting the session.
- **Colonists don't add items.** orb refuses them with `colonist`. Put it in your report to your chief instead.
- **`--pane <id>`** sends the user's ⏎ to another pane, such as a tab running a prototype. By default it's your own pane.

## When orb says no

- **Every refusal carries a hint; follow it.** These codes need your judgement:

| Code | What to do |
| --- | --- |
| `not_running` | Tell the user and stop. |
| `settled` | Leave it; un-settling is the user's call. |
| `colonist` | Put it in your report to your chief. |
| `agent_blocked` | Tell the user. Only if they asked, `orb pane read` the dialog and answer with `orb pane keys`. |
| `prompt_stalled` | `orb pane read` to see why. Don't paste the prompt twice. |
| `turn_failed` | The model call failed (quota, API error). Tell the user. |
| `busy` | Type a line into it (`pane type --enter`), quit it, or ask; `--force` only into your own. |
| `not_run` | If the line was kept or is unfinished, clear it with `orb pane keys <pane> ctrl+c`. Then fix it and run it again. |
| `timeout` | Check with `orb agent get` or `orb pane read` before trying again; a session may already be there. |

## Commands

- **Ask orb itself.** `orb help` lists the nouns and the common jobs, `orb <noun> --help` a noun's verbs, and `orb <noun> <verb> --help` the flags, defaults, examples and the reply.
- **An empty name in `rename`** goes back to the usual name.
- **Splitting, stacking or closing the tab of a floating pane** is refused.
