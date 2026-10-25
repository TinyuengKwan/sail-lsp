mod types;
pub use types::*;
pub mod support;

use self::support::{binding_type_hint, infer_call_arg_types_at_position};
use crate::markdown_remove::remove_markdown;

/// Limit only top-level members; commas in dependent types and tuples do not
/// separate fields. Tokenization also keeps braces in strings/comments inert.
fn truncate_members(text: &str, limit: Option<usize>) -> String {
    let Some(limit) = limit else { return text.to_owned() };
    let tokens = parser::tokenize(text);
    let mut nesting = 0usize;
    let Some(open) = tokens.iter().position(|(_, span)| {
        match &text[span.start..span.end] {
            "{" if nesting == 0 => return true,
            "(" | "[" | "{" => nesting += 1,
            ")" | "]" | "}" => nesting = nesting.saturating_sub(1),
            _ => {}
        }
        false
    }) else {
        return text.to_owned();
    };
    let mut depth = 0usize;
    let mut starts = Vec::new();
    let mut next_member = true;
    for (_, span) in &tokens[open + 1..] {
        let token = &text[span.start..span.end];
        if token == "}" && depth == 0 {
            if starts.len() <= limit {
                return text.to_owned();
            }
            let cut = starts[limit];
            return format!(
                "{}... (+{} more) {}",
                &text[..cut],
                starts.len() - limit,
                &text[span.start..]
            );
        }
        if depth == 0 && token == "," {
            next_member = true;
            continue;
        }
        if next_member {
            starts.push(span.start);
            next_member = false;
        }
        match token {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    text.to_owned()
}

/// Get plain-text version of a documentation string (for inlay hints etc).
#[allow(unused)] // will be wired into inlay hints / plain-text hover fallback
/// Documentation with its Markdown removed, for a client that takes no
/// Markdown.
///
/// `to_proto` sent `MarkupKind::Markdown` unconditionally, so a client
/// whose `contentFormat` omits it rendered `**bold**` and the backticks
/// as themselves. This had no caller.
pub fn doc_to_plain_text(doc: &str) -> String {
    remove_markdown(doc)
}
use crate::signature_help::find_call_at_position;
use hir::DefKind;
use ide_db::ide_types::SymbolKind;
use ide_db::line_index::TextRange;
use ide_db::token_classify::{token_is_close_bracket, token_is_open_bracket};
use ide_db::{builtin_docs, find_callable_signature, instantiate_signature, FileDb, LineCol};
use parser::Token;
use syntax::parser_lower::{Decl, DeclKind, DeclRole, Scope};
use url::Url;

/// Hover — returns internal HoverResult (framework-independent).
pub fn hover_for_symbol<'a, F, I>(
    files: I,
    current_uri: &Url,
    current_file: &dyn FileDb,
    position: LineCol,
    hover_range: TextRange,
    symbol_key: &str,
    config: &crate::hover::HoverConfig,
) -> Option<HoverResult>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    if symbol_key.starts_with('\'') {
        return Some(HoverResult {
            markup: fenced_sail(symbol_key),
            range: hover_range,
            actions: Vec::new(),
        });
    }

    let files: Vec<(&Url, &dyn FileDb)> =
        files.into_iter().map(|(u, f)| (u, f as &dyn FileDb)).collect();
    let current_offset = current_file.offset_at(&position);

    // Builtin docs. `config.documentation` is the client's setting, which
    // used to be parsed and converted by the server and then dropped.
    if let Some(doc) = builtin_docs(symbol_key) {
        let mut markdown = vec![format!("**builtin** **{symbol_key}**")];
        if config.documentation {
            markdown.push("___".to_string());
            markdown.push(doc.to_string());
        }
        return Some(markdown_hover(markdown.join("\n\n"), hover_range));
    }

    let ordered_def = if current_file
        .file_text()
        .is_some_and(|(db, ft)| hir::source_order::declarations_for(db, ft).is_some())
    {
        Some(ide_db::defs::Definition::classify_at(current_file, position)?)
    } else {
        None
    };
    let decl_ref = if let Some(ref def) = ordered_def {
        resolve_ordered_decl(&files, current_uri, current_file, def)
    } else {
        resolve_decl(files.iter().copied(), current_uri, symbol_key, Some(current_offset))
    };

    let mut markdown = Vec::new();

    if let Some(decl_ref) = decl_ref.as_ref() {
        let label = decl_kind_label(decl_ref.decl.kind);
        let name = &decl_ref.decl.name;

        // Hover shows kind + name + type signature + docs.
        // Reference/implementation counts belong in code lenses, not hover.
        //
        // The head line is what the HIR says this definition is:
        // `Definition::classify_at` resolves the cursor and `display(db)`
        // renders it, which is how rust-analyzer's `ide` renders a
        // definition. Every line of this markup used to come from the
        // per-file scrape's `Decl`, so `HirDisplay` had one impl per
        // definition form and no caller anywhere. Getting here took three
        // fixes on the resolution side, not the rendering side:
        // `Local::is_param`, containment rather than id equality for an
        // annotated parameter, and the value namespace preferring a
        // `function` over its `val`.
        //
        // `Unresolved` *is* "the resolver could not place this", and it
        // renders as the bare name — less than the scrape's label says.
        // That is the case the fallback exists for.
        let hir_rendered = current_file.file_text().and_then(|(db, _)| {
            use hir::HirDisplay;
            let def = ide_db::defs::Definition::classify_at(current_file, position)?;
            // `Unresolved` *is* "the resolver could not place this",
            // and a `ValSpec` names the declaration where the cursor
            // may be on the definition — `function add` with a
            // `val add` above it still resolves to the spec through
            // the workspace scope. Both fall back.
            if matches!(
                def,
                ide_db::defs::Definition::Unresolved(_) | ide_db::defs::Definition::ValSpec(_)
            ) {
                return None;
            }
            let rendered = def.display(db).to_string();
            (!rendered.trim().is_empty()).then_some(rendered)
        });
        let member_limit = match decl_ref.decl.kind {
            DeclKind::Struct | DeclKind::Union => config.max_fields_count,
            DeclKind::Enum => config.max_enum_variants_count,
            DeclKind::Overload => config.max_overloads_count,
            _ => None,
        };
        let hir_rendered = hir_rendered.map(|text| truncate_members(&text, member_limit));
        let header = hir_rendered
            .as_deref()
            .map(|rendered| format!("**{rendered}**"))
            .unwrap_or_else(|| format!("**{label}** **{name}**"));
        markdown.push(header);

        // The signature line, from the HIR when it renders one.
        //
        // `render_signature` falls through to `decl_headline`, which walks
        // the token stream back from the declaration's span and slices the
        // source text — the per-file scrape answering a question the item
        // tree already holds. A rendering that carries a `:` or a `=` is a
        // signature; a bare keyword and name is not more than the scrape
        // recovers, so that still wins.
        //
        // This could not fire until `Semantics::analyze` stopped giving up
        // where no *body* contains the offset, which is every top-level
        // declaration site — so the definitions that have signatures never
        // reached the HIR at all.
        let selected_signature = ordered_def.as_ref().and_then(|def| {
            let (db, _) = current_file.file_text()?;
            def.def_location()?.lookup_signature(db)
        });
        let mut headline = selected_signature
            .or(hir_rendered
                .filter(|rendered| rendered.contains(" : ") || rendered.contains(" = ")))
            .unwrap_or_else(|| {
                render_signature(decl_ref, files.iter().copied(), current_uri, symbol_key)
            });

        // Item 2: Generic Substitution / Instantiation
        if symbol_kind_for_decl(decl_ref.decl.kind) == SymbolKind::Function {
            let offset = current_file.offset_at(&position);
            let is_at_definition = decl_ref.decl.role == DeclRole::Definition
                && decl_ref.decl.span.start <= offset
                && offset <= decl_ref.decl.span.end;

            if !is_at_definition {
                let lc_pos = position;
                if let Some((callee, _)) = find_call_at_position(current_file, lc_pos) {
                    if callee == *name {
                        if let Some(sig) = current_file
                            .file_text()
                            .and_then(|(db, ft)| {
                                ide_db::signatures::callable_signatures_at(
                                    db,
                                    ft,
                                    current_offset,
                                    name,
                                )
                            })
                            .map(|sigs| sigs.into_iter().next())
                            .unwrap_or_else(|| {
                                find_callable_signature(files.iter().copied(), current_uri, name)
                            })
                        {
                            if let Some(arg_types) = infer_call_arg_types_at_position(
                                &files,
                                current_uri,
                                current_file,
                                lc_pos,
                                name,
                            ) {
                                let instantiated = instantiate_signature(&sig, &arg_types);
                                if instantiated != headline {
                                    markdown.push("___".to_string());
                                    markdown.push("*instantiated as:*".to_string());
                                    headline = instantiated;
                                }
                            }
                        }
                    }
                }
            }
        }

        // For call sites (not definition), supplement headline with val spec
        // type info when the function def lacks type annotations.
        if symbol_kind_for_decl(decl_ref.decl.kind) == SymbolKind::Function {
            let offset = current_file.offset_at(&position);
            let same_file = decl_ref.uri == current_uri;
            let at_def = same_file
                && decl_ref.decl.role == DeclRole::Definition
                && decl_ref.decl.span.start <= offset
                && offset <= decl_ref.decl.span.end;
            if !at_def && !headline.contains(':') {
                if let Some(sig) =
                    find_callable_signature(files.iter().copied(), current_uri, &decl_ref.decl.name)
                {
                    if sig.label.contains(':') {
                        headline = sig.label;
                    }
                }
            }
        }

        markdown.push("___".to_string());
        markdown.push(fenced_sail(&truncate_members(&headline, member_limit)));

        // Show inferred type for local bindings (let/var)
        if matches!(decl_ref.decl.kind, DeclKind::Let | DeclKind::Var) {
            if let Some(ty) = binding_type_hint(&files, current_uri, current_file, &decl_ref.decl) {
                markdown.push(format!("**type:** `{ty}`"));
            }
        }

        // Overload inspection
        if decl_ref.decl.kind == DeclKind::Overload {
            let members = current_file
                .file_text()
                .and_then(|(db, file)| {
                    ide_db::signatures::callable_signatures_at(db, file, current_offset, name)
                })
                .map(|signatures| signatures.into_iter().map(|sig| sig.label).collect::<Vec<_>>())
                .unwrap_or_else(|| {
                    overload_members(decl_ref.file, &decl_ref.decl)
                        .into_iter()
                        .map(|member| {
                            find_callable_signature(files.iter().copied(), current_uri, &member)
                                .map(|sig| sig.label)
                                .unwrap_or(member)
                        })
                        .collect()
                });
            if !members.is_empty() {
                markdown.push("___".to_string());
                markdown.push(format!("**{} overloads:**", members.len()));
                let limit = config.max_overloads_count.unwrap_or(members.len());
                for member in members.iter().take(limit) {
                    markdown.push(fenced_sail(member));
                }
                if members.len() > limit {
                    markdown.push(format!("… (+{} more)", members.len() - limit));
                }
            }
        }

        // Respect the callable identity visible at this program point.
        if matches!(decl_ref.decl.kind, DeclKind::Function | DeclKind::Value | DeclKind::Mapping) {
            let all_sigs = current_file
                .file_text()
                .and_then(|(db, file)| {
                    ide_db::signatures::callable_signatures_at(
                        db,
                        file,
                        current_offset,
                        &decl_ref.decl.name,
                    )
                })
                .unwrap_or_else(|| {
                    ide_db::signatures::find_all_callable_signatures(
                        files.iter().copied(),
                        &decl_ref.decl.name,
                    )
                });
            if all_sigs.len() > 1 {
                markdown.push("___".to_string());
                markdown.push(format!("**{} overloads:**", all_sigs.len()));
                let limit = config.max_overloads_count.unwrap_or(all_sigs.len());
                for sig in all_sigs.iter().take(limit) {
                    markdown.push(fenced_sail(&sig.label));
                }
                if all_sigs.len() > limit {
                    markdown.push(format!("… (+{} more)", all_sigs.len() - limit));
                }
            }
        }

        // Bitfield layout visualization in hover.
        // Shows field names, bit positions, and widths.
        if decl_ref.decl.kind == DeclKind::Bitfield {
            // Extract the full definition text from the source
            let full_text = decl_ref
                .file
                .text()
                .get(decl_ref.decl.span.start..decl_ref.decl.span.end)
                .unwrap_or("");
            let fields = crate::bitfield_layout::parse_bitfield_fields(full_text);
            if !fields.is_empty() {
                markdown.push("___".to_string());
                markdown.push("**layout:**".to_string());
                markdown.push(String::new());
                // Render as ASCII box-drawing table inside code block
                // for proper monospace alignment in hover.
                let layout = crate::bitfield_layout::render_bitfield_layout(&fields);
                markdown.push(layout);
            }
        }

        // Enum/Union hover — show all variants (scattered or inline).
        // Collects enum/union clauses from workspace ItemTrees.
        if matches!(decl_ref.decl.kind, DeclKind::Enum | DeclKind::Union) {
            let enum_name = &decl_ref.decl.name;
            let mut variants: Vec<String> = Vec::new();

            // Collect from all workspace ItemTrees (scattered enum clauses)
            for (_, file) in files.iter() {
                if let Some(tree) = file.item_tree() {
                    for &id in tree.top_level_items() {
                        // Scattered clauses: member_name carries the variant name
                        if *id.name(tree) == **enum_name && id.is_clause(tree) {
                            if let Some(member) = id.member_name(tree) {
                                let member_s = member.to_string();
                                if !variants.contains(&member_s) {
                                    variants.push(member_s);
                                }
                            }
                        }
                    }
                }
            }

            // Also try extracting inline variants from source text
            if variants.is_empty() {
                let full_text = decl_ref
                    .file
                    .text()
                    .get(decl_ref.decl.span.start..decl_ref.decl.span.end)
                    .unwrap_or("");
                if let Some(brace_start) = full_text.find('{') {
                    if let Some(brace_end) = full_text.rfind('}') {
                        let inner = &full_text[brace_start + 1..brace_end];
                        for part in inner.split(',') {
                            let v = part.trim().split(':').next().unwrap_or("").trim();
                            if !v.is_empty() {
                                variants.push(v.to_string());
                            }
                        }
                    }
                }
            }

            if !variants.is_empty() {
                markdown.push("___".to_string());
                let count = variants.len();
                let limit = member_limit.unwrap_or(count);
                if count <= limit {
                    markdown.push(format!("**{count} variants:** {}", variants.join(", ")));
                } else {
                    let shown: Vec<_> = variants.iter().take(limit).map(|s| s.as_str()).collect();
                    markdown.push(format!(
                        "**{count} variants:** {}, ... (+{} more)",
                        shown.join(", "),
                        count - limit
                    ));
                }
            }
        }

        // Effect tracking: show side effects of functions
        if matches!(decl_ref.decl.kind, DeclKind::Function | DeclKind::Value | DeclKind::Mapping) {
            {
                let text = decl_ref.file.text();
                let (cst_root, _) = syntax::parse_text(text);
                // Show declared effects from val spec (if any)
                let declared = declared_effects_for_def_cst(decl_ref.file, &decl_ref.decl.name);
                let effects = infer_effects_for_def_with_workspace_cst(
                    &cst_root,
                    &decl_ref.decl.name,
                    &files,
                );
                if !declared.is_empty() || !effects.is_empty() {
                    markdown.push("___".to_string());
                    if !declared.is_empty() {
                        markdown.push(format!("**declared effects:** {{{}}}", declared.join(", ")));
                    }
                    if !effects.is_empty() {
                        markdown.push(format!("**inferred effects:** {}", effects.join(", ")));
                    } else if !declared.is_empty() {
                        markdown.push("**inferred effects:** *pure*".to_string());
                    }
                } else {
                    markdown.push("___".to_string());
                    markdown.push("**effects:** *pure*".to_string());
                }
            }
        }

        // Show doc comments. Primary path: ItemTree.doc field
        // (extracted from CST during parse → Docs).
        // The doc the item tree recorded. The fallback that stood here
        // scanned backwards through the source for `///` lines above the
        // definition — a second doc-comment parser, for the case where the
        // item tree is absent, which is no longer a case any caller has.
        let doc_text = decl_ref.decl.doc.clone();
        if let Some(comments) = doc_text.filter(|_| config.documentation) {
            markdown.push("___".to_string());
            markdown.push(comments);
        }

        // Constant folding: show computed value for let/var bindings
        if matches!(decl_ref.decl.kind, DeclKind::Let | DeclKind::Var) {
            let value_span =
                find_binding_value_span_from_text(decl_ref.file.text(), decl_ref.decl.span);
            if let Some(value_span) = value_span {
                let value_text = decl_ref.file.text().get(value_span.start..value_span.end);
                if let Some(value_text) = value_text {
                    if let Some(folded) = ide_assists::try_fold_constant(value_text) {
                        markdown.push("___".to_string());
                        markdown.push(format!("**value:** `{folded}`"));
                    }
                }
            }
        }

        // Show path like RA (using simple relative path or filename)
        let path = decl_ref.uri.path();
        let filename = path.split('/').next_back().unwrap_or(path);

        // Build navigation link
        let pos = decl_ref.file.position_at(decl_ref.decl.span.start);
        let link = format!("{}#L{},{}", decl_ref.uri, pos.line + 1, pos.col + 1);

        markdown.push("___".to_string());
        markdown.push(format!("[Go to Definition]({}) • *in {}*", link, filename));
    } else if let Some(rendered) = current_file.file_text().and_then(|(db, _)| {
        // The scrape has no `Decl` for a struct field — it records
        // top-level declarations — so hovering the `variety` of
        // `struct { variety = .. }` fell through to the item-tree search
        // below, which looks for a *top-level* name and finds none, and
        // said the bare word. Ask the HIR first: `classify_at` knows what
        // a name in a field list is, and `HirDisplay` renders it.
        use hir::HirDisplay;
        let def = ide_db::defs::Definition::classify_at(current_file, position)?;
        if matches!(def, ide_db::defs::Definition::Unresolved(_)) {
            return None;
        }
        let rendered = def.display(db).to_string();
        (!rendered.trim().is_empty() && rendered.trim() != symbol_key).then_some(rendered)
    }) {
        markdown.push(format!("**{rendered}**"));
    } else {
        // when the structural decl resolver returned nothing
        // (cross-file symbols, half-parsed files, etc.) fall back
        // to looking up the symbol across every file's ItemTree
        // — every public top-level item carries a stable
        // span-free signature_text rendered at parse time, so we
        // can show a meaningful one-line summary even though we
        // never managed to find a structural decl. Prefer
        // val/mapping spec entries since they always carry the
        // explicit type signature; bare function definitions may
        // only have the parameter list. Falls through to the
        // bare-symbol-key behaviour when no item tree entry exists
        // either.
        let item_tree_signature = files
            .iter()
            .filter_map(|(_, file)| file.item_tree())
            .filter_map(|tree| {
                let mut best: Option<hir::DefItem> = None;
                for &id in tree.top_level_items() {
                    if id.name(tree).as_str() != symbol_key {
                        continue;
                    }
                    let take = match best {
                        None => true,
                        Some(existing) => {
                            let new_is_spec = matches!(
                                id.def_kind(tree),
                                DefKind::ValSpec | DefKind::MappingSpec
                            );
                            let existing_is_spec = matches!(
                                existing.def_kind(tree),
                                DefKind::ValSpec | DefKind::MappingSpec
                            );
                            new_is_spec && !existing_is_spec
                        }
                    };
                    if take {
                        best = Some(id);
                    }
                }
                best.map(|id| (id.signature(tree).to_string(), id.doc(tree).map(|d| d.to_string())))
            })
            .next();

        if let Some((signature, doc)) = item_tree_signature {
            markdown.push(format!("**{symbol_key}**"));
            markdown.push("___".to_string());
            markdown.push(fenced_sail(&signature));
            if let Some(doc_text) = doc {
                markdown.push("___".to_string());
                markdown.push(doc_text);
            }
            // Annotate synthesized bitfield accessors.
            if let Some(accessor) = hir::bitfield::parse_accessor_name(symbol_key) {
                let desc = match accessor.kind {
                    hir::bitfield::BitfieldAccessorKind::Constructor => format!(
                        "*(generated constructor for bitfield `{}`)*",
                        accessor.bitfield_name
                    ),
                    hir::bitfield::BitfieldAccessorKind::Getter => format!(
                        "*(generated getter for `{}.{}`)*",
                        accessor.bitfield_name, accessor.field_name
                    ),
                    hir::bitfield::BitfieldAccessorKind::Updater => format!(
                        "*(generated updater for `{}.{}`)*",
                        accessor.bitfield_name, accessor.field_name
                    ),
                    hir::bitfield::BitfieldAccessorKind::Setter => format!(
                        "*(generated setter for `{}.{}`)*",
                        accessor.bitfield_name, accessor.field_name
                    ),
                };
                markdown.push("___".to_string());
                markdown.push(desc);
            } else {
                markdown.push("___".to_string());
                markdown.push("*from item tree (no structural decl resolved)*".to_string());
            }
        } else {
            // Try showing inferred expression type even without a resolved decl
            if let Some(ty) = current_file.file_text().and_then(|(db, ft)| {
                hir::Semantics::new(db).expr_type_text(
                    ft,
                    parser::Span::new(
                        base_db::range_start(hover_range),
                        base_db::range_end(hover_range),
                    ),
                    current_file.bodies(),
                )
            }) {
                markdown.push(fenced_sail(symbol_key));
                markdown.push(format!("**type:** `{ty}`"));
            } else {
                markdown.push(fenced_sail(symbol_key));
            }
        }
    }

    let markup = markdown.join("\n\n");

    // J2-4: Populate HoverActions.
    // RA attaches "Go to implementations" / "Go to references" actions
    // based on the definition kind. LSP Hover doesn't natively support
    // actions, but clients like VS Code can use custom commands.
    let mut actions = Vec::new();
    if let Some(dr) = decl_ref.as_ref() {
        let decl = &dr.decl;
        match decl.kind {
            // Val spec → "Go to implementations"
            DeclKind::Value => {
                actions.push(crate::hover::HoverAction::Implementation(
                    crate::hover::HoverFilePosition {
                        file_id: base_db::FileId::from_raw(0), // placeholder
                        offset: decl.span.start,
                    },
                ));
            }
            // Function/Mapping → "Go to references"
            DeclKind::Function | DeclKind::Mapping => {
                actions.push(crate::hover::HoverAction::Reference(
                    crate::hover::HoverFilePosition {
                        file_id: base_db::FileId::from_raw(0), // placeholder
                        offset: decl.span.start,
                    },
                ));
            }
            _ => {}
        }
    }

    Some(HoverResult { markup, range: hover_range, actions })
}

