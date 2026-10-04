//! pi's models, from `pi --list-models`: a header line, then one model per
//! line in whitespace-separated columns, provider first, model second.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use error_stack::{Report, ResultExt};
use wherror::Error;

use super::runner::Runner;
use crate::feat::harness::{ModelChoice, ModelGroup};

/// How long `pi --list-models` may run.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// `pi --list-models` failed.
#[derive(Debug, Error)]
#[error(debug)]
pub struct ModelsError;

/// Runs `pi --list-models` and groups what it prints.
///
/// # Errors
///
/// Returns an error, with the reason attached as a `String`, if pi can't run
/// or exits with an error.
pub async fn list(runner: &dyn Runner) -> Result<Vec<ModelGroup>, Report<ModelsError>> {
    let argv = [OsString::from("pi"), OsString::from("--list-models")];
    let output = runner
        .run(&argv, Path::new("/"), LIST_TIMEOUT)
        .await
        .change_context(ModelsError)?;
    match output.code {
        Some(0) => Ok(parse(&output.stdout)),
        code => {
            let detail = output.first_line().map_or_else(
                || {
                    code.map_or_else(
                        || "killed by a signal".to_owned(),
                        |code| format!("exit code {code}"),
                    )
                },
                str::to_owned,
            );
            Err(Report::new(ModelsError).attach(format!("pi --list-models failed: {detail}")))
        }
    }
}

/// The models in `text`, one group per provider in first-seen order; ids and
/// names are `provider/model`.
pub fn parse(text: &str) -> Vec<ModelGroup> {
    let mut groups: Vec<ModelGroup> = Vec::new();
    for line in text.lines().skip(1) {
        let mut columns = line.split_whitespace();
        let (Some(provider), Some(model)) = (columns.next(), columns.next()) else {
            continue;
        };
        let id = format!("{provider}/{model}");
        let choice = ModelChoice {
            id: id.clone(),
            name: id,
            aliases: Vec::new(),
        };
        match groups
            .iter_mut()
            .find(|group| group.heading.as_deref() == Some(provider))
        {
            Some(group) => group.models.push(choice),
            None => groups.push(ModelGroup {
                heading: Some(provider.to_owned()),
                models: vec![choice],
            }),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::{list, parse};
    use crate::feat::harness::pi::runner::RunOutput;
    use crate::feat::harness::pi::runner::fake::FakeRunner;
    use crate::feat::harness::{ModelChoice, ModelGroup};

    const HEADER: &str =
        "provider       model                    context  max-out  thinking  images\n";

    /// Each group as its heading and model ids.
    fn ids(groups: &[ModelGroup]) -> Vec<(Option<&str>, Vec<&str>)> {
        groups
            .iter()
            .map(|group| {
                (
                    group.heading.as_deref(),
                    group.models.iter().map(|model| model.id.as_str()).collect(),
                )
            })
            .collect()
    }

    #[rstest::rstest]
    fn parse_skips_the_header() {
        // Given the header and one model.
        let text = format!(
            "{HEADER}anthropic      claude-haiku-4-5         200K     64K      yes       yes\n"
        );

        // When parsing it.
        let groups = parse(&text);

        // Then the one model is the only one, named by provider and model.
        assert_eq!(
            groups,
            [ModelGroup {
                heading: Some("anthropic".to_owned()),
                models: vec![ModelChoice {
                    id: "anthropic/claude-haiku-4-5".to_owned(),
                    name: "anthropic/claude-haiku-4-5".to_owned(),
                    aliases: Vec::new(),
                }],
            }],
            "the header should not be a model"
        );
    }

    #[rstest::rstest]
    fn parse_groups_by_provider_in_first_seen_order() {
        // Given two providers, the first one's models split by the second's.
        let text = format!(
            "{HEADER}\
             openai-codex   gpt-5.5                  272K     128K     yes       yes\n\
             anthropic      claude-haiku-4-5         200K     64K      yes       yes\n\
             openai-codex   gpt-6-astra              272K     128K     yes       yes\n"
        );

        // When parsing it.
        let groups = parse(&text);

        // Then each provider is one group, in the order first seen.
        assert_eq!(
            ids(&groups),
            [
                (
                    Some("openai-codex"),
                    vec!["openai-codex/gpt-5.5", "openai-codex/gpt-6-astra"]
                ),
                (Some("anthropic"), vec!["anthropic/claude-haiku-4-5"]),
            ],
            "models should group by provider in first-seen order"
        );
    }

    #[rstest::rstest]
    fn parse_keeps_slashes_and_colons_in_ids() {
        // Given an OpenRouter model whose name has a slash and a colon.
        let text =
            format!("{HEADER}openrouter     anthropic/claude-fable-5:batch  1M  128K  yes  yes\n");

        // When parsing it.
        let groups = parse(&text);

        // Then the id keeps them.
        assert_eq!(
            ids(&groups),
            [(
                Some("openrouter"),
                vec!["openrouter/anthropic/claude-fable-5:batch"]
            )],
            "the model's own slash and colon should be kept"
        );
    }

    #[rstest::rstest]
    #[case::with_output(
        Some(1),
        "No API key for any provider\n",
        "pi --list-models failed: No API key for any provider"
    )]
    #[case::silent_exit(Some(2), "", "pi --list-models failed: exit code 2")]
    #[case::signal(None, "", "pi --list-models failed: killed by a signal")]
    #[tokio::test]
    async fn list_failure_has_a_reason(
        #[case] code: Option<i32>,
        #[case] stderr: &str,
        #[case] expected: &str,
    ) {
        // Given pi failing to list its models.
        let runner = FakeRunner::new(RunOutput {
            code,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        });

        // When listing them.
        let result = list(&runner).await;

        // Then the reason says why.
        let reason = result
            .err()
            .and_then(|report| report.downcast_ref::<String>().cloned());
        assert_eq!(reason.as_deref(), Some(expected), "the failure reason");
    }
}
