//! Claude Code's models and permission modes, as the pickers offer them.

use crate::feat::harness::{HarnessId, HarnessInfo, ModelChoice, ModelGroup};

/// A Claude model a draft can pick: the full ID passed as `--model`, the
/// name shown for it, and the other values T3 Code's manifest maps to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    /// Other names for the model, such as `opus`, that a draft or thread may
    /// have stored.
    pub aliases: &'static [&'static str],
}

/// The current models, in T3 Code's order.
pub const MODELS: [Model; 4] = [
    Model {
        id: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        aliases: &["opus-5.5", "claude-opus-5.5"],
    },
    Model {
        id: "claude-fable-5-1",
        name: "Claude Fable 5.1",
        aliases: &["fable", "fable-5.1", "claude-fable-5.1"],
    },
    Model {
        id: "claude-opus-5",
        name: "Claude Opus 5",
        aliases: &["opus", "opus-5", "claude-opus-5.0", "claude-opus-5-0"],
    },
    Model {
        id: "claude-sonnet-5",
        name: "Claude Sonnet 5",
        aliases: &[
            "sonnet",
            "sonnet-5",
            "claude-sonnet-5.0",
            "claude-sonnet-5-0",
        ],
    },
];

/// The older models T3 Code files under "Legacy models", in its order.
pub const LEGACY_MODELS: [Model; 7] = [
    Model {
        id: "claude-fable-5",
        name: "Claude Fable 5",
        aliases: &[],
    },
    Model {
        id: "claude-opus-4-8",
        name: "Claude Opus 4.8",
        aliases: &["opus-4.8", "claude-opus-4.8"],
    },
    Model {
        id: "claude-opus-4-7",
        name: "Claude Opus 4.7",
        aliases: &["opus-4.7", "claude-opus-4.7"],
    },
    Model {
        id: "claude-opus-4-6",
        name: "Claude Opus 4.6",
        aliases: &["opus-4.6", "claude-opus-4.6", "claude-opus-4-6-20251117"],
    },
    Model {
        id: "claude-opus-4-5",
        name: "Claude Opus 4.5",
        aliases: &[],
    },
    Model {
        id: "claude-sonnet-4-6",
        name: "Claude Sonnet 4.6",
        aliases: &[
            "sonnet-4.6",
            "claude-sonnet-4.6",
            "claude-sonnet-4-6-20251117",
        ],
    },
    Model {
        id: "claude-haiku-4-5",
        name: "Claude Haiku 4.5",
        aliases: &[
            "haiku",
            "haiku-4.5",
            "claude-haiku-4.5",
            "claude-haiku-4-5-20251001",
        ],
    },
];

/// The `--permission-mode` values a draft can pick, besides Claude's default.
pub const PERMISSION_MODES: [&str; 6] = [
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "manual",
    "dontAsk",
    "plan",
];

/// Claude Code as the pickers show it: the current models, the legacy
/// models under their heading, and the permission modes.
pub fn info() -> HarnessInfo {
    HarnessInfo {
        id: HarnessId::new(super::ID),
        label: super::LABEL.to_owned(),
        tag: None,
        icon: Some("✳".to_owned()),
        unavailable: None,
        models: vec![
            ModelGroup {
                heading: None,
                models: choices(&MODELS),
            },
            ModelGroup {
                heading: Some("Legacy models".to_owned()),
                models: choices(&LEGACY_MODELS),
            },
        ],
        permission_modes: PERMISSION_MODES.map(str::to_owned).to_vec(),
        nudge_on_attach: false,
        notice: None,
    }
}

/// `models` as the harness-neutral choices a picker lists.
fn choices(models: &[Model]) -> Vec<ModelChoice> {
    models
        .iter()
        .map(|model| ModelChoice {
            id: model.id.to_owned(),
            name: model.name.to_owned(),
            aliases: model
                .aliases
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
        })
        .collect()
}