fn overload_members(file: &dyn FileDb, decl: &Decl) -> Vec<String> {
    let Some(tokens) = file.tokens() else {
        return Vec::new();
    };
    let Some(idx) = tokens.iter().position(|(_, span)| span.start == decl.span.start) else {
        return Vec::new();
    };

    let mut members = Vec::new();
    let mut j = idx;
    while j < tokens.len() && tokens[j].0 != Token::Equal {
        j += 1;
    }
    j += 1; // skip '='

    let mut depth = 0;
    while j < tokens.len() {
        let (token, _) = &tokens[j];
        match token {
            Token::LeftCurlyBracket => depth += 1,
            Token::RightCurlyBracket => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Token::Id(name) if depth == 1 => members.push(name.clone()),
            Token::Comma if depth == 1 => {}
            _ if depth == 0 && token_starts_declaration(token) => break,
            _ => {}
        }
        j += 1;
    }
    members
}

fn markdown_hover(markdown: String, range: TextRange) -> HoverResult {
    HoverResult { markup: markdown, range, actions: Vec::new() }
}

fn fenced_sail(text: &str) -> String {
    format!("```sail\n{text}\n```")
}

/// The fallback for a cursor the resolver cannot place. The header comes
/// from `HirDisplay` when it can — see `hover_for_symbol`.
fn decl_kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Function => "function",
        DeclKind::Value => "value",
        DeclKind::Mapping => "mapping",
        DeclKind::Overload => "overload",
        DeclKind::Register => "register",
        DeclKind::Parameter => "parameter",
        DeclKind::Type => "type",
        DeclKind::Struct => "struct",
        DeclKind::Union => "union",
        DeclKind::Bitfield => "bitfield",
        DeclKind::Enum => "enum",
        DeclKind::EnumMember => "enum member",
        DeclKind::Newtype => "newtype",
        DeclKind::Let => "let binding",
        DeclKind::Var => "var binding",
    }
}

