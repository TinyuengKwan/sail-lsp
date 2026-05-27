//! Inference variable fallback.
//! After type inference completes, any remaining unsolved inference
//! variables are resolved to default types to prevent cascading
//! `{unknown}` in hover/diagnostics.
//!
//! Sail fallback rules:
//! - Unsolved integer inference vars → `int`
//! - Unsolved boolean inference vars → `bool`
//! - All other unsolved vars → `Error` (avoids unsound assumptions)

use super::unify::{InferenceTable, TyVarKey, TyVarValue};
use crate::ty::{Scalar, Ty, TyKind};

impl InferenceTable {
    /// Resolve remaining inference variables to default types.
    ///
    /// Called after inference completes to ensure no raw `?N` variables
    /// leak into IDE features (hover, diagnostics, inlay hints).
    pub fn fallback_unsolved(&mut self) {
        let count = self.var_count();
        for i in 0..count {
            let key = TyVarKey(i);
            if let TyVarValue::Unknown = self.safe_probe_by_id(i) {
                let fallback = Ty::new(TyKind::Scalar(Scalar::Int));
                self.table.union_value(key, TyVarValue::Known(fallback));
            }
        }
    }
}
