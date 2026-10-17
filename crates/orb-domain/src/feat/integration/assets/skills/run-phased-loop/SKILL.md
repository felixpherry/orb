---
name: run-phased-loop
user-invocable: false
description: Execute a phased plan from `.plans/{task}/` with a written execution plan, full test run, commit, review, and spec annotation at the end of every phase. Use for multi-phase work where later phases depend on what earlier ones discover.
argument-hint: "<.plans/task or slug>"
---

# Phased Task Loop

A disciplined workflow for implementing multi-phase coding tasks. `tasks.md` tracks progress in real time; each phase leaves behind an execution plan, a review, and an annotated spec, so the next phase plans from current facts.

**Arguments:** `$ARGUMENTS` — the plan (a `.plans/<task>/` path or slug). Always run the whole plan.

**Read `task-loop-rules.md` in this skill's folder first.** It holds the project commands, constraints, concepts, task-list discipline, and finishing steps all three task loops share. This file adds only what is specific to this loop.

**Orchestrated mode:** read [ORCHESTRATED.md](ORCHESTRATED.md) before step 1. It assigns each loop step to you or a subagent; the steps below still define what each must achieve. A subagent whose prompt says to run solo does every step itself instead, skipping ORCHESTRATED.md. Without that instruction, a subagent running this loop (like ship-plan's build helper) is the orchestrator.

---

## Constraints (on top of the shared ones)

- **Sync before the next phase.** Run `sync` at the end of every phase.

Resolve every judgement call yourself and record it in the phase review's **Divergence**.

---

## Concepts (on top of the shared ones)

**Execution plan** — The file `phase-N.md`: file-by-file implementation details, with an acceptance-criteria checklist at the bottom.

---

## The Loop

**Repeat until every task is complete.** Work per phase, top to bottom.

1.  **Check status.** `Read` `tasks.md`. Every task `[x]` → go to **When you are done**. Otherwise start at the next task.

2.  **Write the execution plan** to `.plans/<task>/phase-N.md` with these sections:
    - **Problem** — What and why.
    - **What Moves / What Stays** — Scope boundary.
    - **File Changes** — Numbered list. For each: created/modified/deleted, before/after code snippets, source→destination for moves.
    - **Implementation Order** — Sequence of file changes.
    - **Acceptance Criteria** — `- [ ]` checklist; each item independently verifiable (file exists, named test passes, import resolves).

    The execution plan says _how_, file by file; point at the spec rather than copying it. Before implementing, check: follows conventions? No circular deps? All consumers updated? Tests preserved?

3.  **Implement, one task at a time:**

    a. **Restate** the current task in one sentence.

    b. **Do the work** for that task. Run `check` after each logical group of changes; if it fails, fix and re-run.

    c. **Tick it** in `tasks.md`.

    d. **Pick the next task** under the current phase header and repeat from (a). When the phase has no `- [ ]` left, go to 3.5.

3.5. **Audit.** Re-read `tasks.md`. Any task in this phase not `[x]` means you drifted: return to step 3 with it. The list must be clean before you test or commit.

4.  **Verify.** Run the full `test` suite; all tests pass. Then take the acceptance criteria **one at a time**: gather its evidence (test name, command output, file check), and only then flip that line to `[x]`. Fix any gap before proceeding. Flipping criteria in bulk turns them back into a task list.

5.  **Commit and sync.** `commit` with message `<TASK> Phase N: <one sentence>`, then `sync` (resolve conflicts, re-test, commit the merge). Commit only green code.

6.  **Review.** _Append_ to `phase-N.md`: **Changes** (what and why), **Divergence** (what didn't go to plan, or "None"), **Verification** (how verified), **Risks** (concerns or follow-up).

7.  **Annotate the spec** — strikethrough and divergence notes only, every phase. The next execution plan is written from the spec, so it must reflect what actually happened.

8.  **Go to step 1.**

## When you are done

Follow **Finishing** in the rules file.