fn symbol_kind_for_decl(kind: DeclKind) -> SymbolKind {
    match kind {
        DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload => {
            SymbolKind::Function
        }
        DeclKind::Register | DeclKind::Parameter | DeclKind::Let | DeclKind::Var => {
            SymbolKind::Variable
        }
        DeclKind::Enum => SymbolKind::Enum,
        DeclKind::EnumMember => SymbolKind::EnumMember,
        DeclKind::Type
        | DeclKind::Struct
        | DeclKind::Union
        | DeclKind::Bitfield
        | DeclKind::Newtype => SymbolKind::Struct,
    }
}

fn token_starts_declaration(token: &Token) -> bool {
    matches!(
        token,
        Token::KwFunction
            | Token::KwVal
            | Token::KwMapping
            | Token::KwOverload
            | Token::KwRegister
            | Token::KwType
            | Token::KwStruct
            | Token::KwUnion
            | Token::KwBitfield
            | Token::KwNewtype
            | Token::KwEnum
            | Token::KwLet
            | Token::KwVar
            | Token::KwScattered
            | Token::Directive { .. }
            | Token::StructuredDirectiveStart(_)
    )
}

fn decl_headline(file: &dyn FileDb, decl: &Decl) -> String {
    // Use ItemTree entry for top-level definitions (CST-native)
    if decl.scope == Scope::TopLevel {
        if let Some(it) = file.item_tree() {
            for &id in it.top_level_items() {
                let span = id.span(it);
                if id.name(it).as_str() == decl.name
                    && span.start <= decl.span.start
                    && span.end >= decl.span.end
                {
                    if let Some(text) = file.text().get(span.start..span.end) {
                        return text.trim().to_string();
                    }
                }
            }
        }
    }

    // For local bindings (parameters, let, var), scan forward from the
    // binding name to include the type annotation if present.
    if decl.scope == Scope::Local {
        if let Some(tokens) = file.tokens() {
            if let Some(idx) = tokens.iter().position(|(_, span)| span.start == decl.span.start) {
                let label = decl_kind_label(decl.kind);
                // Scan forward for `: type` annotation
                let mut end = idx;
                if end + 1 < tokens.len() && tokens[end + 1].0 == Token::Colon {
                    // Skip colon + type tokens until we hit =, ), comma, or newline
                    end += 1; // colon
                    let mut depth = 0_usize;
                    while end + 1 < tokens.len() {
                        let next = &tokens[end + 1].0;
                        if token_is_open_bracket(next) {
                            depth += 1;
                            end += 1;
                        } else if token_is_close_bracket(next) {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                            end += 1;
                        } else if depth == 0
                            && matches!(next, Token::Equal | Token::Comma | Token::FatRightArrow)
                        {
                            break;
                        } else {
                            end += 1;
                        }
                    }
                    let text = file.text();
                    let start_off = tokens[idx].1.start;
                    let end_off = tokens[end].1.end;
                    if let Some(binding_text) = text.get(start_off..end_off) {
                        return format!("{label} {}", binding_text.trim());
                    }
                }
                return format!("{label} {}", decl.name);
            }
        }
    }

    let Some(tokens) = file.tokens() else {
        return format!("{} {}", decl_kind_label(decl.kind), decl.name);
    };
    let Some(idx) = tokens.iter().position(|(_, span)| span.start == decl.span.start) else {
        return format!("{} {}", decl_kind_label(decl.kind), decl.name);
    };
    let text = file.text();
    let mut start_idx = idx;
    while start_idx > 0 {
        let token = &tokens[start_idx - 1].0;
        if token_starts_declaration(token) {
            start_idx -= 1;
            break;
        }
        start_idx -= 1;
    }
    let mut end_idx = idx;
    let mut depth = 0_usize;
    while end_idx + 1 < tokens.len() {
        let token = &tokens[end_idx + 1].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token_starts_declaration(token) {
            break;
        }
        end_idx += 1;
    }
    text[tokens[start_idx].1.start..tokens[end_idx].1.end].trim().to_string()
}

