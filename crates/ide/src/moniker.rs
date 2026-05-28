//! Moniker — cross-workspace symbol identification.
//!
//! by assigning stable, unique identifiers to symbols.
//!
//! In Sail's flat file/$include model, a moniker path is:
//!   `project_name::file_stem::symbol_name`
//!
//! For example: `riscv::decode::execute` identifies the `execute`
//! function in `decode.sail` within the `riscv` project.

use ide_db::FileDb;

/// Kind of symbol in the moniker path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonikerDescriptorKind {
    /// File-level namespace (e.g., the .sail file).
    Namespace,
    /// Type definition (struct, enum, union, bitfield).
    Type,
    /// Value definition (register, let binding, val spec).
    Term,
    /// Function or mapping.
    Method,
}

/// A segment in a moniker path.
#[derive(Debug, Clone)]
pub struct MonikerDescriptor {
    pub name: String,
    pub desc: MonikerDescriptorKind,
}

/// Full moniker identifier (path from project root).
#[derive(Debug, Clone)]
pub struct MonikerIdentifier {
    /// Project or workspace name.
    pub project_name: String,
    /// Path segments from root to symbol.
    pub description: Vec<MonikerDescriptor>,
}

/// Whether the symbol is exported or imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonikerKind {
    /// Symbol defined in the current project (exported).
    Export,
    /// Symbol from an included library (imported).
    Import,
}

/// A resolved moniker for a symbol.
#[derive(Debug, Clone)]
pub struct Moniker {
    pub identifier: MonikerIdentifier,
    pub kind: MonikerKind,
}

/// Result of moniker resolution.
#[derive(Debug, Clone)]
pub enum MonikerResult {
    /// Non-local definition with a full moniker.
    Moniker(Moniker),
    /// Local variable or pattern binding (no stable moniker).
    Local,
}

/// Resolve the moniker for the symbol at the given offset.
/// Algorithm:
/// 1. Find token at offset
/// 2. Determine if it's a definition or reference
/// 3. Build moniker path: project → file → symbol
pub fn moniker(
    file: &dyn FileDb,
    file_url: &url::Url,
    offset: usize,
    project_name: &str,
) -> Option<MonikerResult> {
    let _text = file.text();
    let tokens = file.tokens()?;

    // Find token at offset
    let (token, _span) =
        tokens.iter().find(|(_, span)| span.start <= offset && offset <= span.end)?;

    // Only identifiers can have monikers
    let name = match token {
        parser::Token::Id(name) => name.clone(),
        _ => return None,
    };

    // Check if this is a local binding (function parameter, let var)
    if let Some(parsed) = file.parsed() {
        for occ in &parsed.symbol_occurrences {
            if occ.name == name && occ.span.start <= offset && offset <= occ.span.end
                && occ.scope == Some(syntax::parser_lower::Scope::Local) {
                    return Some(MonikerResult::Local);
                }
        }
    }

    // Build moniker path: project → file stem → symbol
    let file_stem = file_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("unknown")
        .strip_suffix(".sail")
        .unwrap_or("unknown");

    // Determine descriptor kind from ItemTree
    let desc_kind = if let Some(item_tree) = file.item_tree() {
        item_tree
            .top_level_items()
            .iter()
            .find(|id| id.name(item_tree).as_str() == name)
            .map(|id| match id.item_kind(item_tree) {
                hir_def::item_tree::ItemKind::Function
                | hir_def::item_tree::ItemKind::Mapping
                | hir_def::item_tree::ItemKind::ValSpec
                | hir_def::item_tree::ItemKind::MappingSpec => MonikerDescriptorKind::Method,
                hir_def::item_tree::ItemKind::Struct
                | hir_def::item_tree::ItemKind::Union
                | hir_def::item_tree::ItemKind::Enum
                | hir_def::item_tree::ItemKind::Bitfield
                | hir_def::item_tree::ItemKind::Newtype
                | hir_def::item_tree::ItemKind::TypeAlias => MonikerDescriptorKind::Type,
                hir_def::item_tree::ItemKind::Register
                | hir_def::item_tree::ItemKind::Let
                | hir_def::item_tree::ItemKind::Var => MonikerDescriptorKind::Term,
                _ => MonikerDescriptorKind::Term,
            })
            .unwrap_or(MonikerDescriptorKind::Term)
    } else {
        MonikerDescriptorKind::Term
    };

    Some(MonikerResult::Moniker(Moniker {
        identifier: MonikerIdentifier {
            project_name: project_name.to_string(),
            description: vec![
                MonikerDescriptor {
                    name: file_stem.to_string(),
                    desc: MonikerDescriptorKind::Namespace,
                },
                MonikerDescriptor { name, desc: desc_kind },
            ],
        },
        kind: MonikerKind::Export, // All symbols in workspace are exports
    }))
}

