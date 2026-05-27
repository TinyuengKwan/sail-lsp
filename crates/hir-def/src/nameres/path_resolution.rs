//! Path resolution within the DefMap.
//! Given a name path, resolves it through the DefMap's ItemScope.
//! In Sail there are no module paths or use-statement resolution —
//! all paths are single identifiers resolved in the current scope.

use super::DefMap;
use crate::name::Name;
use crate::per_ns::PerNs;

/// Result of resolving a path in the DefMap.
#[derive(Debug, Clone)]
pub struct ResolvePathResult {
    /// The resolved definition(s) in each namespace.
    pub resolved: PerNs,
    /// Remaining unresolved segments (always empty in Sail since
    /// paths are single identifiers, but included for RA compat).
    pub remaining: Option<usize>,
}

impl DefMap {
    /// Resolve a single name in this DefMap's root module scope.
    ///
    /// Simplified for Sail: always a single-segment path.
    pub fn resolve_path(&self, name: &Name) -> ResolvePathResult {
        let resolved = self.root_scope().get(name);
        ResolvePathResult { resolved, remaining: None }
    }
}
