---
name: review-code
description: "Review the changes since a fixed point (commit, branch, tag, or merge-base) along two axes: Standards (does the code follow this repo's documented coding standards?) and Spec (does the code match what the originating issue/spec asked for?). Runs both reviews in parallel sub-agents and reports them side by side. Use when the user wants to review a branch, a PR, work-in-progress changes, or asks to \"review since X\"."
metadata:
  based-on: "code-review from mattpocock/skills (MIT)"
---

# Review code

Review the diff between `HEAD` and a fixed point on two axes, each in its own sub-agent so neither colours the other:

- **Standards:** does the code follow this repo's documented coding standards?
- **Spec:** does the code do what the originating issue or spec asked for?

If the arguments say `standards-only`, skip the Spec axis: the caller has already checked the spec.

## 1. Pin the fixed point

The fixed point is whatever the user or caller named (a commit SHA, branch, tag, `main`, `HEAD~5`). If none was given, use the merge-base with the main line.

The diff command is `git diff <fixed-point>...HEAD` (three dots, so it compares against the merge-base). The commit list is `git log <fixed-point>..HEAD --oneline`.

Confirm the fixed point resolves (`git rev-parse <fixed-point>`) and the diff is non-empty before spawning anything.

## 2. Find the spec

Look in this order:

1. A path the user passed as an argument.
2. A plan folder for this work: `.plans/<task>/plan.md`, matched by the branch name or the feature.
3. A Jira key (like `PROJ-123`) in the branch name, commit messages, or PR title. Read it with the read-ticket skill if you have it, else with your issue tracker's tools, every custom field included.
4. GitHub issue references in the commit messages (`#123`, `Closes #45`), read with `gh issue view`.
5. A spec file under `docs/`, `specs/`, or `.scratch/` matching the branch name or feature.

If none is found, skip the Spec sub-agent and say "no spec available" in the report.

## 3. Find the standards

Any file in the repo that documents how code should be written, such as `CODING_STANDARDS.md` or `CONTRIBUTING.md`.

The Standards axis also always carries the smell baseline in [references/smells.md](references/smells.md): twelve code smells that apply even when the repo documents nothing. The Standards sub-agent reads that file; you don't need to.

## 4. Spawn both sub-agents in parallel

**Standards sub-agent.** Give it:

- The diff command and the commit list.
- The standards files from step 3, and the absolute path of `references/smells.md` to read first.
- The brief: "Report, per file/hunk where relevant, (a) every place the diff violates a documented standard: cite the standard (file + the rule); and (b) any baseline smell you spot: name it and quote the hunk. Distinguish hard violations from judgement calls: documented-standard breaches can be hard, but baseline smells are always judgement calls, and a documented repo standard overrides the baseline. Skip anything tooling enforces. Under 400 words."

**Spec sub-agent.** Give it:

- The diff command and the commit list.
- The path or fetched contents of the spec.
- The brief: "Report: (a) requirements the spec asked for that are missing or partial; (b) behaviour in the diff that wasn't asked for (scope creep); (c) requirements that look implemented but where the implementation looks wrong. Quote the spec line for each finding. Under 400 words."

## 5. Report

Present the two reports under `## Standards` and `## Spec`, verbatim or lightly cleaned. Don't merge or rerank findings across axes: a change can pass one and fail the other, and a merged list lets one hide the other.

End with one line: the number of findings per axis and the worst issue within each.
