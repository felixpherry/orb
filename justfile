# sccache (optional): when installed, cargo compiles through it so dependency
# crates are served from its cache instead of being recompiled. Builds fall
# back to plain rustc when sccache is missing, so it is never required. A
# caller-provided RUSTC_WRAPPER (set, or explicitly empty to disable) wins.
export RUSTC_WRAPPER := env_var_or_default('RUSTC_WRAPPER', `command -v sccache 2>/dev/null || true`)

# ponytail: Homebrew Rust has no rustup llvm-tools; coverage uses llvm@22 to match rustc 1.98's LLVM 22. Bump the formula when `rustc -vV` reports a new LLVM major.
export LLVM_COV := env_var_or_default('LLVM_COV', '/opt/homebrew/opt/llvm@22/bin/llvm-cov')
export LLVM_PROFDATA := env_var_or_default('LLVM_PROFDATA', '/opt/homebrew/opt/llvm@22/bin/llvm-profdata')

# Review changes and pass just test before committing.
commit MSG:
    git add --all
    git diff --cached --check
    git commit -m {{quote(MSG)}}

# PROTOTYPE: the real orb with a LazyVim-style which-key popup
# (helix|classic|modern|current; ←/→ while a popup is open flips variants).
which-key VARIANT="helix":
    ORB_WHICHKEY_VARIANT={{VARIANT}} cargo run

# PROTOTYPE: print every which-key variant for ␣, g and z, in colour.
which-key-dump WIDTH="140" HEIGHT="36":
    cargo run -q -p orb-tui --example which_key_prototype -- {{WIDTH}} {{HEIGHT}}

test:
    cargo test --workspace

check:
    cargo check --workspace

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt -- --check

fmt-fix:
    cargo fmt

# Run all linters (check + clippy + fmt check + test-attr guard)
lint:
    cargo check --workspace
    just clippy
    cargo fmt -- --check
    just lint-testattr

# Fail on bare #[test]/#[tokio::test] lacking an rstest attr (escapes the rstest timeout)
lint-testattr:
   #!/usr/bin/env python3
   import os
   import re
   import sys

   ALLOWLIST = set()
   SKIP_DIRS = {"target", ".git", ".plans"}

   def is_test_attr(s):
       if re.fullmatch(r"#\[test\]", s):
           return True
       return bool(re.fullmatch(r"#\[tokio::test(?:\([^()]*(?:\([^()]*\)[^()]*)?\))?\]", s))

   offenders = []
   for dirpath, dirnames, filenames in os.walk(os.getcwd()):
       dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
       for fn in filenames:
           if not fn.endswith(".rs"):
               continue
           fpath = os.path.join(dirpath, fn)
           rel = os.path.normpath(os.path.relpath(fpath, os.getcwd()))
           if rel in ALLOWLIST:
               continue
           with open(fpath, encoding="utf-8") as f:
               lines = f.readlines()
           i, n = 0, len(lines)
           while i < n:
               s = lines[i].strip()
               if s.startswith("#["):
                   # Walk the contiguous attribute region (multi-line ok).
                   depth = 0
                   has_rstest = False
                   has_bare = False
                   bare_line = None
                   j = i
                   while j < n:
                       sj = lines[j].strip()
                       depth += sj.count("[") + sj.count("(")
                       depth -= sj.count("]") + sj.count(")")
                       if "rstest" in sj:
                           has_rstest = True
                       if depth == 0 and is_test_attr(sj):
                           has_bare = True
                           bare_line = j + 1
                       if depth <= 0:
                           nxt = j + 1
                           if nxt < n and lines[nxt].strip().startswith("#["):
                               j = nxt
                               continue
                           break
                       j += 1
                   else:
                       j = n - 1
                   if has_bare and not has_rstest:
                       offenders.append((rel, bare_line))
                   i = j + 1
               else:
                   i += 1

   for rel, ln in offenders:
       print(f"ERROR: {rel}:{ln}: bare test attribute lacks an #[rstest::rstest] companion", file=sys.stderr)
   if offenders:
       print(
           f"\n{len(offenders)} test(s) would run WITHOUT the rstest timeout.\n"
           "Stack #[rstest::rstest] above the attribute (or extend the allowlist\n"
           "in the lint-testattr recipe with a stated reason).",
           file=sys.stderr,
       )
       sys.exit(1)

coverage:
    cargo llvm-cov --workspace --lcov --output-path coverage.lcov

coverage-report:
    cargo llvm-cov report --html

debt: coverage
    debtmap analyze . --lcov coverage.lcov
