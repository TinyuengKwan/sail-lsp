//! Cached callable metadata used by semantic queries.

use base_db::FileText;
use hir_def::callable_info::CallableSignature;
use std::{collections::HashMap, sync::Arc};
use syntax::parse_query::parsed_file;

/// Salsa tracked function: build the per-file signature index.
///
/// Maps callable name → CallableSignature. Used by completion,
/// hover, signature help, and many other IDE features.
///
/// This query reads parsed heads and their source text. Salsa caches the
/// resulting index; unchanged signatures do not invalidate its dependents.
#[salsa::tracked(returns(ref))]
pub fn signature_index(
    db: &dyn salsa::Database,
    input: FileText,
) -> Arc<HashMap<String, CallableSignature>> {
    let pf = parsed_file(db, input);
    let text = input.text(db);
    match pf.as_ref() {
        Some(pf) => Arc::new(hir_def::callable_info::build_signature_index_from_parsed(
            &pf.0,
            text.as_ref(),
        )),
        None => Arc::new(HashMap::new()),
    }
}
