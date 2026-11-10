//! Handler for `AnyDiagnostic::InactiveCode`.
//!
//! rust-analyzer's `inactive_code`: code under a `#[cfg]` that is off is
//! reported as a faded hint, not a problem. Here the region is what an
//! `$ifdef`/`$ifndef`/`$iftarget`/`$else` the preprocessor did not take
//! encloses (`preprocess.ml`).

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, InactiveCode};

pub(crate) fn inactive_code(_ctx: &DiagnosticsContext<'_>, d: &InactiveCode) -> Diagnostic {
    let message = if d.in_else {
        format!("code is inactive: `{}` is true, this is its `$else` branch", d.directive)
    } else {
        format!("code is inactive: `{}` is false", d.directive)
    };
    Diagnostic::new(DiagnosticCode::SailLint("inactive-code", Severity::Hint), message, d.range)
        .with_unused(true)
}

#[cfg(test)]
mod tests {
    use crate::tests::check_dispatch;
    use hir::diagnostics::*;

    fn region(directive: &str, in_else: bool) -> AnyDiagnostic {
        AnyDiagnostic::InactiveCode(InactiveCode {
            directive: directive.to_owned(),
            in_else,
            range: hir::FileRange {
                file_id: base_db::FileId::from_raw(0),
                range: base_db::text_range(9, 20),
            },
        })
    }

    #[test]
    fn a_hint_tagged_unnecessary_that_names_the_condition() {
        let d = check_dispatch(&region("$ifdef FOO", false)).unwrap();
        assert_eq!(d.code.as_str(), "inactive-code");
        assert_eq!(d.severity, Severity::Hint);
        assert!(d.unused);
        assert_eq!(d.message, "code is inactive: `$ifdef FOO` is false");
        assert_eq!(d.range.range, base_db::text_range(9, 20));

        let d = check_dispatch(&region("$iftarget c", true)).unwrap();
        assert_eq!(
            d.message,
            "code is inactive: `$iftarget c` is true, this is its `$else` branch"
        );
    }
}
