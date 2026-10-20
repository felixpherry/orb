# Syncing orb's skills

The skills orb installs are edited in the skills repo (`~/.agents/skills`), never in `crates/orb-domain/src/feat/integration/assets/skills/`. sync.py copies them over.

- **Which skills:** every folder already in orb's assets, plus any named with `--add`. A skill leaves orb only when you delete its folder there.
- **What it writes:** the skill folders, `task-loop-rules.md` (the three loop copies in the skills repo must match), the embedded list in `skills.rs`, and the skill counts in README.md and docs/product.md.
- **What it refuses:** a skill that names itemku, a work email domain or a home path, since orb is published.

```
python3 .agents/sync-skills/sync.py --add NAME   # copy now, nothing committed
python3 .agents/sync-skills/sync.py --commit     # copy, just test, just lint, commit and push main
python3 .agents/sync-skills/sync.py --install-hook
```

- **The hook:** `--install-hook` writes `post-commit` and `post-merge` in the skills repo. Each runs `--commit` in the background and logs to the skills repo's `.git/orb-sync.log`. A failure shows a macOS notification.
- **`--commit` pushes main to origin** as the repo's owner, with gh's token for that account, since the keychain may hold another account. It pushes any other unpushed commits on main too.
- **`--commit` skips** when orb isn't on main, has staged changes, or already has uncommitted edits in the synced files. A failed test or lint leaves the copied files uncommitted.
- **CHANGELOG.md isn't touched.** Add a line by hand when a sync changes what users see.
- **Don't run `orb integration install` from an old build.** It writes orb's built-in copies over the skills repo.
