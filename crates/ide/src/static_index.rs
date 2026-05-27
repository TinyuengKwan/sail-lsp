//! Pre-computed static index of all definitions and references.
//!
//! Enables SCIP/LSIF export by collecting all tokens across all
//! files with their hover, definition, references, and moniker data.
//!
//! Data flow:
//!   `compute()` → `add_file()` per file → resolve tokens →
//!   store in `TokenStore` with dedup via `def_map`.

use std::collections::HashMap;

use base_db::TextRange;
use ide_db::ide_types::SymbolKind;
use ide_db::FileDb;

use crate::moniker::{self, MonikerResult};

/// Opaque handle into the `TokenStore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(usize);

/// A single reference to a definition.
#[derive(Debug, Clone)]
pub struct ReferenceData {
    pub url: url::Url,
    pub range: TextRange,
    pub is_definition: bool,
}

/// Per-token static data stored in the index.
#[derive(Debug, Clone)]
pub struct TokenStaticData {
    /// Hover text (signature + docs).
    pub hover: Option<String>,
    /// Location of the definition.
    pub definition: Option<(url::Url, TextRange)>,
    /// All reference sites.
    pub references: Vec<ReferenceData>,
    /// SCIP/LSIF moniker.
    pub moniker: Option<MonikerResult>,
    /// Human-readable display name.
    pub display_name: Option<String>,
    /// Symbol kind for SCIP kind field.
    pub kind: SymbolKind,
}

/// Dense storage of `TokenStaticData`, indexed by `TokenId`.
#[derive(Debug, Default)]
pub struct TokenStore(Vec<TokenStaticData>);

impl TokenStore {
    pub fn insert(&mut self, data: TokenStaticData) -> TokenId {
        let id = TokenId(self.0.len());
        self.0.push(data);
        id
    }

    pub fn get(&self, id: TokenId) -> Option<&TokenStaticData> {
        self.0.get(id.0)
    }

    pub fn get_mut(&mut self, id: TokenId) -> Option<&mut TokenStaticData> {
        self.0.get_mut(id.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TokenId, &TokenStaticData)> {
        self.0.iter().enumerate().map(|(i, d)| (TokenId(i), d))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Per-file portion of the static index.
#[derive(Debug)]
pub struct StaticIndexedFile {
    pub url: url::Url,
    /// (token range, token id) pairs for this file.
    pub tokens: Vec<(TextRange, TokenId)>,
}

/// Pre-computed index of all definitions and references.
pub struct StaticIndex {
    pub files: Vec<StaticIndexedFile>,
    pub tokens: TokenStore,
    /// Definition dedup map: symbol name + file → TokenId.
    def_map: HashMap<(String, String), TokenId>,
}

impl StaticIndex {
    /// Compute the static index over all workspace files.
    pub fn compute(all_files: &[(&url::Url, &dyn FileDb)], project_name: &str) -> Self {
        let mut index = StaticIndex {
            files: Vec::new(),
            tokens: TokenStore::default(),
            def_map: HashMap::new(),
        };
        for (url, file) in all_files {
            index.add_file(url, *file, project_name);
        }
        index
    }

    /// Index a single file.
    fn add_file(&mut self, url: &url::Url, file: &dyn FileDb, project_name: &str) {
        let Some(parsed) = file.parsed() else { return };
        let _text = file.text();

        let file_stem = url
            .path_segments()
            .and_then(|s| s.last())
            .unwrap_or("unknown")
            .strip_suffix(".sail")
            .unwrap_or("unknown");

        let mut file_tokens: Vec<(TextRange, TokenId)> = Vec::new();

        // Walk all top-level declarations
        for decl in &parsed.decls {
            if decl.scope != syntax::parser_lower::Scope::TopLevel {
                continue;
            }

            let def_key = (decl.name.clone(), file_stem.to_string());
            let range = base_db::text_range(decl.span.start, decl.span.end);

            // Dedup: reuse existing TokenId if we've seen this def before
            if let Some(&existing_id) = self.def_map.get(&def_key) {
                // Add a reference to the existing token
                if let Some(data) = self.tokens.get_mut(existing_id) {
                    data.references.push(ReferenceData {
                        url: url.clone(),
                        range,
                        is_definition: true,
                    });
                }
                file_tokens.push((range, existing_id));
                continue;
            }

            // Resolve moniker for this definition
            let moniker_result = moniker::moniker(file, url, decl.span.start, project_name);

            // Determine symbol kind
            let kind = decl_kind_to_symbol_kind(decl.kind);

            // Build hover text from signature index
            let hover = file
                .signature_index()
                .and_then(|idx| idx.get(&decl.name))
                .map(|sig| sig.label.clone());

            let token_id = self.tokens.insert(TokenStaticData {
                hover,
                definition: Some((url.clone(), range)),
                references: vec![ReferenceData { url: url.clone(), range, is_definition: true }],
                moniker: moniker_result,
                display_name: Some(decl.name.clone()),
                kind,
            });

            self.def_map.insert(def_key, token_id);
            file_tokens.push((range, token_id));
        }

        // Walk symbol occurrences (references) and link to definitions
        for occ in &parsed.symbol_occurrences {
            let ref_range = base_db::text_range(occ.span.start, occ.span.end);
            let is_def = occ
                .role
                .as_ref()
                .is_some_and(|r| matches!(r, syntax::parser_lower::DeclRole::Definition));

            // Try to find the definition in def_map by name
            // (simplified: search across all file stems)
            let token_id = self.find_def_token(&occ.name);
            if let Some(tid) = token_id {
                if let Some(data) = self.tokens.get_mut(tid) {
                    data.references.push(ReferenceData {
                        url: url.clone(),
                        range: ref_range,
                        is_definition: is_def,
                    });
                }
                file_tokens.push((ref_range, tid));
            }
        }

        self.files.push(StaticIndexedFile { url: url.clone(), tokens: file_tokens });
    }

    /// Look up a definition token by name (across all file stems).
    fn find_def_token(&self, name: &str) -> Option<TokenId> {
        self.def_map.iter().find(|((n, _), _)| n == name).map(|(_, &tid)| tid)
    }
}

/// Map parser DeclKind to SymbolKind.
fn decl_kind_to_symbol_kind(kind: syntax::parser_lower::DeclKind) -> SymbolKind {
    use syntax::parser_lower::DeclKind;
    match kind {
        DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload => {
            SymbolKind::Function
        }
        DeclKind::Struct | DeclKind::Bitfield | DeclKind::Newtype => SymbolKind::Struct,
        DeclKind::Union | DeclKind::Type => SymbolKind::TypeAlias,
        DeclKind::Enum => SymbolKind::Enum,
        DeclKind::EnumMember => SymbolKind::EnumMember,
        DeclKind::Register => SymbolKind::Variable,
        DeclKind::Let | DeclKind::Var => SymbolKind::Variable,
        DeclKind::Parameter => SymbolKind::Variable,
    }
}
