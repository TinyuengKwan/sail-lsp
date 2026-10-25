//! Assist handler modules.
//! Each handler is in its own file and registered in `all()`.
//! Handler type: `fn(&mut Assists, &AssistContext<'_>) -> Option<()>`.

use crate::assist_context::Handler;

mod add_braces;
mod add_explicit_type;
mod add_missing_fields;
pub(crate) mod add_missing_match_arms;
mod add_return_type;
mod add_scattered_end;
mod apply_demorgan;
pub(crate) mod auto_include;
mod bind_unused_param;
mod bitfield_accessors;
mod block_to_line_comment;
mod change_visibility;
mod convert_comment_block;
mod convert_comment_style;
mod convert_literal_format;
mod convert_match_to_if;
pub(crate) mod evaluate_constant;
mod expand_rest_pattern;
mod extract_function;
mod extract_struct_from_enum_variant;
mod extract_to_include;
mod extract_type_alias;
pub(crate) mod extract_variable;
mod flip_binexpr;
mod flip_comma;
mod flip_or_pattern;
mod generate_constant;
mod generate_doc_template;
mod generate_documentation_template;
pub(crate) mod generate_enum_variant;
mod generate_fn_type_alias;
mod generate_function;
mod generate_getter_or_setter;
mod generate_mapping_clause;
mod generate_new;
mod generate_val_spec;
mod guarded_return;
pub(crate) mod inline_call;
mod inline_const_as_literal;
mod inline_type_alias;
mod inline_variable;
mod invert_if;
mod line_to_block_comment;
mod merge_match_arms;
mod merge_nested_if;
mod move_guard;
pub(crate) mod organize_imports;
mod promote_local_to_const;
mod pull_assignment_up;
pub(crate) mod quick_fix;
mod remove_else_branches;
mod remove_mut;
mod remove_parentheses;
mod remove_underscore;
mod remove_unused_param;
mod reorder_fields;
mod replace_if_with_match;
pub(crate) mod simplify_boolean;
mod sort_items;
mod toggle_doc_comment;
mod unmerge_match_arm;
mod unwrap_block;
mod unwrap_tuple;

/// Return all registered assist handlers.
pub(crate) fn all() -> &'static [Handler] {
    &[
        add_explicit_type::add_explicit_type,
        auto_include::auto_include,
        invert_if::invert_if,
        flip_binexpr::flip_binexpr,
        flip_or_pattern::flip_or_pattern,
        apply_demorgan::apply_demorgan,
        extract_variable::extract_variable,
        extract_type_alias::extract_type_alias,
        expand_rest_pattern::expand_rest_pattern,
        extract_function::extract_function,
        extract_struct_from_enum_variant::extract_struct_from_enum_variant,
        inline_type_alias::inline_type_alias,
        inline_variable::inline_variable,
        generate_doc_template::generate_doc_template,
        unmerge_match_arm::unmerge_match_arm,
        unwrap_block::unwrap_block,
        unwrap_tuple::unwrap_tuple,
        pull_assignment_up::pull_assignment_up,
        guarded_return::guarded_return,
        sort_items::sort_items,
        line_to_block_comment::line_to_block_comment,
        block_to_line_comment::block_to_line_comment,
        toggle_doc_comment::toggle_doc_comment,
        bind_unused_param::bind_unused_param,
        bitfield_accessors::bitfield_accessors,
        evaluate_constant::evaluate_constant,
        simplify_boolean::simplify_boolean,
        organize_imports::organize_imports,
        generate_val_spec::generate_val_spec,
        add_missing_match_arms::add_missing_match_arms,
        generate_enum_variant::generate_enum_variant,
        generate_fn_type_alias::generate_fn_type_alias,
        generate_function::generate_function,
        add_missing_fields::add_missing_fields,
        convert_match_to_if::convert_match_to_if,
        merge_match_arms::merge_match_arms,
        move_guard::move_guard,
        remove_unused_param::remove_unused_param,
        replace_if_with_match::replace_if_with_match,
        inline_call::inline_call,
        generate_mapping_clause::generate_mapping_clause,
        add_return_type::add_return_type,
        add_scattered_end::add_scattered_end,
        change_visibility::change_visibility,
        flip_comma::flip_comma,
        remove_parentheses::remove_parentheses,
        inline_const_as_literal::inline_const_as_literal,
        promote_local_to_const::promote_local_to_const,
        remove_else_branches::remove_else_branches,
        reorder_fields::reorder_fields,
        remove_mut::remove_mut,
        generate_constant::generate_constant,
        convert_comment_style::convert_comment_style,
        extract_to_include::extract_to_include,
        add_braces::add_braces,
        convert_comment_block::convert_comment_block,
        convert_literal_format::convert_literal_format,
        merge_nested_if::merge_nested_if,
        remove_underscore::remove_underscore,
        generate_documentation_template::generate_documentation_template,
        generate_new::generate_new,
        generate_getter_or_setter::generate_getter_or_setter,
    ]
}
