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
        let Some(tree) = file.item_tree() else { return };

        let file_stem = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("unknown")
            .strip_suffix(".sail")
            .unwrap_or("unknown");

        let mut file_tokens: Vec<(TextRange, TokenId)> = Vec::new();

        // Every definition in the file, from the item tree. `top_level_items`
        // is top level by construction, so the scope filter is gone with the
        // scrape.
        for &id in tree.top_level_items() {
            let decl_name = id.name(tree).to_string();
            let decl_span = id.span(tree);
            let def_key = (decl_name.clone(), file_stem.to_string());
            let range = base_db::text_range(decl_span.start, decl_span.end);

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
            let moniker_result = moniker::moniker(file, url, decl_span.start, project_name);
            let kind = def_kind_to_symbol_kind(id.def_kind(tree));

            // Build hover text from signature index
            let hover = file
                .signature_index()
                .and_then(|idx| idx.get(&decl_name))
                .map(|sig| sig.label.clone());

            let token_id = self.tokens.insert(TokenStaticData {
                hover,
                definition: Some((url.clone(), range)),
                references: vec![ReferenceData { url: url.clone(), range, is_definition: true }],
                moniker: moniker_result,
                display_name: Some(decl_name.clone()),
                kind,
            });

            self.def_map.insert(def_key, token_id);
            file_tokens.push((range, token_id));
        }

        // Every call site, from the callgraph built out of the lowered
        // bodies. The scrape offered every name-shaped occurrence in the
        // file, resolved or not.
        let sites: Vec<(String, base_db::TextRange)> = file
            .callgraph()
            .map(|g| {
                g.call_sites_in_file()
                    .map(|s| {
                        (
                            s.callee.clone(),
                            base_db::text_range(s.callee_span.start, s.callee_span.end),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (name, ref_range) in sites {
            let is_def = false;
            let token_id = self.find_def_token(&name);
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

/// How a definition of this kind appears in a static index.
fn def_kind_to_symbol_kind(kind: hir::DefKind) -> SymbolKind {
    use hir::DefKind as D;
    match kind {
        D::Function
        | D::ValSpec
        | D::Mapping
        | D::MappingSpec
        | D::Overload
        | D::Outcome
        | D::ScatteredHead
        | D::ScatteredClause => SymbolKind::Function,
        D::Struct | D::Bitfield | D::Newtype => SymbolKind::Struct,
        D::Union | D::TypeAlias => SymbolKind::TypeAlias,
        D::Enum => SymbolKind::Enum,
        D::Variant => SymbolKind::EnumMember,
        D::Field => SymbolKind::Field,
        D::Register | D::Let | D::Var => SymbolKind::Variable,
        D::EndMarker | D::Constraint | D::TerminationMeasure | D::Instantiation => {
            SymbolKind::Other
        }
    }
}
