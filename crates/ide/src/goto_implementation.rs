//! Go to implementation — finds function/mapping implementations.

use ide_db::ide_types::FileLocation;
use ide_db::workspace_index::SymbolIndex;
use url::Url;

pub use crate::navigation::implementation_locations;

/// Go to implementation using the workspace symbol index (O(1) lookup).
pub fn goto_implementation(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
) -> Vec<FileLocation> {
    crate::navigation::implementation_locations_indexed(index, symbol_key, uri_hint)
}
