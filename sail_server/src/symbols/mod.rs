pub mod analysis;
pub mod calls;
pub mod lenses;
pub mod navigation;
pub mod references;

pub(crate) use analysis::{
    add_parsed_definitions, build_signature_index, builtin_docs, collect_callable_signatures,
    extract_comments, extract_symbol_decls, find_callable_signature, function_snippet,
    inlay_param_name, instantiate_signature, token_is_close_bracket, token_is_open_bracket,
    token_symbol_key, CallableSignature, Parameter,
};
#[cfg(test)]
pub(crate) use analysis::infer_binding_type;
pub(crate) use calls::{find_call_at_position, signature_help_for_position};
pub(crate) use lenses::{
    code_lens_title, code_lenses_for_file, collect_implementation_counts, collect_reference_counts,
};
pub(crate) use navigation::{
    call_edges_from, call_edges_to, call_hierarchy_item, implementation_locations,
    parse_named_type, resolve_workspace_symbol, symbol_declaration_locations,
    symbol_definition_locations, type_definition_locations, type_hierarchy_item,
    type_name_candidates_at_position, type_subtypes, type_supertypes, typed_bindings,
    will_rename_file_edits,
};
#[cfg(test)]
pub(crate) use navigation::{call_edges_for_file, type_alias_edges};
pub(crate) use references::{
    normalize_validated_rename, reference_locations, rename_edits, resolve_symbol_at,
    symbol_spans_for_file,
};
