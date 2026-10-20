---
name: make-pr-easy-to-review
description: Prepares a PR for review by cleaning noisy commit history and rewriting the PR description with reviewer guidance, without changing code behavior. Use for "make this easy to review", "tidy this PR", or "clean up commits".
metadata:
  based-on: "make-pr-easy-to-review from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Make PR Easy to Review

Prepare a PR so a reviewer can quickly understand the intent, important files, and risk. The default goal is reviewability without behavior changes.

## Workflow

1. Resolve the target PR from the user-provided URL or current branch.
2. Inspect commits, diff size, changed paths, generated files, and PR description.
3. Identify reviewability issues: noisy commits, stale description, unrelated changes, mixed mechanical and logic changes, missing tests, or unclear reviewer entry points.
4. Apply safe improvements, then verify the tree or diff still matches the intended code.
5. Report what you changed, including whether you rewrote history.

## Before the PR exists

When the branch has no PR and hasn't been pushed (ship-plan calls it this way), every commit is local and yours, so clean up history without being asked. Note `git rev-parse HEAD^{tree}` before you start and check it's unchanged after. Don't push, and write the description to the file the caller names instead of running `gh pr edit`. If the caller gives the description's sections (ship-plan does), use those. If it gives a plan folder with pictures, pick them as below and list the files to attach after the description.

## History Cleanup

Rewrite history only when the user asked for commit cleanup and every commit on the branch is theirs (check `git log --format='%an' origin/<baseRefName>..origin/<headRefName>`). If someone else has pushed to the branch, leave history alone and say so in the report. Before rewriting:

```bash
gh pr view <PR> --json title,headRefName,baseRefName,state,commits
git fetch origin <headRefName> <baseRefName>
ORIGINAL_TREE=$(git rev-parse origin/<headRefName>^{tree})
```

Good commit groupings usually follow dependency order:

1. Schema/storage or generated API definitions.
2. Core logic.
3. Wiring and integration.
4. UI or surface behavior.
5. Tests.

After rewriting, verify content identity:

```bash
echo "Original tree: $ORIGINAL_TREE"
echo "Current tree:  $(git rev-parse HEAD^{tree})"
git diff origin/<headRefName> --stat
```

Do not push if the tree changed unintentionally. If it matches, push with `git push --force-with-lease` (never plain `--force`) and say in the report that history was rewritten.

## Reviewer Guidance

Guidance goes in the PR description (`gh pr edit <PR> --body-file <file>`), never in comments on the diff. Keep it short enough that the reviewer reads all of it. Put these at the top, before the repo's usual template:

- **Summary:** what changed, at most 5 bullets, matching the actual diff. Name the generated or mechanical files in one line, so the reviewer can skip them.
- **How it was tried:** the tests and checks that ran and what they showed, with pictures (see below). Script output and logs go in as a table or code block, never as a picture of text. Commands to try it only if they run from the repo itself.
- **Known limitations:** stated as facts for a reviewer, worst first, not as asks: risky behavior changes, migration order or rollout, gaps in the tests, open questions. Write each one out; don't send readers elsewhere for the rest.

Link issue trackers, dashboards, or design docs when they explain intent. Never point at what a reviewer can't open: the plan folder, a recipe, a path on this machine. Leave out everything else. A reviewer skims a long description.

## Pictures

When the plan folder has `media/media.md`, taste-test's pictures and videos, show the change instead of only describing it.

- **Pick up to 3 pictures and 1 short video:** the ones that best show the change working. Anything still broken gets one, even past that. Don't mention the rest. A video is a real recording that ends on what it proves; watch it before picking it.
- **Put each under the line it proves,** as `![<what it shows>](<path>)`, videos too, with the same path you pass to `--attach`. gh swaps each one for its upload in place. A path on its own stays as text, and its file lands at the bottom.
- **In a public repo, pick only pictures that show nothing private:** no names, paths, messages or tokens from the user's data. If none qualifies, leave them all out and say so.
- **Leave out any file over 10 MB.** GitHub refuses them.
- **Attach them** with one `--attach <path>` each, on the `gh pr create` or `gh pr edit` that sets the description. It needs gh 2.99 or newer. With an older one, leave the pictures out and say so under Known limitations.
- **If gh says attaching files requires write access,** and `gh auth status` lists another login for the host, run that one command again with `GH_TOKEN` set from `gh auth token --user <login>`. Never print the token.
- **If `gh pr create` fails after printing a link,** the PR exists. Fix it with `gh pr edit`, which is safe to repeat, and never open a second PR. If attaching still fails, take the picture lines out and say so under Known limitations.

## Guardrails

- Never hide meaningful behavior changes inside "cleanup".
- Never post PR comments or review annotations. The description is the only place you write.
- Do not bypass hooks unless the user explicitly asks.
- If the PR is too large to make reviewable with notes, recommend splitting instead of polishing around the problem.
