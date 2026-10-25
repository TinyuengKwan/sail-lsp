//! Extract complete top-level declarations into a sibling include file.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use syntax::SyntaxKind as SK;

pub(crate) fn extract_to_include(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    if !ctx.config.file_creation || ctx.has_empty_selection() {
        return None;
    }
    let selected = ctx.selection_trimmed();
    let root = ctx.source_file()?;
    let nodes: Vec<_> = root
        .children()
        .filter_map(|node| {
            // The parser may attach a following doc comment to the previous
            // declaration. Require complete non-trivia syntax, preserving exactly
            // the comments and whitespace selected by the user.
            let tokens: Vec<_> = node
                .descendants_with_tokens()
                .filter_map(|it| it.into_token())
                .filter(|token| !token.kind().is_trivia() && token.kind() != SK::DOC_COMMENT)
                .collect();
            let range = syntax::TextRange::new(
                tokens.first()?.text_range().start(),
                tokens.last()?.text_range().end(),
            );
            range.intersect(selected).filter(|range| !range.is_empty()).map(|_| (node, range))
        })
        .collect();
    if nodes.is_empty()
        || nodes.iter().any(|(node, range)| {
            !selected.contains_range(*range)
                || matches!(node.kind(), SK::ERROR | SK::DIRECTIVE_DEF)
                || node.descendants().any(|node| node.kind() == SK::ERROR)
        })
    {
        return None;
    }
    let name = nodes.iter().find_map(|(node, _)| {
        node.children().find(|child| child.kind() == SK::NAME).map(|name| name.to_string()).or_else(
            || {
                node.children_with_tokens()
                    .filter_map(|it| it.into_token())
                    .find(|token| token.kind() == SK::IDENT)
                    .map(|token| token.text().to_owned())
            },
        )
    })?;
    let name: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let path = format!("{name}_extracted.sail");
    let text = ctx.source_text();
    let start = usize::from(selected.start());
    let end = usize::from(selected.end());
    // The include must occupy its own line. Do not move an inline fragment or
    // swallow a trailing comment that the user did not select.
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    if !text[line_start..start].trim().is_empty() || !text[end..line_end].trim().is_empty() {
        return None;
    }
    // Pragmas can change include paths and conditional state. Keep them in the
    // original file; extraction replaces declarations in their existing order.
    if root
        .descendants()
        .any(|node| node.kind() == SK::DIRECTIVE_DEF && selected.contains_range(node.text_range()))
    {
        return None;
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let contents = format!("{}{newline}", &text[start..end]);
    let include = format!("$include \"{path}\"");
    acc.add(
        AssistId("extract_to_include", AssistKind::RefactorExtract),
        format!("Extract to {path}"),
        selected,
        |builder| {
            builder.create_file(path, contents);
            builder.replace(selected, include);
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::source_change::FileCreation;

    #[test]
    fn moves_selected_declarations_and_preserves_comments_and_order() {
        let source = "val before : int\n/// documentation\nval foo : int -> int\nfunction foo(x) = x\nval after : int\n";
        let file = ide_db::test_utils::TestFile::new(source);
        let config = crate::AssistConfig::default();
        let start = source.find("///").unwrap();
        let end = source.find("val after").unwrap();
        let ctx = AssistContext::new(&config, &file, base_db::text_range(start, end));
        let mut acc = Assists::new(base_db::FileId::from_raw(7));
        extract_to_include(&mut acc, &ctx).unwrap();
        let change = acc.finish().remove(0).source_change.unwrap();
        let FileCreation { anchor, path, contents } = &change.file_creations[0];
        assert_eq!(*anchor, base_db::FileId::from_raw(7));
        assert_eq!(path, "foo_extracted.sail");
        assert_eq!(contents, &source[start..end]);
        let edit = &change.source_file_edits[anchor][0];
        let mut after = source.to_owned();
        after.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
        assert_eq!(after, "val before : int\n$include \"foo_extracted.sail\"\nval after : int\n");
    }

    #[test]
    fn rejects_partial_declarations_and_directives() {
        for source in [
            "function f() = $0{ 1 }$0\n",
            "$0$include \"other.sail\"\nfunction f() = 1$0\n",
            "$0function f() = $0{ 1 }\n",
        ] {
            crate::tests::check_assist_not_applicable(extract_to_include, source);
        }
    }

    #[test]
    fn respects_file_creation_capability() {
        let source = "function f() = 1\n";
        let file = ide_db::test_utils::TestFile::new(source);
        let config = crate::AssistConfig { file_creation: false, ..Default::default() };
        let ctx = AssistContext::new(&config, &file, base_db::text_range(0, source.len()));
        assert!(extract_to_include(&mut Assists::default(), &ctx).is_none());
    }
}
