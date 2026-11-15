---
name: plan-with-me
description: "Plans a software change with the user before any code is written. Use when the user wants to plan, design, or think through a feature, fix, or refactor."
---

# Plan with me

Create a high-level implementation plan for the task at the end of this prompt. Planning only: implement nothing, and edit nothing in the repo, until the user approves the plan.

Don't accept the user's first idea as given. Probe it for edge cases, architectural flaws and better alternatives. Present the plan only once every question is answered. A plan never contains an assumption or an open question.

## 1. Consult the Record

Read `.agents/RECORD.md` if it exists. It is the project's record of **current** state: scoped facts about how the application works now, never future intent.

- **Contradictions gate the plan.** If the feature changes, breaks or replaces what an entry describes, raise the conflict as a question before proposing a plan.
- **Gaps get filled.** If the feature establishes a new high-level fact about the application, propose it as an entry in the plan's "Record Updates" section. No implementation minutiae.
- **Absence is not a constraint.** No entry, or no file, means nothing is recorded there yet.
- **Bootstrapping.** If the file is missing, Record Updates leads with one item: create it with [record-preamble.md](record-preamble.md) as its body, copied verbatim, followed by the proposed entries.
- **Entries are written at the end of implementation,** by a task the approved plan produces, checked against the actual changes.

Before proposing entries, read [references/record-entry-format.md](references/record-entry-format.md).

## 2. Explore and ask

- **Trace the code first.** Verify what exists; don't guess. Find the existing patterns that fit the request.
- **The user hasn't seen the code and doesn't read it.** Say what the code does now in plain words, with a `file:line` they can open: "Saving an order also sends the receipt email (orders.ts:120)", not "this changes `saveOrder()`". Show code only when the code itself is the decision.
- **Probe the request:** clarify vague terms, question assumptions, ask for evidence, trace implications.
- **Questions are a numbered list,** so the user can answer by number. Keep them short; the user wants to answer fast.
- **A question with distinct resolutions gets lettered options (A, B, C).** Each states what it does, why it works, and what choosing it implies. Mark the one you recommend with a one-line reason. Don't force options where none fit; ask directly.
- **By the end you know:** why (trade-offs settled), where (files and paths), what (structures that change), how (the logic flow), and the gotchas (edge cases, anti-goals).
- **Where to ask:** yes/no and quick picks go in chat. Questions with real trade-offs or several options go on a lavish page with clickable choices and your recommendation.

## 3. Probe risky guesses

- **When the plan depends on how a real program behaves** (a shell, a terminal, an editor, a browser, a library, an outside API) and reading code can't settle it, run a throwaway probe before presenting the plan: a small script or command against the real program.
- **Probe the guesses that would change the design if wrong.** Skip the ones the build would catch anyway.
- **Probe in a temp folder.** Never change the user's real data, and leave no probe files in the repo.
- **If a probe needs something you don't have** (production access, credentials, a device, paid calls, someone else's account), ask the user first. They may grant it, or run it and paste the result. If they can't, list the guess in the plan as an open risk.
- **If a probe shows a guess was wrong,** go back to step 2. Tell the user what it showed and which part of the design it breaks, offer the ways around it as lettered options, and probe the path they pick if it rests on a new guess. If no path works, say so and present no plan.

## 4. Present the plan

Present it as a lavish page (load the lavish skill if you have it, else run `npx -y lavish-axi --help`). The plan confirms direction, so it is brief: no deep code snippets, dependency lists or detailed algorithms. It contains:

- **Problem** and **Solution**.
- **Phases,** as a numbered or bulleted list.
- **Acceptance criteria.**
- **A table of test cases.**
- **What each probe showed,** including the ones that changed the design.
- **Record Updates,** if the feature changes a recorded fact or establishes one: the bootstrap item when the file is missing, then the exact entry lines in final on-disk form. No rationales, no preamble changes.

**Put it on the user's plate (in orb only).** When `$ORB_PANE_ID` is set, right after presenting run `orb plate add "Approve the plan for <task>" --verb approve --link <the plan page's address>` and keep the ID it prints (`{"item": N}`). The address is the lavish page's URL, or the HTML file's absolute path. If you revise and present again, run `orb plate update <id>` (a new `--link` if the page moved) instead of adding another. Once the user approves, run `orb plate done <id>`. If orb refuses with `colonist`, skip the plate and keep going.

Then wait for the user's approval.

## Task
