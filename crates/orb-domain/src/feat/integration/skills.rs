//! The skills orb installs for Claude Code and pi: the `orb` skill and the
//! workflow skills that take an approved plan to an open pull request. Each
//! file is baked into orb from `assets/skills/`.

use std::collections::BTreeSet;

/// The rules the three task loops share, installed into each loop's folder.
pub(super) const LOOP_RULES: &str = include_str!("assets/skills/task-loop-rules.md");

/// Every skill file: its path inside a skills folder, and its contents.
pub(super) const SKILLS: &[(&str, &str)] = &[
    (
        "approve-plan/SKILL.md",
        include_str!("assets/skills/approve-plan/SKILL.md"),
    ),
    (
        "approve-plan/agents/openai.yaml",
        include_str!("assets/skills/approve-plan/agents/openai.yaml"),
    ),
    (
        "babysit/SKILL.md",
        include_str!("assets/skills/babysit/SKILL.md"),
    ),
    (
        "check-blast-radius/SKILL.md",
        include_str!("assets/skills/check-blast-radius/SKILL.md"),
    ),
    (
        "check-gaps/SKILL.md",
        include_str!("assets/skills/check-gaps/SKILL.md"),
    ),
    (
        "check-gaps/agents/openai.yaml",
        include_str!("assets/skills/check-gaps/agents/openai.yaml"),
    ),
    (
        "deslop/SKILL.md",
        include_str!("assets/skills/deslop/SKILL.md"),
    ),
    (
        "diagnose-bug/SKILL.md",
        include_str!("assets/skills/diagnose-bug/SKILL.md"),
    ),
    (
        "falsify/SKILL.md",
        include_str!("assets/skills/falsify/SKILL.md"),
    ),
    (
        "fix-ci/SKILL.md",
        include_str!("assets/skills/fix-ci/SKILL.md"),
    ),
    (
        "fix-merge-conflicts/SKILL.md",
        include_str!("assets/skills/fix-merge-conflicts/SKILL.md"),
    ),
    (
        "get-pr-comments/SKILL.md",
        include_str!("assets/skills/get-pr-comments/SKILL.md"),
    ),
    (
        "hand-off/SKILL.md",
        include_str!("assets/skills/hand-off/SKILL.md"),
    ),
    (
        "interrogate/SKILL.md",
        include_str!("assets/skills/interrogate/SKILL.md"),
    ),
    (
        "interrogate/references/code-quality-review.md",
        include_str!("assets/skills/interrogate/references/code-quality-review.md"),
    ),
    (
        "interrogate/references/lead-judgment.md",
        include_str!("assets/skills/interrogate/references/lead-judgment.md"),
    ),
    (
        "interrogate/references/reviewer-prompt.md",
        include_str!("assets/skills/interrogate/references/reviewer-prompt.md"),
    ),
    (
        "interrogate/references/rubric.md",
        include_str!("assets/skills/interrogate/references/rubric.md"),
    ),
    (
        "make-pr-easy-to-review/SKILL.md",
        include_str!("assets/skills/make-pr-easy-to-review/SKILL.md"),
    ),
    ("orb/SKILL.md", include_str!("assets/skills/orb/SKILL.md")),
    (
        "plan-with-me/SKILL.md",
        include_str!("assets/skills/plan-with-me/SKILL.md"),
    ),
    (
        "plan-with-me/agents/openai.yaml",
        include_str!("assets/skills/plan-with-me/agents/openai.yaml"),
    ),
    (
        "plan-with-me/record-preamble.md",
        include_str!("assets/skills/plan-with-me/record-preamble.md"),
    ),
    (
        "review-code/SKILL.md",
        include_str!("assets/skills/review-code/SKILL.md"),
    ),
    (
        "review-code/agents/openai.yaml",
        include_str!("assets/skills/review-code/agents/openai.yaml"),
    ),
    (
        "run-micro-loop/SKILL.md",
        include_str!("assets/skills/run-micro-loop/SKILL.md"),
    ),
    (
        "run-phased-loop/ORCHESTRATED.md",
        include_str!("assets/skills/run-phased-loop/ORCHESTRATED.md"),
    ),
    (
        "run-phased-loop/SKILL.md",
        include_str!("assets/skills/run-phased-loop/SKILL.md"),
    ),
    (
        "run-simple-loop/SKILL.md",
        include_str!("assets/skills/run-simple-loop/SKILL.md"),
    ),
    (
        "ship-plan/SKILL.md",
        include_str!("assets/skills/ship-plan/SKILL.md"),
    ),
    (
        "taste-test/SKILL.md",
        include_str!("assets/skills/taste-test/SKILL.md"),
    ),
    (
        "write-recipe/SKILL.md",
        include_str!("assets/skills/write-recipe/SKILL.md"),
    ),
    ("run-micro-loop/task-loop-rules.md", LOOP_RULES),
    ("run-phased-loop/task-loop-rules.md", LOOP_RULES),
    ("run-simple-loop/task-loop-rules.md", LOOP_RULES),
];

/// How many skills [`SKILLS`] holds: its distinct top-level folders.
pub(super) fn skill_count() -> usize {
    SKILLS
        .iter()
        .filter_map(|(path, _)| path.split('/').next())
        .collect::<BTreeSet<_>>()
        .len()
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests read the assets folder with `?` and assert on the outcome"
)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use error_stack::{Report, ResultExt};

    use super::SKILLS;
    use crate::feat::integration::IntegrationError;

    /// The shared loop rules' file in the assets folder.
    const LOOP_RULES_FILE: &str = "task-loop-rules.md";
    /// The task loops, each of which gets the shared loop rules.
    const LOOPS: [&str; 3] = ["run-micro-loop", "run-phased-loop", "run-simple-loop"];

    /// Every file under `dir`, keyed by its path relative to `root`.
    fn walk(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<String, String>,
    ) -> Result<(), Report<IntegrationError>> {
        for entry in fs::read_dir(dir).change_context(IntegrationError)? {
            let path = entry.change_context(IntegrationError)?.path();
            if path.is_dir() {
                walk(root, &path, files)?;
            } else {
                let relative = path
                    .strip_prefix(root)
                    .change_context(IntegrationError)?
                    .to_string_lossy()
                    .into_owned();
                let text = fs::read_to_string(&path).change_context(IntegrationError)?;
                files.insert(relative, text);
            }
        }
        Ok(())
    }

    #[rstest::rstest]
    fn the_list_matches_the_assets_folder() -> Result<(), Report<IntegrationError>> {
        // Given the files in the skills assets folder, with the loop rules
        // placed in each loop's folder.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/feat/integration/assets/skills");
        let mut assets = BTreeMap::new();
        walk(&root, &root, &mut assets)?;
        let rules = assets.remove(LOOP_RULES_FILE).unwrap_or_default();
        for name in LOOPS {
            assets.insert(format!("{name}/{LOOP_RULES_FILE}"), rules.clone());
        }

        // When reading the embedded list.
        let listed: BTreeMap<String, String> = SKILLS
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();

        // Then both hold the same files with the same contents.
        assert_eq!(
            listed, assets,
            "every file under assets/skills should be in the embedded list, and only those"
        );
        Ok(())
    }
}
