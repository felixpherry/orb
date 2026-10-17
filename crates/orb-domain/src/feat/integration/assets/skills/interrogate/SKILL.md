---
name: interrogate
description: "Has four agents review the same code change independently, each from its own angle (correctness, security, structure, tests), then merges their findings and sorts each into act on, consider, noted, or dismissed. Use for \"interrogate\", \"adversarial review\", \"multi-model review\", \"challenge this\", \"stress test this code\", \"find blind spots\", \"tear this apart\", or as the autopilot's review step."
metadata:
  based-on: "interrogate from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Interrogate

Spawn four reviewers on the same model to adversarially review code changes. Each owns one angle, so they don't all find the same things. A problem raised from two angles independently is the strongest signal. Always spawn all four, however small the diff: a small diff is cheap to review, and the angles exist so no single reader decides what's worth checking. The deliverable is a synthesized verdict. Do NOT auto-apply changes.

## Step 1, Scope

Identify what to review from context:

- If the user points at specific files or a diff, use that.
- If on a feature branch, run `git diff main...HEAD` (or the appropriate base branch) for the full changeset.
- If the user's message references recent work, gather the relevant files.

Package the diff or file contents with any surrounding context files the reviewers need to understand the code.

## Step 2, Intent

Before spawning reviewers, state the intent in one clear paragraph from the user's message, commit messages, the PR description if one exists, and the code. If you're unsure about the intent, take it from the plan, the PR description, or the commits, and say which. Don't stop to ask.

## Step 3, Spawn Reviewers

Launch all four reviewers in a single message with the `Agent` tool, each with one angle:

| Subagent | Angle | Rubric sections it goes deepest on |
|----------|-------|------------------------------------|
| Reviewer A | Correctness | Correctness; Root Causes vs. Symptoms |
| Reviewer B | Security | Security |
| Reviewer C | Structure | Structural Integrity; Complexity Budget; the code-quality lens |
| Reviewer D | Tests | Verification |

All four run on `opus`, or on the model the user or caller names. Each gets `subagent_type: "general-purpose"` and is told to read only: no edits. If the `Agent` tool rejects the model, use the closest valid one from the error message and say so. Don't block the review on it.

Read `references/reviewer-prompt.md` and fill in the same template for every reviewer with its angle, the stated intent, the diff or file contents, the rubric from `references/rubric.md`, and the code-quality lens from `references/code-quality-review.md`.

## Step 4, Synthesize

As results come back, build a unified picture. Parse every reviewer's findings. Merge findings that describe the same issue differently, and note which reviewers raised each one. Findings two or more reviewers raised independently are the highest signal. Read a lone reviewer's finding, but weight it accordingly. Note disagreements. If one reviewer flags something and another explicitly says the opposite, that is context for the verdict.

## Step 5, Lead Judgment

You are the lead reviewer, a pragmatic senior engineer, not a neutral aggregator. Read `references/lead-judgment.md` for the full framework. Put every finding in one of four buckets, with the reviewer(s) that raised it and a one-line rationale.

- **Act on**. Real issues affecting correctness, security, or maintainability given the actual goals. They would block a real PR.
- **Consider**. Legitimate, but you're not sure they outweigh the cost of addressing them now. Worth the user's attention.
- **Noted**. Technically valid but not actionable. Context-dependent, premature optimization, or low-impact at this stage.
- **Dismissed**. Wrong, nitpicky, or missing context, with a brief reason.

## Output Format

### Intent
> [The stated intent paragraph from Step 2]

### Reviewers
- Reviewer [label]: [angle], [N findings] (one bullet per reviewer)

### Act On
[Each: description, which reviewers raised it, why it matters.]

### Consider
[Each: description, which reviewers raised it, the tradeoff.]

### Noted
[Brief list.]

### Dismissed
[Each with a brief rationale.]

### Agreement Map
[Where reviewers agreed and diverged, and what that pattern tells us.]
