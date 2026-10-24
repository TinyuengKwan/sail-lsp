//! `bitfield_accessors` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn bitfield_accessors(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = bitfield_accessor_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("bitfield_accessors", AssistKind::RefactorRewrite),
        "Generate bitfield accessors",
        ctx.range,
        te,
    );
    Some(())
}

pub fn bitfield_accessor_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    use hir::DefKind;
    let offset = base_db::range_start(range);
    let item_tree = file.item_tree()?;
    let text = file.text();

    for &id in item_tree.top_level_items() {
        let span = id.span(item_tree);
        if offset < span.start || offset > span.end {
            continue;
        }
        if id.def_kind(item_tree) != DefKind::Bitfield {
            continue;
        }

        let bf_name = id.name(item_tree).as_str();
        let def_text = text.get(span.start..span.end)?;
        // Extract field names from { field : hi .. lo } body
        let brace_start = def_text.find('{')?;
        let brace_end = def_text.rfind('}')?;
        let inner = &def_text[brace_start + 1..brace_end];

        let mut code = String::new();
        for item in inner.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let field_name = item.split(':').next()?.trim();
            if field_name == "bits" || field_name.is_empty() {
                continue;
            }

            code.push_str(&format!(
                "\nfunction _get_{bf_name}_{field_name}(bf : {bf_name}) -> bits('n) = bf.bits[{field_name}]\n"
            ));
            code.push_str(&format!(
                "\nfunction _set_{bf_name}_{field_name}(bf : {bf_name}, v : bits('n)) -> {bf_name} = {{\n  let new_bits = [bf.bits with {field_name} = v];\n  Mk_{bf_name}(new_bits)\n}}\n"
            ));
        }

        if code.is_empty() {
            return None;
        }
        return Some(vec![IdeTextEdit {
            range: base_db::text_range(span.end, span.end),
            new_text: code,
        }]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels =
            assist_labels(bitfield_accessors, "bitfield Foo : bits(8) = { x : 0 .. 7 }\n", 0);
        let _ = labels;
    }
}
