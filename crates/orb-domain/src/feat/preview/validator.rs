//! Checks whether the user can yank or fold the preview's cursor block.

use wherror::Error;

use crate::AppState;

/// Why yanking can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum YankError {
    /// The preview has no blocks.
    NoBlock,
}

/// Why folding can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum FoldError {
    /// The preview has no blocks.
    NoBlock,
    /// The cursor's block has nothing to fold.
    NotFoldable,
}

/// Allow yanking only with a block under the cursor.
///
/// # Errors
///
/// Returns [`YankError::NoBlock`] when the preview has no blocks.
pub fn validate_yank(state: &AppState) -> Result<(), YankError> {
    match state.preview.cursor_block() {
        None => Err(YankError::NoBlock),
        Some(_) => Ok(()),
    }
}

/// Allow folding only a block under the cursor that folds.
///
/// # Errors
///
/// Returns [`FoldError::NoBlock`] when the preview has no blocks, and
/// [`FoldError::NotFoldable`] when the cursor's block doesn't fold.
pub fn validate_toggle_fold(state: &AppState) -> Result<(), FoldError> {
    match state.preview.cursor_block() {
        None => Err(FoldError::NoBlock),
        Some(block) if !block.foldable() => Err(FoldError::NotFoldable),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{FoldError, YankError, validate_toggle_fold, validate_yank};
    use crate::AppState;
    use crate::feat::preview::block::{Block, BlockId, BlockKind};
    use crate::feat::preview::state::Preview;

    fn showing(blocks: Vec<Block>) -> AppState {
        AppState {
            preview: Preview {
                blocks: blocks.into(),
                ..Preview::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn yank_rejected_without_blocks() {
        // Given a preview with no blocks.
        let state = AppState::default();

        // When validating yank.
        let result = validate_yank(&state);

        // Then validation fails with NoBlock.
        assert_eq!(result, Err(YankError::NoBlock), "yank needs a block");
    }

    #[rstest::rstest]
    #[case(vec![], FoldError::NoBlock)]
    #[case(
        vec![Block { id: BlockId(0), parts: 1, kind: BlockKind::Claude("Done.".into()) }],
        FoldError::NotFoldable,
    )]
    fn toggle_fold_rejected(#[case] blocks: Vec<Block>, #[case] error: FoldError) {
        // Given a preview whose cursor block can't fold.
        let state = showing(blocks);

        // When validating toggle fold.
        let result = validate_toggle_fold(&state);

        // Then validation fails with the reason.
        assert_eq!(result, Err(error), "fold needs a foldable block");
    }
}
