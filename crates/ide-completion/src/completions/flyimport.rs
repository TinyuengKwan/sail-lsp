//! Candidate search is shared with the missing-include assist. Edits are
//! computed only when the client resolves a chosen completion.
use super::Completions;
use crate::context::{CompletionContext, CompletionPosition};
use crate::item::{CompletionItem, CompletionRelevance};
use ide_db::{imports::import_assets, FileDb};
use url::Url;

pub(crate) fn import_on_the_fly(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    _all_files: &[(&Url, &dyn FileDb)],
    _current_uri: &Url,
) {
    if ctx.position == CompletionPosition::Pattern || ctx.prefix.len() < 2 {
        return;
    }
    let Some(file) = ctx.file_text else { return };
    for candidate in import_assets::search(ctx.db, file, ctx.offset, ctx.prefix, false) {
        let mut item = CompletionItem::new(candidate.kind, &candidate.request.name)
            .detail(format!("include {}", candidate.include_path))
            .relevance(CompletionRelevance { requires_import: true, ..Default::default() });
        item.import = Some(candidate.request);
        acc.add(item);
    }
}
