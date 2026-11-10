//! User templates. Receiver insertion is a single pass over the template.
use super::Completions;
use crate::{
    context::{CompletionContext, CompletionPosition},
    item::CompletionItem,
    SnippetScope,
};
use ide_db::defs::CompletionItemKind;

pub(super) fn prefix(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    if !ctx.config.enable_snippets {
        return;
    }
    let scope = match ctx.position {
        CompletionPosition::TopLevel => SnippetScope::Item,
        CompletionPosition::Expression => SnippetScope::Expr,
        CompletionPosition::TypeAnnotation => SnippetScope::Type,
        CompletionPosition::Pattern => return,
    };
    for template in ctx.config.custom_templates.iter() {
        let Some(trigger) = &template.prefix else { continue };
        if template.scope != scope
            || !trigger.to_lowercase().starts_with(&ctx.prefix.to_lowercase())
        {
            continue;
        }
        acc.add(
            CompletionItem::new(CompletionItemKind::Snippet, trigger)
                .detail(&template.description)
                .filter_text(trigger)
                .insert_text(template.body.as_ref()),
        );
    }
}

pub(super) fn postfix(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    if !ctx.config.enable_snippets || !ctx.config.enable_postfix_completions {
        return;
    }
    let Some(dot) = ctx
        .offset
        .checked_sub(ctx.prefix.len() + 1)
        .filter(|&dot| ctx.text.as_bytes().get(dot) == Some(&b'.'))
    else {
        return;
    };
    let Some((start, receiver)) = ctx.postfix_receiver(dot) else { return };
    let suffix = ctx.text[ctx.offset..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .map(char::len_utf8)
        .sum::<usize>();
    for template in ctx.config.custom_templates.iter() {
        let Some(trigger) = &template.postfix else { continue };
        if !trigger.to_lowercase().starts_with(&ctx.prefix.to_lowercase()) {
            continue;
        }
        let mut item = CompletionItem::new(CompletionItemKind::Snippet, format!(".{trigger}"))
            .detail(&template.description)
            .filter_text(trigger)
            .insert_text(expand_receiver(&template.body, receiver))
            .sort_text(format!("0000_{trigger}"));
        item.source_range = Some(base_db::text_range(start, ctx.offset + suffix));
        acc.add(item);
    }
}

pub(super) fn escape_literal(receiver: &str) -> String {
    let mut escaped = String::with_capacity(receiver.len());
    for c in receiver.chars() {
        if matches!(c, '\\' | '$' | '}') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn expand_receiver(template: &str, receiver: &str) -> String {
    let escaped = escape_literal(receiver);
    let mut out = String::new();
    let mut rest = template;
    while !rest.is_empty() {
        if rest.starts_with("${receiver}") {
            out.push_str(&escaped);
            rest = &rest[11..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
            if c == '\\' {
                if let Some(c) = rest.chars().next() {
                    out.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn receiver_braces_dollars_and_backslashes_remain_literals() {
        assert_eq!(
            super::expand_receiver(r#"wrap(${receiver}, ${1:body})$0"#, r#"{ x = "$1" }"#),
            r#"wrap({ x = "\$1" \}, ${1:body})$0"#
        );
        assert_eq!(super::expand_receiver(r#"\${receiver} ${receiver}"#, "a"), r#"\${receiver} a"#);
    }
}