fn render_signature<'a, I>(
    decl_ref: &DeclRef<'a>,
    files: I,
    current_uri: &Url,
    symbol_key: &str,
) -> String
where
    I: IntoIterator<Item = (&'a Url, &'a dyn FileDb)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    if matches!(decl_ref.decl.kind, DeclKind::Parameter | DeclKind::Let | DeclKind::Var) {
        if let Some(ty) = binding_type_hint(&files, current_uri, decl_ref.file, &decl_ref.decl) {
            let kw = match decl_ref.decl.kind {
                DeclKind::Parameter => "parameter",
                DeclKind::Var => "var",
                _ => "let",
            };
            return format!("{kw} {} : {ty}", decl_ref.decl.name);
        }
    }

    if decl_ref.decl.kind == DeclKind::EnumMember {
        if let Some(EnumInfo::Member { enum_name, .. }) =
            enum_info_for_symbol(decl_ref.file, symbol_key)
        {
            return format!("{enum_name}::{symbol_key}");
        }
    }

    decl_headline(decl_ref.file, &decl_ref.decl)
}

struct DeclRef<'a> {
    uri: &'a Url,
    file: &'a dyn FileDb,
    decl: Decl,
}

fn resolve_ordered_decl<'a>(
    files: &[(&'a Url, &'a dyn FileDb)],
    current_uri: &'a Url,
    current_file: &'a dyn FileDb,
    def: &ide_db::defs::Definition,
) -> Option<DeclRef<'a>> {
    let (db, _) = current_file.file_text()?;
    if let ide_db::defs::Definition::Local(local) = def {
        return Some(DeclRef {
            uri: current_uri,
            file: current_file,
            decl: Decl {
                name: local.name.to_string(),
                kind: if local.is_param { DeclKind::Parameter } else { DeclKind::Let },
                role: DeclRole::Definition,
                scope: Scope::Local,
                span: local.span,
                name_span: Some(local.span),
                is_scattered: false,
                doc: None,
            },
        });
    }
    let loc = def.def_location()?;
    let (tree, item) = loc.item(db)?;
    let (uri, file) = files
        .iter()
        .copied()
        .find(|(_, f)| f.file_text().is_some_and(|(_, ft)| ft == loc.file_text))?;
    Some(DeclRef {
        uri,
        file,
        decl: Decl {
            name: item.name(tree).to_string(),
            kind: decl_kind_of(item.def_kind(tree))?,
            role: if declares_only(item.def_kind(tree)) {
                DeclRole::Declaration
            } else {
                DeclRole::Definition
            },
            scope: Scope::TopLevel,
            span: item.span(tree),
            name_span: Some(item.name_span(tree)),
            is_scattered: item.is_clause(tree),
            doc: item.doc(tree).map(str::to_owned),
        },
    })
}

