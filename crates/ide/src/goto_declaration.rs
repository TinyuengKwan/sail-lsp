//! Go to declaration — resolves the declaration site (val spec) of a symbol.

use ide_db::ide_types::FileLocation;
use ide_db::workspace_index::SymbolIndex;
use url::Url;

pub use crate::navigation::symbol_declaration_locations;

/// Go to declaration using the workspace symbol index (O(1) lookup).
pub fn goto_declaration(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
) -> Vec<FileLocation> {
    crate::navigation::declaration_locations_indexed(index, symbol_key, uri_hint)
}
