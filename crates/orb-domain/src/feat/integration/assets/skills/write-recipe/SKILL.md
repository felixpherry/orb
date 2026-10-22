---
name: write-recipe
description: Writes a recipe for trying a project the way its user does: how to start a throwaway copy with real data and real logins, check it's healthy, drive it, capture proof and clean up, plus a short map of its main features. Puts every fixed step in a script, with a self-test that runs them all, and proves the recipe before handing it over, and keeps it outside the repo in ~/.orb/recipes, where taste-test finds it. Asks the user an intake round, so it never runs unattended. Use for "write a recipe for this repo", "how do we try this app", when approve-plan finds a repo has no recipe, or when a recipe has gone stale.
---

# Write a recipe

A recipe lets an agent that has never seen the project start a throwaway copy and use it the way the user does, without guessing at settings, logins or ports. Write it for that agent: it reads the recipe cold, in the middle of a task.

## Where it lives

`~/.orb/recipes/<repo>/`, where `<repo>` is the repo's name: the end of its `origin` remote, like `orb`, else its main checkout's folder name. It sits outside the repo, so every worktree finds it and nothing commits it.

- `RECIPE.md`: the recipe.
- Scripts: one per fixed step, executable, each with its usage in its first lines.
- A self-test script that runs all the others.
- `features/`: one file per main feature, and a `README.md` listing them.

If a recipe is already there, read it and fix what's wrong instead of starting over.

## 1. Learn the project, then ask

Look before asking. Answer these from the repo, its docs and its scripts:

- **Who drives it, and how:** a person (a web page, a terminal app, a desktop app), an agent or script (a CLI, an API), or both. A terminal app can take a person's keys and an agent's commands. Cover every way in that real people or agents use.
- **How it starts:** the repo's own run command first. Ports, settings, seed data, logins.
- **Where it keeps state:** its home, config and data folders, its database, and whether two copies can run side by side.
- **How an agent can drive it:** the repo's own test drivers first (browser tests, terminal drivers, HTTP calls). Then a generic way: a browser for web, a terminal driver for terminal apps, plain HTTP for services.
- **What proof you can capture:** screenshots, screen text, responses, logs, exit codes, stored data.

Then ask the user one numbered batch and wait for every answer: what only they know, and what the code left unclear. Typically:

- which of their data to copy, and from where;
- which logins the program needs, and where their tokens live;
- the environments it can run against (a local copy, staging, an internal build on production data), how they switch between them, and what's allowed in each;
- what using the main features looks like to them.

If the user hands you their own steps or rules, use them as given. Send follow-up batches until nothing is left to guess. If the project doesn't build or start as is, fix your setup or tell the user exactly why before you write anything.

## 2. Write the scripts and RECIPE.md

**Scripts do the fixed steps; `RECIPE.md` holds the judgement.** A step that runs the same way every time goes in a script, and `RECIPE.md` shows how to run it. `RECIPE.md` keeps only what needs judgement: the user's rules, what to look at in the proof, and the traps no script catches.

Use this project's real commands, names and paths, with no placeholders. Each part below is a script, except Environments:

- **Launch:** the exact steps to start a throwaway copy, and how to tell it's ready to use (a log line, a prompt, a page that loads), not just that a process answered.
  - Everything it writes goes in a temp folder, never the user's real one.
  - A copy of the user's real local data, when there is some: empty setups hide the bugs users hit. Say what to copy and from where.
  - The real programs it talks to, not fakes.
  - Logins: link the user's token files into the temp folder, or pass a token through the environment. Name where each comes from. Never write a token's value, print it or copy the file.
- **Health check:** one read-only check that this copy is worth driving: it runs, it's the build you meant, the port and data folder are yours, the login works.
- **Drive:** one part per way in (a person's keys or clicks, an agent's commands or API), with this project's real commands, keys, labels or routes. Prefer stable handles (labels, prompt text, routes) over screen positions.
- **Proof:** what to capture and where it goes. Use the real way in, not internal hooks or test-only endpoints. Capture the action and the state after it, and check side effects (files, rows, messages), not only the screen.
- **Pictures and video:** one script saves a picture of what a person sees. Another records a real video while the flow is driven, at the speed it happened, never stills stitched together. Both run off the user's screen: a browser's own screenshot and recording for web pages; for terminal apps, pictures drawn from the saved screen text and a recording of the hidden terminal the app runs in. They're only for what a person sees on screen: output, logs and responses stay text.
- **Environments:** each one it can run against, how to switch to it (a setting, a flag, a command), and what's allowed there: writes, or read only. Write down the user's rules as they gave them. Where they gave none: local data on a copy only, staging writes are fine, production is read only.
- **Cleanup:** stop only what the launch started, never by process name. Remove the temp state and keep the proof. Fail if anything is left.
- **Guards:** when a step could reach the user's real data or folders, the script refuses it, instead of `RECIPE.md` warning about it.

## 3. Map the main features

In `features/`, one file per main feature. Start with 3 to 5, taken from the routes, commands, keys or docs. Each file says:

- **What it is,** from the user's side.
- **How to reach it:** every way in, for people and for agents.
- **How to drive it:** each user action, with its exact command and what you should see.
- **Gotchas:** traps that waste a run or fake a pass.

If the setup lacks something a feature needs, the feature fails and the reason goes in its gotchas. Never weaken a check to make it pass.

## 4. Prove it

Write the self-test, run it, then follow the recipe by hand once: launch, drive one mapped feature, capture its proof, clean up. Check the proof is still there after cleanup. Fix what fails, and run the cleanup after every failed try so nothing is left running. A recipe nobody ran is a draft.

- **The self-test starts a fresh throwaway, runs every script once, and cleans up.** It prints one line per check and fails if any check fails.
- **It tries each kind of input a script takes,** like one key and several, and every refusal. A run that only follows one path misses the rest.
- **A refusal check must be safe if the guard breaks.** Aim it at something that doesn't exist, or at a harmless form of the command, so a broken guard fails the check instead of doing the harm. Break each guard once in a copy and see the self-test fail.
- **Run the self-test again after any change to the recipe.**

## Stay off the user's screen

If a step could raise something the user must answer (a keychain prompt, a login window, a browser opening), tell the user before you run it.

## Hand back

Where the recipe is, the self-test's result, what you proved by hand, and what you couldn't prove, with why.