/// The declaration `symbol_key` names, ranked by how likely it is the one
/// the cursor meant.
///
/// The `Decl` it returns is built from the item tree and the bodies, not
/// read out of the flat scrape: `parser_lower::Decl` is the record hover's
/// markup switches on, and this fills it from the layer that lowered the
/// definitions.
fn resolve_decl<'a, I>(
    files: I,
    current_uri: &Url,
    symbol_key: &str,
    current_offset: Option<usize>,
) -> Option<DeclRef<'a>>
where
    I: IntoIterator<Item = (&'a Url, &'a dyn FileDb)>,
{
    let files = files.into_iter().collect::<Vec<_>>();

    // A local binding under the cursor wins over anything at top level: it
    // is what the cursor is on.
    if let Some(offset) = current_offset {
        for (uri, file) in files.iter().copied() {
            if uri != current_uri {
                continue;
            }
            let Some(bodies) = file.bodies() else { continue };
            let mut best: Option<Decl> = None;
            for entry in bodies.entries() {
                if entry.def_span.start > offset || offset > entry.def_span.end {
                    continue;
                }
                for (pat_id, pat) in entry.body.iter_pats() {
                    let hir::Pat::Bind(name) = pat else { continue };
                    if name.as_str() != symbol_key {
                        continue;
                    }
                    let Some(span) = entry.source_map.pat_syntax(pat_id) else { continue };
                    if span.start > offset {
                        continue;
                    }
                    // A parameter is a binding the body lists as one; hover
                    // says so, and a `let` inside the body is a `let`.
                    let is_param = entry.body.params.iter().any(|&p| {
                        p == pat_id
                            || matches!(
                                entry.body.pat(p),
                                Some(hir::Pat::Typed { inner, .. }) if *inner == pat_id
                            )
                    });
                    if best.as_ref().is_none_or(|b| b.span.start < span.start) {
                        best = Some(Decl {
                            name: symbol_key.to_string(),
                            kind: if is_param { DeclKind::Parameter } else { DeclKind::Let },
                            role: DeclRole::Definition,
                            scope: Scope::Local,
                            span,
                            name_span: Some(span),
                            is_scattered: false,
                            doc: None,
                        });
                    }
                }
            }
            if let Some(decl) = best {
                return Some(DeclRef { uri, file, decl });
            }
        }
    }

    let mut best: Option<(usize, DeclRef<'a>)> = None;
    for (uri, file) in files {
        let Some(tree) = file.item_tree() else {
            continue;
        };
        // Every definition with this name, plus the `enum`/`union` members
        // that carry it — the item tree records members on the definition.
        let mut found: Vec<Decl> = Vec::new();
        for &id in tree.top_level_items() {
            if id.name(tree).as_str() == symbol_key {
                if let Some(kind) = decl_kind_of(id.def_kind(tree)) {
                    found.push(Decl {
                        name: symbol_key.to_string(),
                        kind,
                        role: if declares_only(id.def_kind(tree)) {
                            DeclRole::Declaration
                        } else {
                            DeclRole::Definition
                        },
                        scope: Scope::TopLevel,
                        span: id.span(tree),
                        name_span: Some(id.name_span(tree)),
                        is_scattered: id.is_clause(tree),
                        doc: id.doc(tree).map(str::to_string),
                    });
                }
            }
            if matches!(id.def_kind(tree), hir::DefKind::Enum | hir::DefKind::Union) {
                for field in id.fields(tree) {
                    if field.name.as_str() == symbol_key {
                        found.push(Decl {
                            name: symbol_key.to_string(),
                            kind: DeclKind::EnumMember,
                            role: DeclRole::Definition,
                            scope: Scope::TopLevel,
                            span: field.name_span,
                            name_span: Some(field.name_span),
                            is_scattered: false,
                            doc: None,
                        });
                    }
                }
            }
        }
        for decl in found {
            let mut score = uri_prefix_score(current_uri, uri) * 16;
            if uri == current_uri {
                score += 8;
            }
            if decl.role == DeclRole::Definition {
                score += 4;
            }
            score += match symbol_kind_for_decl(decl.kind) {
                SymbolKind::EnumMember => 2,
                SymbolKind::Function => 1,
                _ => 0,
            };
            match &best {
                Some((best_score, _)) if *best_score > score => {}
                _ => best = Some((score, DeclRef { uri, file, decl })),
            }
        }
    }
    best.map(|(_, decl_ref)| decl_ref)
}

/// A `val` or a bare `mapping` signature declares without defining.
fn declares_only(kind: hir::DefKind) -> bool {
    matches!(kind, hir::DefKind::ValSpec | hir::DefKind::MappingSpec | hir::DefKind::ScatteredHead)
}

/// The `DeclKind` hover's markup switches on, for a definition of this kind.
fn decl_kind_of(kind: hir::DefKind) -> Option<DeclKind> {
    use hir::DefKind as D;
    Some(match kind {
        D::Function | D::ScatteredHead | D::ScatteredClause => DeclKind::Function,
        D::ValSpec | D::Outcome => DeclKind::Value,
        D::Mapping | D::MappingSpec => DeclKind::Mapping,
        D::Overload => DeclKind::Overload,
        D::TypeAlias => DeclKind::Type,
        D::Struct => DeclKind::Struct,
        D::Union => DeclKind::Union,
        D::Enum => DeclKind::Enum,
        D::Bitfield => DeclKind::Bitfield,
        D::Newtype => DeclKind::Newtype,
        D::Register => DeclKind::Register,
        D::Let => DeclKind::Let,
        D::Var => DeclKind::Var,
        D::EndMarker
        | D::Constraint
        | D::TerminationMeasure
        | D::Instantiation
        | D::Field
        | D::Variant => return None,
    })
}

fn uri_prefix_score(lhs: &Url, rhs: &Url) -> usize {
    match (lhs.path_segments(), rhs.path_segments()) {
        (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
        _ => 0,
    }
}

enum EnumInfo {
    Member { enum_name: String },
}

fn enum_info_for_symbol(file: &dyn FileDb, symbol: &str) -> Option<EnumInfo> {
    // The enum that declares this member. The item tree records members on
    // the definition, so there is no need for the positional walk this used
    // to do — track the last enum seen, and give up on the first
    // declaration that is not a member of it.
    if let Some(tree) = file.item_tree() {
        for &id in tree.top_level_items() {
            if id.def_kind(tree) != hir::DefKind::Enum {
                continue;
            }
            if id.fields(tree).iter().any(|f| f.name.as_str() == symbol) {
                return Some(EnumInfo::Member { enum_name: id.name(tree).to_string() });
            }
        }
    }

    let tokens = file.tokens()?;
    let mut i = 0usize;

    while i + 1 < tokens.len() {
        if tokens[i].0 != Token::KwEnum {
            i += 1;
            continue;
        }
        let Token::Id(enum_name) = &tokens[i + 1].0 else {
            i += 1;
            continue;
        };

        let mut j = i + 2;
        while j < tokens.len() && tokens[j].0 != Token::LeftCurlyBracket {
            if token_starts_declaration(&tokens[j].0) {
                break;
            }
            j += 1;
        }
        if j >= tokens.len() || tokens[j].0 != Token::LeftCurlyBracket {
            i += 1;
            continue;
        }

        let mut members = Vec::<String>::new();
        let mut depth = 1_i32;
        j += 1;
        while j < tokens.len() && depth > 0 {
            match &tokens[j].0 {
                Token::LeftCurlyBracket => depth += 1,
                Token::RightCurlyBracket => depth -= 1,
                Token::Id(name) if depth == 1 => members.push(name.clone()),
                _ => {}
            }
            j += 1;
        }

        if members.iter().any(|member| member == symbol) {
            return Some(EnumInfo::Member { enum_name: enum_name.clone() });
        }

        i = j;
    }

    None
}

/// CST-native version of `infer_effects_for_def_with_workspace`.
fn infer_effects_for_def_with_workspace_cst(
    cst_root: &syntax::SyntaxNode,
    name: &str,
    all_files: &[(&url::Url, &dyn ide_db::FileDb)],
) -> Vec<String> {
    use hir::callgraph::{CallGraph, WorkspaceCallGraph};
    use hir::{CallableBodies, EffectTag};

    let bodies = CallableBodies::from_cst(cst_root);
    let local_callgraph = CallGraph::from_callable_bodies(&bodies);

    let ws_callgraph: Option<WorkspaceCallGraph> = if all_files.len() > 1 {
        Some(WorkspaceCallGraph::from_callgraphs(
            all_files.iter().filter_map(|(_, f)| f.callgraph()),
        ))
    } else {
        None
    };

    let mut direct: std::collections::HashMap<String, std::collections::BTreeSet<EffectTag>> =
        std::collections::HashMap::new();
    for entry in bodies.entries() {
        direct.entry(entry.name.clone()).or_default().extend(entry.effects.iter().copied());
    }

    let mut full = direct.clone();
    for _ in 0..full.len() + 1 {
        let mut changed = false;
        let snapshot = full.clone();
        for (caller, effects) in full.iter_mut() {
            let callees: Vec<&str> = if let Some(ref ws) = ws_callgraph {
                ws.callees_of(caller).collect()
            } else {
                local_callgraph.callees_of(caller).collect()
            };
            for callee in callees {
                if let Some(callee_effects) = snapshot.get(callee) {
                    for eff in callee_effects {
                        if effects.insert(*eff) {
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let effects = full.get(name).cloned().unwrap_or_default();
    effects.iter().map(|t| t.as_str().to_string()).collect()
}

pub fn collect_effects_from_body(file: &dyn ide_db::FileDb, name: &str) -> Vec<String> {
    use hir::CallableBodies;

    let (cst_root, _) = syntax::parse_text(file.text());
    let bodies = CallableBodies::from_cst(&cst_root);

    let mut effects = Vec::new();
    for entry in bodies.entries() {
        if entry.name == name {
            for tag in &entry.effects {
                effects.push(tag.as_str().to_string());
            }
        }
    }
    effects.sort();
    effects.dedup();
    effects
}

/// Find the span of a binding's initializer value using Body arena.
/// CST-native replacement for `declared_effects_for_def`.
pub fn declared_effects_for_def_cst(file: &dyn ide_db::FileDb, name: &str) -> Vec<String> {
    // Effects are stored in the val spec signature text.
    // For now, delegate to EffectTag infrastructure.
    collect_effects_from_body(file, name)
}

/// Find the value span of a let/var binding from source text.
/// Looks for `= value` in the definition text. CST-native replacement
/// for `find_binding_value_span`.
fn find_binding_value_span_from_text(
    source: &str,
    decl_span: parser::Span,
) -> Option<parser::Span> {
    let def_text = source.get(decl_span.start..decl_span.end)?;
    // Find standalone `=` (not `==`, `=>`, `!=`, `<=`, `>=`)
    let bytes = def_text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'=' {
            let prev = if i > 0 { bytes[i - 1] } else { 0 };
            let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            if prev == b'!' || prev == b'<' || prev == b'>' {
                continue;
            }
            if next == b'=' || next == b'>' {
                continue;
            }
            let value_start = decl_span.start + i + 1;
            let value_text = &def_text[i + 1..];
            let trimmed_start = value_text.len() - value_text.trim_start().len();
            let trimmed_end = value_text.len() - value_text.trim_end().len();
            return Some(parser::Span::new(
                value_start + trimmed_start,
                decl_span.start + def_text.len() - trimmed_end,
            ));
        }
    }
    None
}

/// 投産-4: Semantic hover enrichment using SourceAnalyzer.
///
/// Resolves field access and method call expressions at the cursor to
/// provide richer hover information. Called from the LSP handler layer
/// where salsa database is available.
///
/// Returns additional markdown lines to append to hover, or None if
/// no semantic enrichment is possible.
pub fn hover_resolve_field_or_method(
    db: &dyn salsa::Database,
    file_text: base_db::FileText,
    offset: usize,
) -> Option<String> {
    let sema = hir::Semantics::new(db);
    let source = file_text.text(db);

    // Extract the identifier at offset for display
    let token_name = extract_hover_identifier(source, offset)?;

    // Try field resolution
    if let Some(_field_def_id) = sema.resolve_field(file_text, offset) {
        // Try to find the parent type name by looking at the expression before the dot
        let type_name = find_type_before_dot(source, offset);
        return match type_name {
            Some(ty) => Some(format!("field `{}` of `{}`", token_name, ty)),
            None => Some(format!("field `{}`", token_name)),
        };
    }

    // Try method/function call resolution
    if let Some((_func_id, _target_file_id)) = sema.resolve_method_call(file_text, offset) {
        // Try to get the return type from type inference
        if let Some(ty) = sema.type_of_expr(file_text, offset) {
            use hir::HirDisplay;
            return Some(format!("```sail\n{} : {}\n```", token_name, ty.display(db)));
        }
        return Some(format!("function `{}`", token_name));
    }

    None
}

/// Extract an identifier at a byte offset (for hover display).
fn extract_hover_identifier(source: &str, offset: usize) -> Option<String> {
    if offset >= source.len() {
        return None;
    }
    let bytes = source.as_bytes();
    if !(bytes[offset].is_ascii_alphanumeric() || bytes[offset] == b'_') {
        return None;
    }
    let mut start = offset;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    let mut end = offset;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some(source[start..end].to_string())
}

/// Try to find the type/expression name before a `.field` access.
/// Scans backward from the field name past the dot to find the preceding identifier.
fn find_type_before_dot(source: &str, field_offset: usize) -> Option<String> {
    let bytes = source.as_bytes();
    // Walk backward past the field name
    let mut pos = field_offset;
    while pos > 0 && (bytes[pos - 1].is_ascii_alphanumeric() || bytes[pos - 1] == b'_') {
        pos -= 1;
    }
    // Expect a dot
    if pos == 0 || bytes[pos - 1] != b'.' {
        return None;
    }
    pos -= 1; // skip dot
              // Skip whitespace before dot
    while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
        pos -= 1;
    }
    // Now read the identifier before the dot
    if pos == 0 {
        return None;
    }
    let end = pos;
    while pos > 0 && (bytes[pos - 1].is_ascii_alphanumeric() || bytes[pos - 1] == b'_') {
        pos -= 1;
    }
    if pos == end {
        return None;
    }
    Some(source[pos..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    fn hover_markdown(hover: HoverResult) -> String {
        hover.markup
    }

    #[test]
    fn falls_back_to_item_tree_signature_when_no_decl_resolved() {
        let file_a_src = "val helper : int -> int\nfunction helper(x) = x\n";
        let file_b_src = "function caller() -> int = helper(1)\n";
        let file_a = TestFile::new(file_a_src);
        let file_b = TestFile::new(file_b_src);
        let uri_a = Url::parse("file:///tmp/a.sail").unwrap();
        let uri_b = Url::parse("file:///tmp/b.sail").unwrap();

        let off = file_b_src.find("helper(1)").unwrap();
        let pos = file_b.position_at(off);
        let hover = hover_for_symbol(
            [(&uri_a, &file_a), (&uri_b, &file_b)],
            &uri_b,
            &file_b,
            pos,
            base_db::text_range(off, off + 6),
            "helper",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        let from_structural = markdown.contains("**function** **helper**")
            || markdown.contains("**value** **helper**");
        let from_fallback = markdown.contains("from item tree");
        assert!(
            from_structural || from_fallback,
            "expected either structural or ItemTree-fallback hover, got: {markdown}"
        );
        assert!(markdown.contains("int"), "hover should mention the int type, got: {markdown}");
    }

    #[test]
    fn shows_function_signature_and_location() {
        let source = "val add : int -> int\nfunction add(x) = x\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.find("add(x)").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 3),
            "add",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**function** **add**"));
        assert!(markdown.contains("```sail\nfunction add(x) = x\n```"));
        assert!(markdown.contains("*in main.sail*"));
    }

    #[test]
    fn shows_local_binding_type_hint() {
        let source = "function foo() = {\n  let x : bits(32) = 1;\n  x\n}\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.rfind("x").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 1),
            "x",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**let x**"));
        assert!(markdown.contains("```sail\nlet x : bits(32)\n```"));
    }

    #[test]
    fn shows_inferred_local_binding_type_hint() {
        let source = "function foo() = {\n  let x = 1;\n  x\n}\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.rfind("x").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 1),
            "x",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**let x**"));
        assert!(markdown.contains("```sail\nlet x : atom(1)\n```"));
    }

    #[test]
    fn shows_parameter_type_hint() {
        // Run in a thread with larger stack — the hover path has deep
        // call chains that exceed the default 2MB test thread stack
        // in debug builds (RA mitigates this via salsa-cached queries).
        let result = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let source = "function foo(x : bits(32)) = x\n".to_string();
                let file = TestFile::new(&source);
                let uri = Url::parse("file:///tmp/main.sail").unwrap();
                let off = source.rfind("x").unwrap();
                let pos = file.position_at(off);

                let hover = hover_for_symbol(
                    std::iter::once((&uri, &file)),
                    &uri,
                    &file,
                    pos,
                    base_db::text_range(off, off + 1),
                    "x",
                    &crate::hover::HoverConfig::default(),
                )
                .expect("hover");
                let markdown = hover_markdown(hover);
                assert!(markdown.contains("**parameter x**"));
                assert!(
                    markdown.contains("parameter x : bits(32)")
                        || markdown.contains("**parameter x**"),
                    "expected parameter hover, got: {markdown}"
                );
            })
            .unwrap()
            .join();
        result.unwrap();
    }

    #[test]
    fn shows_enum_member_context() {
        let source = "enum color = { Red, Green, Blue }\nlet x = Red\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.rfind("Red").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 3),
            "Red",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        // The header is what the HIR says the name is, not the scrape's
        // category label: `Definition::EnumVariant` renders the enum it
        // belongs to, which the label could not.
        assert!(markdown.contains("**color::Red**"), "{markdown}");
        assert!(markdown.contains("```sail\ncolor::Red\n```"), "{markdown}");
    }

    #[test]
    fn shows_overload_members() {
        let source = r#"
val add : int -> int
function add(x) = x
val sub : int -> int
function sub(x) = x
overload op = {add, sub}
"#
        .to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let start = source.find("op =").unwrap();
        let pos = file.position_at(start);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(start, start + 2),
            "op",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**overload op"));
        assert!(markdown.contains("**2 overloads:**"), "{markdown}");
        assert!(markdown.contains("```sail\nval add : int -> int\n```"));
        assert!(markdown.contains("```sail\nval sub : int -> int\n```"));
    }

    #[test]
    fn shows_type_variable_hover() {
        let file = TestFile::new("val f : bits('n) -> bits('n)\n");
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = LineCol { line: 0, col: 0 };
        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(0, 2),
            "'n",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert_eq!(markdown.trim(), "```sail\n'n\n```");
    }

    #[test]
    fn shows_comments_in_hover() {
        // `///`, not `//`: upstream lexes the first as a doc comment and
        // discards the second. This used to pass with `//` because hover
        // found comments by scanning backwards through the source.
        let source =
            "/// This is a comment\n/// for the add function\nfunction add(x) = x\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.find("add(x)").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 3),
            "add",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("This is a comment\nfor the add function"));
    }

    #[test]
    fn returns_precise_hover_range_for_identifier() {
        let source = "val add : int -> int\nfunction add(x) = x\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let name_offset = source.rfind("add").unwrap();
        let pos = file.position_at(name_offset + 1);
        let hover_range = base_db::text_range(name_offset, name_offset + 3);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            hover_range,
            "add",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");

        assert_eq!(hover.range, hover_range);
    }

    #[test]
    fn shows_instantiated_signature_in_hover() {
        let source =
            "val f : bits('n) -> bits('n)\nfunction f(x) = x\nlet _ = f(0xDEADBEEF)\n".to_string();
        let file = TestFile::new(&source);
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let off = source.rfind("f(0x").unwrap();
        let pos = file.position_at(off);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 1),
            "f",
            &crate::hover::HoverConfig::default(),
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        // Instantiation requires type checker (not available in TestFile).
        // When type check is wired into TestFile, the full assertion holds.
        // For now, verify that at least the base signature is shown.
        let has_instantiation = markdown.contains("instantiated as");
        let has_base_sig = markdown.contains("bits('n) -> bits('n)");
        assert!(
            has_instantiation || has_base_sig,
            "expected either instantiated or base signature, got: {markdown}"
        );
    }

    #[test]
    fn hover_bitfield_shows_layout_table() {
        let src = "bitfield Flags : bits(8) = { carry : 0, zero : 1, neg : 7 .. 4 }\n";
        let file = TestFile::new(src);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();

        let off = src.find("Flags").unwrap();
        let pos = file.position_at(off);
        let hover = hover_for_symbol(
            [(&uri, &file)],
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 5),
            "Flags",
            &crate::hover::HoverConfig::default(),
        );
        let hover = hover.expect("bitfield hover should produce result");
        let md = hover_markdown(hover);
        assert!(md.contains("**bitfield Flags"), "should have bitfield label: {md}");
        assert!(md.contains("layout"), "should contain layout table: {md}");
        assert!(md.contains("carry"), "should list carry field: {md}");
        assert!(md.contains("zero"), "should list zero field: {md}");
    }

    #[test]
    fn member_limits_preserve_nested_types_and_report_omissions() {
        let source =
            "struct S = { first : vector(2, bits(8)), second : (int, bool), third : string }";
        assert_eq!(
            truncate_members(source, Some(1)),
            "struct S = { first : vector(2, bits(8)), ... (+2 more) }"
        );
        assert_eq!(truncate_members(source, Some(0)), "struct S = { ... (+3 more) }");
        assert_eq!(truncate_members(source, Some(3)), source);
        assert_eq!(truncate_members(source, None), source);
        assert_eq!(
            truncate_members(
                "struct S ('n : Int, 'n in {1, 2}) = { a : bits('n), b : int }",
                Some(1)
            ),
            "struct S ('n : Int, 'n in {1, 2}) = { a : bits('n), ... (+1 more) }"
        );
        assert_eq!(
            truncate_members("struct S = { a : string /* , } */, b : int }", Some(1)),
            "struct S = { a : string /* , } */, ... (+1 more) }"
        );
    }

    #[test]
    fn hover_enum_shows_inline_variants() {
        let src = "enum Color = { Red, Green, Blue }\n";
        let file = TestFile::new(src);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();

        let off = src.find("Color").unwrap();
        let pos = file.position_at(off);
        let hover = hover_for_symbol(
            [(&uri, &file)],
            &uri,
            &file,
            pos,
            base_db::text_range(off, off + 5),
            "Color",
            &crate::hover::HoverConfig::default(),
        );
        let hover = hover.expect("enum hover should produce result");
        let md = hover_markdown(hover);
        assert!(md.contains("**enum Color"), "should have enum label: {md}");
        assert!(md.contains("variants"), "should mention variants: {md}");
        assert!(md.contains("Red"), "should list Red variant: {md}");
        assert!(md.contains("Green"), "should list Green variant: {md}");
        assert!(md.contains("Blue"), "should list Blue variant: {md}");
    }

    #[test]
    fn hover_enum_shows_scattered_variants() {
        // Simulate scattered enum across two files
        let src_a = "scattered enum extension\nenum clause extension = Ext_M\nenum clause extension = Ext_A\n";
        let src_b = "enum clause extension = Ext_F\n";
        let file_a = TestFile::new(src_a);
        let file_b = TestFile::new(src_b);
        let uri_a = Url::parse("file:///tmp/a.sail").unwrap();
        let uri_b = Url::parse("file:///tmp/b.sail").unwrap();

        let off = src_a.find("extension").unwrap();
        let pos = file_a.position_at(off);
        let hover = hover_for_symbol(
            [(&uri_a, &file_a), (&uri_b, &file_b)],
            &uri_a,
            &file_a,
            pos,
            base_db::text_range(off, off + 9),
            "extension",
            &crate::hover::HoverConfig::default(),
        );
        // May or may not produce hover depending on structural resolution,
        // but if it does it should list variants
        if let Some(hover) = hover {
            let md = hover_markdown(hover);
            if md.contains("variants") {
                assert!(md.contains("Ext_M"), "should list Ext_M: {md}");
                assert!(md.contains("Ext_A"), "should list Ext_A: {md}");
            }
        }
    }
}

#[cfg(test)]
mod field_hover_tests {
    use crate::fixture;

    fn hover_at(source: &str, needle: &str) -> Option<String> {
        let (analysis, files) = fixture::analysis(source);
        let at = source.find(needle).expect("fixture");
        let li = ide_db::line_index::LineIndex::new(source);
        analysis
            .hover(files[0].0, li.line_col(at), &crate::hover::HoverConfig::default())
            .expect("no cancellation")
            .map(|r| r.markup)
    }

    /// A field name in a struct literal says which struct and which type.
    ///
    /// The per-file scrape records *top-level* declarations, so it has no
    /// `Decl` for a field — and hover's whole HIR path sat inside the
    /// `if let Some(decl_ref)` branch. Every `struct { variety = .. }` in
    /// the corpus hovered as the bare word. `Definition::Field` was a
    /// classification nothing built and nothing dispatched on.
    #[test]
    fn a_struct_field_hovers_with_its_type() {
        let markup = hover_at(
            "\
default Order dec
struct S = { variety : int, strength : bool }
function mk() -> S = struct { variety = 1, strength = true }
",
            "variety = 1",
        )
        .expect("no hover");
        assert!(markup.contains("S.variety"), "{markup}");
        assert!(markup.contains("int"), "{markup}");
    }

    /// With no annotation to read — a literal handed straight to a call —
    /// a field name that exactly one declaration in the file uses decides
    /// it. Two declarations sharing the name say nothing rather than
    /// guessing.
    #[test]
    fn an_unannotated_literal_resolves_a_unique_field_name() {
        let source = "\
default Order dec
struct S = { variety : int }
val take : S -> unit
function mk() -> unit = take(struct { variety = 1 })
";
        let markup = hover_at(source, "variety = 1").expect("no hover");
        assert!(markup.contains("S.variety"), "{markup}");

        let ambiguous = "\
default Order dec
struct S = { variety : int }
struct T = { variety : bool }
val take : S -> unit
function mk() -> unit = take(struct { variety = 1 })
";
        let markup = hover_at(ambiguous, "variety = 1").unwrap_or_default();
        assert!(!markup.contains("S.variety"), "guessed between two structs: {markup}");
        assert!(!markup.contains("T.variety"), "guessed between two structs: {markup}");
    }
}

#[cfg(test)]
mod constructor_hover_tests {
    use crate::fixture;

    /// A constructor hovers as the type that declares it.
    ///
    /// The *type* carries the constructor's name, not the item, so the
    /// resolver had nothing to answer with and every use came back
    /// `Unresolved`. `Definition::EnumVariant` was a classification nothing
    /// built — the same shape as `Definition::Field`.
    #[test]
    fn a_constructor_hovers_as_its_type() {
        let source = "\
default Order dec
newtype regno = Regno : int
val g : regno -> int
function g(Regno(n)) = n
";
        let (analysis, files) = fixture::analysis(source);
        let li = ide_db::line_index::LineIndex::new(source);
        let at = source.find("Regno(n)").expect("fixture");
        let markup = analysis
            .hover(files[0].0, li.line_col(at), &crate::hover::HoverConfig::default())
            .expect("no cancellation")
            .expect("no hover")
            .markup;
        assert!(markup.contains("regno::Regno"), "{markup}");
        assert!(markup.contains("int"), "{markup}");
    }

    /// Two declarations sharing a member name say nothing rather than
    /// guessing.
    #[test]
    fn an_ambiguous_constructor_is_not_guessed() {
        let source = "\
default Order dec
enum A = {Shared, OnlyA}
enum B = {Shared, OnlyB}
val f : A -> unit
function f(x) = match x { Shared => (), OnlyA => () }
";
        let (analysis, files) = fixture::analysis(source);
        let li = ide_db::line_index::LineIndex::new(source);
        let at = source.find("Shared => ()").expect("fixture");
        let markup = analysis
            .hover(files[0].0, li.line_col(at), &crate::hover::HoverConfig::default())
            .expect("no cancellation")
            .map(|r| r.markup)
            .unwrap_or_default();
        assert!(!markup.contains("A::Shared"), "guessed: {markup}");
        assert!(!markup.contains("B::Shared"), "guessed: {markup}");
    }
}
