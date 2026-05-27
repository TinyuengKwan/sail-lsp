//! Chaining hints: show intermediate types in field access chains.
//! For `x.field1.field2`, shows type hint after `x.field1`.

use super::*;

/// Collect chaining hints for field access expressions.
///
/// Only emits hints when there are 2+ consecutive field accesses.
pub(super) fn collect_chaining_hints(
    _all_files: &[(&Url, &dyn FileDb)],
    _current_file: &dyn FileDb,
    _begin: usize,
    _end: usize,
    _hints: &mut Vec<IdeDbInlayHint>,
    _config: &InlayHintsConfig,
) {
    // Field chaining in Sail: x.field1.field2
    // For each FIELD_ACCESS_EXPR that is itself inside another FIELD_ACCESS_EXPR,
    // show the intermediate type.
    //
    // Implementation: walk CST looking for nested FIELD_ACCESS_EXPR nodes.
    // For now, this is a structural placeholder — actual type inference
    // integration requires passing InferenceResult.
}
