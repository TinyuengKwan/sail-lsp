//! Assist handler modules.
//! Each handler is in its own file and registered in `all()`.
//! Handler type: `fn(&mut Assists, &AssistContext<'_>) -> Option<()>`.

use crate::assist_context::Handler;

mod add_braces;
mod add_explicit_type;
mod add_missing_fields;
mod add_missing_match_arms;
mod add_scattered_end;
mod add_turbo_fish;
mod apply_demorgan;
mod auto_include;
mod bitfield_accessors;
mod block_to_line_comment;
mod change_visibility;
mod convert_comment_block;
mod convert_comment_style;
mod convert_integer_literal;
mod convert_match_to_if;
mod destructure_struct_binding;
mod evaluate_constant;
mod expand_rest_pattern;
mod extract_function;
mod extract_to_include;
mod extract_type_alias;
mod extract_variable;
mod fill_record_fields;
mod flip_binexpr;
mod flip_comma;
mod flip_or_pattern;
mod generate_constant;
mod generate_doc_template;
mod generate_documentation_template;
mod generate_enum_variant;
mod generate_function;
mod generate_getter_or_setter;
mod generate_mapping_clause;
mod generate_new;
mod generate_val_spec;
mod guarded_return;
mod inline_call;
mod inline_const_as_literal;
mod inline_variable;
mod invert_if;
mod line_to_block_comment;
mod merge_imports;
mod merge_match_arms;
mod merge_nested_if;
mod move_guard;
mod number_representation;
mod organize_imports;
mod promote_local_to_const;
mod pull_assignment_up;
mod remove_mut;
mod remove_parentheses;
mod remove_underscore;
mod remove_unused_imports;
mod remove_unused_param;
mod reorder_fields;
mod replace_if_with_match;
mod replace_string_with_char;
mod simplify_boolean;
mod sort_items;
mod split_import;
mod toggle_doc_comment;
mod unwrap_block;
mod unwrap_return_type;
mod wrap_return_type;

/// Return all registered assist handlers.
pub(crate) fn all() -> &'static [Handler] {
    &[
        add_explicit_type::add_explicit_type,
        auto_include::auto_include,
        invert_if::invert_if,
        flip_binexpr::flip_binexpr,
        apply_demorgan::apply_demorgan,
        extract_variable::extract_variable,
        extract_type_alias::extract_type_alias,
        extract_function::extract_function,
        inline_variable::inline_variable,
        generate_doc_template::generate_doc_template,
        unwrap_block::unwrap_block,
        pull_assignment_up::pull_assignment_up,
        guarded_return::guarded_return,
        sort_items::sort_items,
        line_to_block_comment::line_to_block_comment,
        block_to_line_comment::block_to_line_comment,
        toggle_doc_comment::toggle_doc_comment,
        bitfield_accessors::bitfield_accessors,
        evaluate_constant::evaluate_constant,
        simplify_boolean::simplify_boolean,
        organize_imports::organize_imports,
        remove_unused_imports::remove_unused_imports,
        generate_val_spec::generate_val_spec,
        add_missing_match_arms::add_missing_match_arms,
        generate_function::generate_function,
        add_missing_fields::add_missing_fields,
        fill_record_fields::fill_record_fields,
        convert_match_to_if::convert_match_to_if,
        merge_match_arms::merge_match_arms,
        move_guard::move_guard,
        remove_unused_param::remove_unused_param,
        replace_if_with_match::replace_if_with_match,
        inline_call::inline_call,
        generate_mapping_clause::generate_mapping_clause,
        add_scattered_end::add_scattered_end,
        change_visibility::change_visibility,
        flip_comma::flip_comma,
        remove_parentheses::remove_parentheses,
        convert_integer_literal::convert_integer_literal,
        destructure_struct_binding::destructure_struct_binding,
        expand_rest_pattern::expand_rest_pattern,
        flip_or_pattern::flip_or_pattern,
        generate_enum_variant::generate_enum_variant,
        reorder_fields::reorder_fields,
        unwrap_return_type::unwrap_return_type,
        wrap_return_type::wrap_return_type,
        add_turbo_fish::add_turbo_fish,
        merge_imports::merge_imports,
        number_representation::number_representation,
        replace_string_with_char::replace_string_with_char,
        split_import::split_import,
        inline_const_as_literal::inline_const_as_literal,
        promote_local_to_const::promote_local_to_const,
        remove_mut::remove_mut,
        generate_constant::generate_constant,
        convert_comment_style::convert_comment_style,
        extract_to_include::extract_to_include,
        add_braces::add_braces,
        convert_comment_block::convert_comment_block,
        merge_nested_if::merge_nested_if,
        remove_underscore::remove_underscore,
        generate_documentation_template::generate_documentation_template,
        generate_new::generate_new,
        generate_getter_or_setter::generate_getter_or_setter,
    ]
}
