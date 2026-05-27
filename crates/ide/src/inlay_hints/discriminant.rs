//! Discriminant hints: show enum variant numeric values.
//! For `enum color = { Red, Green, Blue }`, shows `= 0`, `= 1`, `= 2`.

use super::*;

/// Collect discriminant value hints for enum variants.
pub(super) fn collect_discriminant_hints(
    _current_file: &dyn FileDb,
    _begin: usize,
    _end: usize,
    _hints: &mut Vec<IdeDbInlayHint>,
    _config: &InlayHintsConfig,
) {
    // Walk ItemTree looking for enum definitions.
    // For each variant, compute and display its discriminant value.
    //
    // Sail enums: `enum color = { Red, Green, Blue }`
    // Implicit discriminants: Red=0, Green=1, Blue=2
    // Explicit: `enum status = { Ok = 0, Error = 1 }`
}
