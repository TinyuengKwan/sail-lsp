//! An infix operator has a fixity, or upstream refuses the program.
//!
//! `raise (err_general (Range (s, e)) ("Undeclared fixity for operator " ^
//! op))` (`initial_check.ml:308`), reached whenever the infix parser meets
//! an operator it cannot give a precedence to. Two sets need no
//! declaration: the ones the infix grammar hardcodes as productions
//! (`initial_check.ml:291-303`) and the default fixity table it starts
//! with (`:2411-2422`). Everything else needs `infix`, `infixl` or
//! `infixr`.
//!
//! The parser here stays lenient on purpose — `infix_bp` gives an
//! undeclared operator level 4 so the expression still builds a tree,
//! which is what an editor needs and what upstream, compiling a whole
//! program, does not. Reporting is this file's job.
//!
//! It is not the parser's job for a second reason: the parser is handed a
//! `FixityContext` and cannot tell "this workspace declares nothing" from
//! "nobody gave me the declarations". `WorkspaceFixities` is the table
//! every file's parse was run with, so asking it here is asking the
//! question that has an answer.

use crate::types::DiagnosticCode;
use crate::Diagnostic;
use ide_db::FileDb;
use syntax::SyntaxKind as SK;

/// Operators upstream's infix parser resolves without a declaration:
/// `initial_check.ml:291-303` maps these to dedicated grammar tokens, and
/// `:2411-2422` seeds the fixity table with the rest.
const BUILT_IN: &[&str] =
    &["+", "-", "*", "<", ">", "<=", ">=", "::", "@", "in", "^", "|", "&", "==", "!=", "/", "%"];

pub(crate) fn undeclared_fixity(db: &dyn salsa::Database, file: &dyn FileDb) -> Vec<Diagnostic> {
    let Some(root) = file.syntax() else { return Vec::new() };
    let declared = base_db::WorkspaceFixities::try_get(db).map(|f| f.fixities(db).clone());
    // No table at all means nobody has scanned the workspace yet, and an
    // operator this file declares itself would read as undeclared. Say
    // nothing rather than guess: `WorkspaceFixities` is set before any
    // file is parsed on the server's path.
    let Some(declared) = declared else { return Vec::new() };

    let mut res = Vec::new();
    for token in root.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if token.kind() != SK::OP_IDENT {
            continue;
        }
        // Only in operator position. The same token kind spells the
        // operator being *declared* in a `FIXITY_DEF`, and declaring one
        // is how it stops being undeclared.
        if token.parent().is_none_or(|p| p.kind() != SK::BIN_EXPR) {
            continue;
        }
        let op = token.text();
        if BUILT_IN.contains(&op) || declared.contains_key(op) {
            continue;
        }
        res.push(Diagnostic::new(
            DiagnosticCode::SailError("undeclared-fixity"),
            format!("Undeclared fixity for operator {op}"),
            token.text_range(),
        ));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    fn messages(source: &str) -> Vec<String> {
        let file = TestFile::new(source);
        undeclared_fixity(file.db(), &file).into_iter().map(|d| d.message).collect()
    }

    /// Upstream's own `test/lexing/unknown_operator.sail`.
    #[test]
    fn an_undeclared_operator_is_reported() {
        assert_eq!(
            messages("default Order dec\nfunction main() -> unit = {\n  let _ = x <=|=> y;\n}\n"),
            ["Undeclared fixity for operator <=|=>"]
        );
    }

    /// Declaring it is how it stops being undeclared, and the declaration
    /// itself spells the operator with the same token kind.
    #[test]
    fn a_declared_operator_is_not() {
        assert_eq!(
            messages(
                "default Order dec\ninfix 4 <=|=>\n\
                 function main() -> unit = {\n  let _ = x <=|=> y;\n}\n"
            ),
            Vec::<String>::new()
        );
    }

    /// The two sets upstream resolves without a declaration: the hardcoded
    /// grammar productions and the default fixity table. `overload
    /// operator ^^^ = operator ^` is *not* one of them — upstream says so
    /// on `test/format/rv_gfmul.sail`.
    #[test]
    fn a_built_in_operator_needs_no_declaration() {
        assert_eq!(
            messages(
                "default Order dec\nfunction main() -> unit = {\n\
                   let _ = (a | b) & (c == d) & (e / f) & (g % h) & (i ^ j);\n}\n"
            ),
            Vec::<String>::new()
        );
    }
}
