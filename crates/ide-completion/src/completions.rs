//! Completion accumulator and provider modules.
//! The `Completions` struct accumulates completion items from providers.
//! Provider modules (one per completion category) live in `completions/`.

use crate::item::CompletionItem;

/// Dot completion (field access after `.`).
pub(crate) mod dot;

/// Expression path completion — the main expression-position provider.
pub(crate) mod expr;

/// Function parameter name completion.
pub(crate) mod fn_param;

/// Auto-import completion (symbols from other files + auto-$include).
pub(crate) mod flyimport;

/// Item list completion — top-level and block-level keywords.
pub(crate) mod item_list;

/// Keyword completion.
pub(crate) mod keyword;

/// Match pattern completion.
pub(crate) mod pattern;

/// Postfix completion (`.if`, `.match`, etc.).
pub(crate) mod postfix;

/// Pragma completion (`@name`, `$name` directives).
/// Sail-specific (no RA counterpart).
pub(crate) mod pragma;

/// Record field completion (inside `{ ... }` literals/patterns).
pub(crate) mod record;

/// Snippet completion (code templates).
pub(crate) mod snippet;

/// Type-position completion (after `:` or `->`).
pub(crate) mod type_;

/// Accumulator for completion items being built.
///
/// Providers push items via `add()`, `add_opt()`, `add_many()`.
/// Convert to `Vec<CompletionItem>` via `From` impl at the end.
#[derive(Debug, Default)]
pub struct Completions {
    buf: Vec<CompletionItem>,
}

impl From<Completions> for Vec<CompletionItem> {
    fn from(val: Completions) -> Self {
        val.buf
    }
}

impl Completions {
    /// Add a single completion item.
    pub(crate) fn add(&mut self, item: impl Into<CompletionItem>) {
        self.buf.push(item.into());
    }

    /// Add multiple completion items.
    pub(crate) fn add_many<I: Into<CompletionItem>>(&mut self, items: impl IntoIterator<Item = I>) {
        self.buf.extend(items.into_iter().map(Into::into));
    }

    /// Add an optional completion item.
    #[allow(dead_code)]
    pub(crate) fn add_opt(&mut self, item: Option<CompletionItem>) {
        if let Some(item) = item {
            self.buf.push(item);
        }
    }
}

use ide_db::FileDb;
use url::Url;

use crate::context;

/// Dispatch NameRef completions based on sub-context.
pub(super) fn complete_name_ref(
    acc: &mut Completions,
    ctx: &context::CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
    name_ref_ctx: &context::NameRefContext,
    current_uri: &Url,
    keywords: &[&str],
    builtins: &[&str],
) {
    // Dot access: `expr.|` → field/method completion
    if let Some(ref dot_access) = name_ref_ctx.dot_access {
        dot::complete_dot(acc, ctx, dot_access, all_files);
        postfix::complete_postfix(acc, ctx);
        return;
    }

    // Path context → dispatch by PathKind
    if let Some(path_ctx) = &name_ref_ctx.path_ctx {
        match &path_ctx.kind {
            context::PathKind::Expr { expr_ctx } => {
                expr::complete_expr_path(
                    acc, ctx, all_files, path_ctx, expr_ctx, keywords, builtins,
                );
                // Record literal field completion inside `StructName { | }`.
                let record_items = record::complete_record(
                    ctx.file, ctx.text, ctx.offset, ctx.prefix, all_files,
                );
                acc.add_many(record_items);
                postfix::complete_postfix(acc, ctx);
            }
            context::PathKind::Type { .. } => {
                let type_names = crate::collect_type_names(all_files);
                let type_name_refs: Vec<(&str, ide_db::defs::SymbolKind)> =
                    type_names.iter().map(|(n, k)| (n.as_str(), *k)).collect();
                let type_items = type_::complete_type_pos(ctx.prefix, &type_name_refs);
                acc.add_many(type_items);
            }
            context::PathKind::Item { kind } => {
                item_list::complete_item_list(acc, ctx, path_ctx, kind, keywords);
            }
            context::PathKind::Pat { pat_ctx } => {
                if pat_ctx.is_param {
                    fn_param::complete_fn_param(acc, ctx);
                }
                pattern::complete_pattern(acc, ctx, all_files);
            }
        }
    }

    // Flyimport — always try cross-file symbol suggestions
    flyimport::import_on_the_fly(acc, ctx, all_files, current_uri);
}
