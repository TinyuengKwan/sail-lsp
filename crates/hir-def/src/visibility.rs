//! Declaration privacy and shared project-module visibility queries.

use base_db::FileId;

/// Unresolved visibility as it appears in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawVisibility {
    /// The definition is visible everywhere (Sail default).
    #[default]
    Public,
    /// The definition is restricted to the declaring Sail module.
    Private,
}

/// Legacy file-only visibility for callers without a semantic database.
/// Project-aware callers must use `symbol_visible_from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// Unrestricted — visible everywhere.
    Public,
    /// Restricted to the declaring file.
    FilePrivate(FileId),
}

impl Visibility {
    /// Resolve a `RawVisibility` into a `Visibility`.
    pub fn resolve(raw: RawVisibility, declaring_file: FileId) -> Self {
        match raw {
            RawVisibility::Public => Visibility::Public,
            RawVisibility::Private => Visibility::FilePrivate(declaring_file),
        }
    }

    /// Whether this item is visible from the given file.
    pub fn is_visible_from(self, from_file: FileId) -> bool {
        match self {
            Visibility::Public => true,
            Visibility::FilePrivate(declaring_file) => from_file == declaring_file,
        }
    }

    /// Whether this item is visible from other files.
    pub fn is_visible_from_other_files(self) -> bool {
        matches!(self, Visibility::Public)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_is_visible_everywhere() {
        let vis = Visibility::Public;
        assert!(vis.is_visible_from(FileId::from_raw(0)));
        assert!(vis.is_visible_from(FileId::from_raw(1)));
        assert!(vis.is_visible_from(FileId::from_raw(999)));
        assert!(vis.is_visible_from_other_files());
    }

    #[test]
    fn private_is_only_visible_in_declaring_file() {
        let vis = Visibility::FilePrivate(FileId::from_raw(42));
        assert!(vis.is_visible_from(FileId::from_raw(42)));
        assert!(!vis.is_visible_from(FileId::from_raw(0)));
        assert!(!vis.is_visible_from(FileId::from_raw(43)));
        assert!(!vis.is_visible_from_other_files());
    }

    #[test]
    fn resolve_public() {
        let vis = Visibility::resolve(RawVisibility::Public, FileId::from_raw(0));
        assert_eq!(vis, Visibility::Public);
    }

    #[test]
    fn resolve_private() {
        let vis = Visibility::resolve(RawVisibility::Private, FileId::from_raw(5));
        assert_eq!(vis, Visibility::FilePrivate(FileId::from_raw(5)));
    }
}

/// The module graph and declaration privacy are shared by inference and IDE
/// queries. Without a manifest, files share Sail's global scope.
pub fn symbol_visible_from(
    db: &dyn salsa::Database,
    from: FileId,
    target: base_db::FileText,
    name: &str,
) -> bool {
    if base_db::module_graph::file_scope(db, from).is_none() {
        return true;
    }
    let target_id = target.file_id(db);
    if !base_db::module_graph::can_reference_file(db, from, target_id) {
        return false;
    }
    if base_db::module_graph::same_module(db, from, target_id) {
        return true;
    }
    !module_private_names(db, target).contains(name)
}

/// A private signature may live in another file of the implementation's
/// module. Project its privacy before exporting either declaration.
#[salsa::tracked(returns(ref))]
fn module_private_names(
    db: &dyn salsa::Database,
    file: base_db::FileText,
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let scope = base_db::module_graph::file_scope(db, file.file_id(db));
    let files = base_db::WorkspaceFiles::try_get(db).map(|ws| ws.file_texts(db));
    for &candidate in files.map(Vec::as_slice).unwrap_or(std::slice::from_ref(&file)) {
        if candidate == file
            || scope.as_ref().is_some_and(|s| s.own.contains(&candidate.file_id(db)))
        {
            names.extend(file_visibility(db, candidate).iter().filter_map(|(name, visibility)| {
                (*visibility == RawVisibility::Private).then(|| name.clone())
            }));
        }
    }
    names
}

#[salsa::tracked(returns(ref))]
fn file_visibility(
    db: &dyn salsa::Database,
    file: base_db::FileText,
) -> std::collections::HashMap<String, RawVisibility> {
    let mut result = std::collections::HashMap::new();
    if let Some(tree) = crate::def_query::file_item_tree(db, file) {
        for &item in tree.top_level_items() {
            let visibility = item.visibility(tree);
            let name = item.name(tree).to_string();
            // A val controls its implementation's visibility.
            if item.def_kind(tree) == crate::item_tree::DefKind::ValSpec {
                result.insert(name, visibility);
            } else {
                result.entry(name).or_insert(visibility);
            }
            for field in item.fields(tree) {
                result.entry(field.name.to_string()).or_insert(
                    if field.visibility == RawVisibility::Private {
                        RawVisibility::Private
                    } else {
                        visibility
                    },
                );
            }
            if let Some((name, _)) = item.scattered_member(tree) {
                result.entry(name.to_owned()).or_insert(visibility);
            }
        }
    }
    result
}

/// Definition candidates in semantic workspace order, before choosing a value
/// or type namespace. This is also the origin query for reference searches.
#[salsa::tracked(returns(ref))]
pub fn visible_files_for_name(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    from: FileId,
    name: crate::Name,
) -> Vec<base_db::FileText> {
    ws.file_texts(db)
        .iter()
        .copied()
        .filter(|&file| {
            base_db::module_graph::can_reference_file(db, from, file.file_id(db))
                && file_visibility(db, file).contains_key(name.as_str())
                && symbol_visible_from(db, from, file, name.as_str())
        })
        .collect()
}