impl MonikerIdentifier {
    /// Encode as a SCIP symbol string.
    /// SCIP symbol format: `scheme manager package-name version descriptor...`
    /// Example: `sail-lsp sail riscv 0.1 decode/execute().`
    pub fn to_scip_symbol(&self) -> String {
        let mut result = String::from("sail-lsp sail ");
        result.push_str(&self.project_name);
        result.push(' ');
        for desc in &self.description {
            match desc.desc {
                MonikerDescriptorKind::Namespace => {
                    result.push_str(&desc.name);
                    result.push('/');
                }
                MonikerDescriptorKind::Type => {
                    result.push_str(&desc.name);
                    result.push('#');
                }
                MonikerDescriptorKind::Term => {
                    result.push_str(&desc.name);
                    result.push('.');
                }
                MonikerDescriptorKind::Method => {
                    result.push_str(&desc.name);
                    result.push_str("().");
                }
            }
        }
        result
    }
}

impl MonikerDescriptorKind {
    /// Convert to SCIP SymbolInformation kind string.
    pub fn scip_kind(&self) -> &'static str {
        match self {
            MonikerDescriptorKind::Namespace => "Namespace",
            MonikerDescriptorKind::Type => "Type",
            MonikerDescriptorKind::Term => "Term",
            MonikerDescriptorKind::Method => "Method",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn moniker_for_function() {
        let file = TestFile::new("function execute(x) = x + 1\n");
        let url = url::Url::parse("file:///project/decode.sail").unwrap();
        let result = moniker(&file, &url, 9, "riscv"); // offset at "execute"
        let Some(MonikerResult::Moniker(m)) = result else {
            panic!("expected moniker, got {:?}", result);
        };
        assert_eq!(m.identifier.project_name, "riscv");
        assert_eq!(m.identifier.description.len(), 2);
        assert_eq!(m.identifier.description[0].name, "decode");
        assert_eq!(m.identifier.description[1].name, "execute");
        assert_eq!(m.identifier.description[1].desc, MonikerDescriptorKind::Method);
        assert_eq!(m.kind, MonikerKind::Export);
    }

    #[test]
    fn scip_symbol_encoding() {
        let id = MonikerIdentifier {
            project_name: "riscv".to_string(),
            description: vec![
                MonikerDescriptor {
                    name: "decode".to_string(),
                    desc: MonikerDescriptorKind::Namespace,
                },
                MonikerDescriptor {
                    name: "execute".to_string(),
                    desc: MonikerDescriptorKind::Method,
                },
            ],
        };
        assert_eq!(id.to_scip_symbol(), "sail-lsp sail riscv decode/execute().");
    }

    #[test]
    fn moniker_for_non_ident_is_none() {
        let file = TestFile::new("function f(x) = x + 1\n");
        let url = url::Url::parse("file:///project/test.sail").unwrap();
        // offset at "=" (not an identifier — returns None)
        let result = moniker(&file, &url, 15, "riscv");
        assert!(result.is_none(), "non-ident should return None");
    }
}
