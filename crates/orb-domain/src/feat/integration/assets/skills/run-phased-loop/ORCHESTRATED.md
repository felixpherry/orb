# Orchestrated mode

You are the **orchestrator**: you own the loop, subagents (the `Agent` tool) do the work. Your context stays lean — you read `tasks.md`, `git status`, and subagent reports. Source files, diffs, and test output stay inside subagents.

Use the subagent model the user names; otherwise inherit. Run subagents **sequentially** — parallel ones collide on shared files.

## Who runs each loop step

| Step | Runner |
|---|---|
| 1. Check status | You — `Read` `tasks.md` |
| 2. Execution plan | **Plan subagent** |
| 3. Implement | **Task subagents**, one per green unit |
| 3.5 Audit | You — re-read `tasks.md`, `git status --short` |
| 4–7. Verify, commit, sync, review, annotate | **Verify subagent** — a fresh one that implemented nothing |
| When you are done | **Verify subagent** runs Finishing step 1; you hand back (step 2) |

A **green unit** is the smallest run of consecutive task lines after which `check` passes. Usually one line. Group lines when an earlier one breaks the build until a later one lands (removing a field breaks every use of it until each use is updated).

## Subagent prompts

Every prompt carries:

- **Paths**, not pasted content: `plan.md`, `phase-N.md`, `tasks.md`, and the project's `CLAUDE.md` / `AGENTS.md`.
- **The job**: "You are a subagent — run solo." plus the loop step(s) it runs from this skill's SKILL.md (give the subagent its absolute path) plus the shared rules in task-loop-rules.md in this skill's folder (absolute path), and for task subagents the exact task line(s) it owns.
- **Recap**: 2–4 sentences on what earlier units changed, drawn from their reports.
- **Blocked rule**: if something cannot be done as planned and no obvious solution aligns with the request, stop and return `BLOCKED: <question + options>`.
- **Report shape**: ≤5 lines — files touched, `check`/`test` result, notes for the next unit.

Task subagents implement only their lines, run `check` (plus the tests nearest the change), tick each owned line the moment it's done, and add discovered work as new `- [ ]` lines.

The verify subagent checks each acceptance criterion individually with cited evidence, and records every judgement call the task subagents reported in **Divergence**.

## Handling reports

- **`BLOCKED:`** → decide as the rules file says (the option closest to the plan's intent, plus a divergence note), then hand the decision to a fresh subagent.
- **Otherwise** → confirm the claim cheaply: owned lines ticked in `tasks.md`, no unexpected files in `git status`. A mismatch means drift — audit before dispatching the next unit.
- **Judgement calls** in a report → forward to the verify subagent for **Divergence**. If the call changes the spec, also add a divergence note to `plan.md`.
