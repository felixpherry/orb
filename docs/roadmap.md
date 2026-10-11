# orb — Overhaul roadmap

> The architecture overhaul after orb became an agent multiplexer (2026-10). This file is the overhaul session's working state: fill each section as the session reaches it.
> Product spec: [`product.md`](product.md). Decisions already made: [`decisions.md`](decisions.md). Current state: `.agents/RECORD.md`. The pre-overhaul roadmap (milestones 0–10) is in git history.

## Goal

Fit orb's architecture to what orb is today, a multiplexer, and keep it easy to change. Covers code shape (file and crate boundaries), concurrency (locks, threads, actors) and observability (seeing what is running or stuck).

## Debt findings

_Empty until the audit runs._

## Reference patterns

From jinn, herdr and zellij. _Empty until the scouts run._

## Target architecture

One entry per accepted decision: the debt it fixes, the pattern it borrows, what it costs, and any rule a script can check. _Empty until checkpoint 2._

## Migration steps

In order. Each step keeps the build and tests green. _Empty until checkpoint 2._

## Drift log

Where the implementation left the target, and why.
