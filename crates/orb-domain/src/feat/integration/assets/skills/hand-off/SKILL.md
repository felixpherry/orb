---
name: hand-off
description: Hands work to a fresh agent in its own orb session. Writes a handoff note, creates the session (with its own git worktree for code work, or as a colonist when called from a colony), starts Claude or pi with the chosen model and effort, and sends the note as the first prompt, including which pane to report back to. Use when the user says "hand this off" or wants work continued in a new session or worktree, or when another skill such as approve-plan needs to launch a runner.
argument-hint: "[what the new agent should do] [model] [effort]"
---

# orb handoff

Hand the current work to a new agent in its own orb session. If `$ORB_PANE_ID` is empty, you're not in orb: say so and stop.

## 1. Write the handoff note

Write it to `/tmp/orb-handoff-<slug>.md`. Follow `~/.claude/hooks/handoff-note.txt`, the six headings compaction uses. Under Next step, add:

- **Skills:** which skills to load, and the exact first command if one applies (`/run-simple-loop /abs/path/.plans/<task>`).
- **Report back:** `orb agent prompt <your $ORB_PANE_ID> "<one line: result, plus the PR or file link>"`, run when done or stuck.

Make every path absolute: a new worktree has no gitignored files (`.plans/`, `.env`), so point at the originals. Reference specs, plans, commits and tickets by path or link instead of copying them. Leave out secrets: API keys, passwords, tokens.

## 2. Start the session

From this skill's folder:

```
scripts/start.sh --name "<short name>" --note <absolute path to the note> [--worktree] [--project <id>] [--model <model>] [--effort <level>] [--pi]
```

It creates the session, starts the agent with permission prompts off, sends the note as the first prompt, and prints `session=<id> pane=<id>`. The session doesn't open in the user's view.

- **`--worktree`** for code work in a repo: a new worktree from the default branch.
- **`--project <id>`** when you're in a colony and want a worktree. There `--worktree` alone is refused; find the id with `orb project list`.
- **`--model` and `--effort`** only when the user or the calling skill named them.
- **`--pi`** starts pi instead of Claude.
- **Exit 3** means orb saw no agent after 30 s, and the script printed the pane. Claude may be asking whether to trust the new folder. Tell the user; don't answer it for them.

## 3. Tell the user

Say in one line which session started, what its agent is doing, and that it reports back to this pane. When the report arrives, pass it on to the user in chat.
