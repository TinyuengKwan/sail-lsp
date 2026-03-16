mod ast;
pub mod core_ast;
mod full_parser;
mod lexer;
mod parser;
mod queries;

pub use core_ast::{
    Attribute, AttributeData, AttributeEntry, BitfieldField, BlockItem, Call, CallableClause,
    CallableDefKind, CallableQuantifier, CallableSpecKind, DefModifiers, EnumFunction, EnumMember,
    Expr, ExternBinding, ExternPurity, ExternSpec, FieldExpr, FieldPattern, FixityKind,
    ForeachExpr, InstantiationSubstitution, LetBinding, Literal, LoopMeasure, MappingArm,
    MappingArmDirection, MappingBody, MatchCase, NamedDefDetail, NamedDefKind, Pattern,
    QuantifierVar, RecMeasure, ScatteredClauseKind, ScatteredKind, Spanned, TerminationMeasureKind,
    TypeArrowKind, TypeExpr, TypeParam, TypeParamSpec, TypeParamTail, TypedField, UnionPayload,
    UnionVariant, VectorUpdate,
};
pub use full_parser::{parse_core_source, parse_expr_fragment};
pub use lexer::*;
pub use parser::*;
pub use queries::{
    find_binding_value_at_span_core as find_binding_value_at_span,
    find_call_at_offset_core as find_call_at_offset,
    find_enum_name_for_member_core as find_enum_name_for_member,
    find_named_members_core as find_named_members,
    find_top_level_item_span_core as find_top_level_item_span, BindingValueAtSpan, CallAtOffset,
};
