//! Re-exports various subcrates databases so that the calling code can depend
//! only on `hir`. This breaks abstraction boundary a bit, it would be cool if
//! we didn't do that.

pub use hir_def::db::DefDatabase;
pub use hir_ty::db::HirDatabase;
