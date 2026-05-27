//! Import candidate search for auto-include.
//! Given an unresolved name, searches the workspace symbol index for
//! definitions that could satisfy it, along with the `$include` path
//! needed to bring them into scope.

use crate::workspace_index::SymbolIndex;

/// A candidate for auto-import.
#[derive(Debug, Clone)]
pub struct LocatedImport {
    /// The file path (relative) that defines this symbol.
    pub include_path: String,
    /// The symbol name.
    pub name: String,
    /// The symbol's signature text (for display in completion).
    pub signature: Option<String>,
}

/// Search the workspace for symbols matching a query.
/// Returns a list of `LocatedImport` candidates, each with the
/// `$include` path needed to import the symbol.
pub fn search_for_imports(index: &SymbolIndex, query: &str) -> Vec<LocatedImport> {
    let entries = index.find(query);
    entries
        .iter()
        .map(|entry| LocatedImport {
            include_path: entry.url.path().to_string(),
            name: entry.name.clone(),
            signature: Some(entry.signature_text.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Integration tests require a populated SymbolIndex,
    // which needs a full workspace scan. Tested via ide-level tests.
}
