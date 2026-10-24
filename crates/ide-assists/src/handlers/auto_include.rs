//! Missing-include quick fixes share candidate search with completion.
use crate::assist_context::{AssistContext, Assists};
use ide_db::imports::import_assets;

pub(crate) fn auto_include(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let (db, file) = ctx.file.file_text()?;
    let (name, _) = import_assets::reference_at(db, file, ctx.offset())?;
    let mut added = false;
    for candidate in import_assets::search(db, file, ctx.offset(), &name, true) {
        let Some((candidate, edit)) = import_assets::resolve(db, &candidate.request) else {
            continue;
        };
        acc.add_import(candidate, ctx.range, edit);
        added = true;
    }
    added.then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn no_placeholder_when_no_candidate_exists() {
        let src = "let x = unknown_fn()\n";
        assert!(assist_labels(auto_include, src, src.find("unknown_fn").unwrap()).is_empty());
    }
}
