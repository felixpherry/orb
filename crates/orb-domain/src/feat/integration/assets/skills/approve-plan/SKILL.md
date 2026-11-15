---
name: approve-plan
description: Turns an approved plan into a standalone spec (.plans/{task}/plan.md) and a task list (tasks.md), then picks the task loop, model and effort and starts an unattended run with ship-plan in its own orb session, which ends in a PR. Use right after the user approves a plan from plan-with-me or a fix plan from diagnose-bug, or says "approve", "ship it" or "go".
---

# Approve plan

The user has approved the plan. Write the spec and the task list, then start the run. After that the user's part is done: the run ends in a PR.

## 1. Write the spec

Write `.plans/<task>/plan.md`, where `<task>` is the task name as a slug. Copy the section headings from [references/spec-template.md](references/spec-template.md) and fill each from the whole planning conversation: every decision, code reference and algorithm discussed.

The spec stands alone. A fresh agent with no memory of the conversation must be able to build the feature from this document only.

**A fix plan from diagnose-bug** goes in its notes folder, `.plans/<bug>/`, next to `diagnosis.md`, the repro and `fix.patch`. The fix starts from `fix.patch`. The repro becomes a test in the repo where one fits; otherwise rerunning it is an acceptance criterion.

## 2. Write the task list

Write `.plans/<task>/tasks.md` with every task from the approved plan, in phase order: one `##` header per phase, one `- [ ]` line per task. The task loops read and tick this file.

```markdown
## Research
- [ ] Map the existing call sites

## Build
- [ ] Add the parser

## Verification
- [ ] <each acceptance criterion, one task each>
```

The last phase is always Verification. If the spec has Record Updates, Verification ends with this task: "Record Updates: review the complete implementation. If it matches the spec's Record Updates section, write those exact entries into `.agents/RECORD.md`. If it diverged, write no entry and report the divergence in the final summary."

## 3. Check both files

Run `python3 -I scripts/check_spec.py .plans/<task>` from this skill's folder and fix what it prints until it prints nothing. Then re-read the spec once for what the script can't see: every phase of the approved plan is covered, and nothing in it depends on the conversation.

## 4. Pick the loop, model and effort

Give a one-line reason for each. If the user's approval named any of them ("phased, opus"), use those.

- **run-micro-loop:** simple, low-risk work where building between phases would only waste time.
- **run-simple-loop:** everyday work, steered by `check`, including refactors where the build breaks between tasks.
- **run-phased-loop:** work where later phases depend on what earlier ones discover, or anything risky or large.

Default to the model you run on at `high` effort. Use a faster model at `medium` only for micro runs.

## 5. Check for a recipe

taste-test tries the build with the repo's recipe. Run `~/.agents/skills/write-recipe/scripts/recipe_dir.sh`. If it exits 1, ask the user: "<repo> has no recipe yet, so the taste test will build its own setup. Write one now? write-recipe asks you a few questions first." On yes, run write-recipe, then go on. On no, go on.

## 6. Start the run

The plan's approval covers this step, so don't ask again.

- **In orb** (`$ORB_PANE_ID` is set): use the hand-off skill with a new worktree, the chosen model and effort, and this goal: run `/ship-plan <absolute path to .plans/<task>> <loop> report-to <your pane id>`, where `<loop>` is `micro`, `simple` or `phased`.
- **Not in orb:** create a branch and run that same command yourself, minus `report-to`.

## 7. Report

Three lines: where the spec and task list are; the loop, model and effort, each with its reason; and where the run is going and that it reports back with a PR.
