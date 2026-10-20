#!/usr/bin/env python3
"""Copy the skills orb ships from the skills repo (~/.agents/skills) into orb's assets.

  sync.py [--add NAME ...]   copy, regenerate the embedded list and the doc counts
  sync.py --commit           the same, then `just test` and `just lint`, then commit on main
  sync.py --install-hook     make the skills repo run `--commit` after every commit and pull

The skills repo is where skills are edited; orb's copies are never edited by hand.
See README.md next to this file.
"""

import argparse
import fcntl
import filecmp
import re
import shutil
import subprocess
import sys
from pathlib import Path

ORB = Path(__file__).resolve().parents[2]
SOURCE = Path.home() / ".agents" / "skills"
ASSETS = ORB / "crates/orb-domain/src/feat/integration/assets/skills"
SKILLS_RS = ORB / "crates/orb-domain/src/feat/integration/skills.rs"
COUNT_DOCS = [ORB / "README.md", ORB / "docs/product.md"]
LOOPS = ["run-micro-loop", "run-phased-loop", "run-simple-loop"]
LOOP_RULES = "task-loop-rules.md"
# orb is published; a skill that names these isn't fit to ship.
PRIVATE = re.compile(r"itemku|multirealmgames|/Users/|/home/\w", re.IGNORECASE)
SKIP = {".DS_Store"}


NOTIFY = False


def die(message):
    print(f"sync-skills: {message}", file=sys.stderr)
    if NOTIFY:
        # Run from the skills repo's hook, where nobody sees the output.
        subprocess.run(["osascript", "-e", 'display notification "see .git/orb-sync.log in the skills repo" '
                        'with title "orb skill sync failed"'], check=False)
    sys.exit(1)


def git(*args, cwd=ORB):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=True).stdout.strip()


def shipped(add):
    names = sorted({p.name for p in ASSETS.iterdir() if p.is_dir()} | set(add))
    for name in names:
        if not (SOURCE / name / "SKILL.md").is_file():
            die(f"{name} isn't in {SOURCE}")
    return names


def source_files(name):
    root = SOURCE / name
    for path in sorted(root.rglob("*")):
        if path.is_file() and path.name not in SKIP and not (name in LOOPS and path.name == LOOP_RULES):
            yield path.relative_to(root)


def check_private(names):
    found = []
    for name in names:
        for rel in source_files(name):
            text = (SOURCE / name / rel).read_text()
            found += [f"{name}/{rel}: {m.group(0)}" for m in PRIVATE.finditer(text)]
    if found:
        die("private text in a skill orb ships:\n  " + "\n  ".join(found))


def copy(names):
    rules = [SOURCE / loop / LOOP_RULES for loop in LOOPS]
    if not all(filecmp.cmp(rules[0], r, shallow=False) for r in rules[1:]):
        die(f"the three {LOOP_RULES} copies in {SOURCE} differ; make them the same first")
    for name in names:
        shutil.rmtree(ASSETS / name, ignore_errors=True)
        for rel in source_files(name):
            (ASSETS / name / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(SOURCE / name / rel, ASSETS / name / rel)
    shutil.copyfile(rules[0], ASSETS / LOOP_RULES)


def regenerate_list():
    files = sorted(
        str(p.relative_to(ASSETS)) for p in ASSETS.rglob("*") if p.is_file() and p.parent != ASSETS
    )
    entries = "".join(f'    (\n        "{f}",\n        include_str!("assets/skills/{f}"),\n    ),\n' for f in files)
    entries += "".join(f'    ("{loop}/{LOOP_RULES}", LOOP_RULES),\n' for loop in LOOPS)
    text = SKILLS_RS.read_text()
    start = "pub(super) const SKILLS: &[(&str, &str)] = &[\n"
    head, _, rest = text.partition(start)
    _, _, tail = rest.partition("\n];\n")
    if not tail:
        die(f"can't find the SKILLS list in {SKILLS_RS}")
    SKILLS_RS.write_text(f"{head}{start}{entries}];\n{tail}")
    subprocess.run(["cargo", "fmt"], cwd=ORB, check=True)
    return len({f.split("/")[0] for f in files})


def update_counts(total):
    workflow = total - 1
    rules = [
        (r"and \d+ skills that take an approved plan", f"and {workflow} skills that take an approved plan"),
        (r"orb's \d+ skills \(the `orb` skill and \d+ plan-to-PR", f"orb's {total} skills (the `orb` skill and {workflow} plan-to-PR"),
    ]
    for doc in COUNT_DOCS:
        text = doc.read_text()
        for pattern, replacement in rules:
            text = re.sub(pattern, replacement, text)
        doc.write_text(text)


def paths():
    return [str(p.relative_to(ORB)) for p in [ASSETS, SKILLS_RS, *COUNT_DOCS]]


def ready_to_commit():
    if git("branch", "--show-current") != "main":
        die("orb isn't on main; skipped")
    if git("diff", "--cached", "--name-only"):
        die("orb has staged changes; skipped")
    if git("status", "--porcelain", "--", *paths()):
        die("orb already has uncommitted changes in the synced files; skipped")


def commit():
    if not git("status", "--porcelain", "--", *paths()):
        print("orb is already in sync")
        return
    for step in (["just", "test"], ["just", "lint"]):
        if subprocess.run(step, cwd=ORB).returncode != 0:
            die(f"`{' '.join(step)}` failed; the synced files are left uncommitted in {ORB}")
    git("add", "--", *paths())
    sha = git("rev-parse", "--short", "HEAD", cwd=SOURCE)
    git("commit", "-m", f"chore(skills): sync from the skills repo at {sha}")
    print(f"committed {git('rev-parse', '--short', 'HEAD')} on orb's main")


HOOK = """#!/usr/bin/env python3
# Written by orb's .agents/sync-skills/sync.py --install-hook. Syncs orb in the background.
import subprocess
import sys
from pathlib import Path

SYNC = Path({sync!r})
if SYNC.is_file():
    log = (Path(__file__).resolve().parents[1] / "orb-sync.log").open("a")
    log.write("\\n--- sync\\n")
    log.flush()
    subprocess.Popen([sys.executable, str(SYNC), "--commit", "--from-hook"], start_new_session=True,
                     stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
"""


def install_hook():
    hooks = SOURCE / ".git" / "hooks"
    if not hooks.is_dir():
        die(f"{SOURCE} isn't a git repo")
    for name in ("post-commit", "post-merge"):
        hook = hooks / name
        if hook.exists() and "sync-skills/sync.py" not in hook.read_text():
            die(f"{hook} exists and isn't ours; merge it by hand")
        hook.write_text(HOOK.format(sync=str(Path(__file__).resolve())))
        hook.chmod(0o755)
        print(f"wrote {hook}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--add", nargs="+", default=[], metavar="NAME", help="also ship these skills")
    parser.add_argument("--commit", action="store_true", help="test, lint and commit on main")
    parser.add_argument("--install-hook", action="store_true", help="install the skills repo's hooks")
    parser.add_argument("--from-hook", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    global NOTIFY
    NOTIFY = args.from_hook
    if args.install_hook:
        install_hook()
        return
    # Two quick commits start two syncs; the second waits for the first.
    with open(ORB / ".git" / "sync-skills.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if args.commit:
            ready_to_commit()
        names = shipped(args.add)
        check_private(names)
        copy(names)
        total = regenerate_list()
        update_counts(total)
        print(f"synced {total} skills from {SOURCE}")
        if args.commit:
            commit()


if __name__ == "__main__":
    main()
