//! Generated from `sail.ungram`, do not edit by hand.
//!
//! Run `cargo test -p syntax -- codegen` to validate.

#![allow(dead_code)]

use parser::SyntaxKind as SK;
use crate::syntax_node::{SyntaxNode, SyntaxToken};

use super::super::{support, AstNode};
use super::super::traits::{HasAttrs, HasName, HasVisibility};

// Top-level definitions

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallableDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for CallableDef {
    fn can_cast(kind: SK) -> bool { kind == SK::CALLABLE_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CALLABLE_DEF }
}

impl HasName for CallableDef {}
impl HasAttrs for CallableDef {}
impl HasVisibility for CallableDef {}

impl CallableDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn params(&self) -> Option<ParamList> { support::child(&self.syntax) }
    pub fn return_type(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallableSpec {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for CallableSpec {
    fn can_cast(kind: SK) -> bool { kind == SK::CALLABLE_SPEC }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CALLABLE_SPEC }
}

impl HasName for CallableSpec {}
impl HasAttrs for CallableSpec {}
impl HasVisibility for CallableSpec {}

impl CallableSpec {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn signature(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstraintDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ConstraintDef {
    fn can_cast(kind: SK) -> bool { kind == SK::CONSTRAINT_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CONSTRAINT_DEF }
}

impl HasAttrs for ConstraintDef {}

impl ConstraintDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn constraint(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DefaultDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for DefaultDef {
    fn can_cast(kind: SK) -> bool { kind == SK::DEFAULT_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::DEFAULT_DEF }
}

impl HasAttrs for DefaultDef {}

impl DefaultDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Definition {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Definition {
    fn can_cast(kind: SK) -> bool { kind == SK::DEFINITION }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::DEFINITION }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DirectiveDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for DirectiveDef {
    fn can_cast(kind: SK) -> bool { kind == SK::DIRECTIVE_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::DIRECTIVE_DEF }
}

impl HasAttrs for DirectiveDef {}

impl DirectiveDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for EndDef {
    fn can_cast(kind: SK) -> bool { kind == SK::END_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::END_DEF }
}

impl HasName for EndDef {}
impl HasAttrs for EndDef {}

impl EndDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FixityDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for FixityDef {
    fn can_cast(kind: SK) -> bool { kind == SK::FIXITY_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::FIXITY_DEF }
}

impl HasAttrs for FixityDef {}

impl FixityDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn operator(&self) -> Option<Name> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstantiationDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for InstantiationDef {
    fn can_cast(kind: SK) -> bool { kind == SK::INSTANTIATION_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::INSTANTIATION_DEF }
}

impl HasName for InstantiationDef {}
impl HasAttrs for InstantiationDef {}

impl InstantiationDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NamedDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for NamedDef {
    fn can_cast(kind: SK) -> bool { kind == SK::NAMED_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::NAMED_DEF }
}

impl HasName for NamedDef {}
impl HasAttrs for NamedDef {}
impl HasVisibility for NamedDef {}

impl NamedDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Body> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutcomeDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for OutcomeDef {
    fn can_cast(kind: SK) -> bool { kind == SK::OUTCOME_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::OUTCOME_DEF }
}

impl HasName for OutcomeDef {}
impl HasAttrs for OutcomeDef {}
impl HasVisibility for OutcomeDef {}

impl OutcomeDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn type_scheme(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScatteredClauseDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ScatteredClauseDef {
    fn can_cast(kind: SK) -> bool { kind == SK::SCATTERED_CLAUSE_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::SCATTERED_CLAUSE_DEF }
}

impl HasName for ScatteredClauseDef {}
impl HasAttrs for ScatteredClauseDef {}
impl HasVisibility for ScatteredClauseDef {}

impl ScatteredClauseDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScatteredDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ScatteredDef {
    fn can_cast(kind: SK) -> bool { kind == SK::SCATTERED_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::SCATTERED_DEF }
}

impl HasName for ScatteredDef {}
impl HasAttrs for ScatteredDef {}
impl HasVisibility for ScatteredDef {}

impl ScatteredDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn signature(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceFile {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for SourceFile {
    fn can_cast(kind: SK) -> bool { kind == SK::SOURCE_FILE }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::SOURCE_FILE }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TerminationMeasureDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TerminationMeasureDef {
    fn can_cast(kind: SK) -> bool { kind == SK::TERMINATION_MEASURE_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TERMINATION_MEASURE_DEF }
}

impl HasName for TerminationMeasureDef {}
impl HasAttrs for TerminationMeasureDef {}

impl TerminationMeasureDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeAliasDef {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeAliasDef {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_ALIAS_DEF }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_ALIAS_DEF }
}

impl HasName for TypeAliasDef {}
impl HasAttrs for TypeAliasDef {}
impl HasVisibility for TypeAliasDef {}

impl TypeAliasDef {
    pub fn attributes(&self) -> Vec<Attribute> { support::children(&self.syntax) }
    pub fn visibility(&self) -> Option<Visibility> { support::child(&self.syntax) }
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn target(&self) -> Option<Type> { support::child(&self.syntax) }
}

// Expressions

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssertExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for AssertExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::ASSERT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::ASSERT_EXPR }
}

impl AssertExpr {
    pub fn condition(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn message(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssignExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for AssignExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::ASSIGN_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::ASSIGN_EXPR }
}

impl AssignExpr {
    pub fn target(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn value(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BinExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for BinExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::BIN_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::BIN_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for BlockExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::BLOCK_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::BLOCK_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for CallExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::CALL_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CALL_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CastExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for CastExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::CAST_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CAST_EXPR }
}

impl CastExpr {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ConfigExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::CONFIG_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CONFIG_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstraintExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ConstraintExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::CONSTRAINT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::CONSTRAINT_EXPR }
}

impl ConstraintExpr {
    pub fn constraint(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExitExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ExitExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::EXIT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::EXIT_EXPR }
}

impl ExitExpr {
    pub fn value(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldAccessExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for FieldAccessExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::FIELD_ACCESS_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::FIELD_ACCESS_EXPR }
}

impl FieldAccessExpr {
    pub fn base(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForeachExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ForeachExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::FOREACH_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::FOREACH_EXPR }
}

impl ForeachExpr {
    pub fn start(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn end_expr(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
    pub fn step(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(2)
    }
    pub fn body(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(3)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for IdentExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::IDENT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::IDENT_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IfExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for IfExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::IF_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::IF_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for IndexExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::INDEX_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::INDEX_EXPR }
}

impl IndexExpr {
    pub fn base(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn index(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LetExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for LetExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::LET_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::LET_EXPR }
}

impl LetExpr {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
    pub fn value(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ListExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::LIST_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::LIST_EXPR }
}

impl ListExpr {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiteralExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for LiteralExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::LITERAL_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::LITERAL_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for MatchExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::MATCH_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::MATCH_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrefixExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for PrefixExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::PREFIX_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::PREFIX_EXPR }
}

impl PrefixExpr {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for RefExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::REF_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::REF_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepeatExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for RepeatExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::REPEAT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::REPEAT_EXPR }
}

impl RepeatExpr {
    pub fn body(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn condition(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReturnExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ReturnExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::RETURN_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::RETURN_EXPR }
}

impl ReturnExpr {
    pub fn value(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SizeofExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for SizeofExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::SIZEOF_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::SIZEOF_EXPR }
}

impl SizeofExpr {
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for StructExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::STRUCT_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::STRUCT_EXPR }
}

impl HasName for StructExpr {}

impl StructExpr {
    pub fn name(&self) -> Option<Name> { support::child(&self.syntax) }
    pub fn fields(&self) -> Vec<FieldInit> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubrangeExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for SubrangeExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::SUBRANGE_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::SUBRANGE_EXPR }
}

impl SubrangeExpr {
    pub fn base(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn hi(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
    pub fn lo(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThrowExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ThrowExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::THROW_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::THROW_EXPR }
}

impl ThrowExpr {
    pub fn value(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TryExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TryExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::TRY_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TRY_EXPR }
}

impl TryExpr {
    pub fn body(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn arms(&self) -> Vec<MatchArm> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TupleExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TupleExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::TUPLE_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TUPLE_EXPR }
}

impl TupleExpr {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TyvarExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TyvarExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::TYVAR_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYVAR_EXPR }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UpdateExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for UpdateExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::UPDATE_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::UPDATE_EXPR }
}

impl UpdateExpr {
    pub fn base(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn fields(&self) -> Vec<FieldInit> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VarExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for VarExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::VAR_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::VAR_EXPR }
}

impl VarExpr {
    pub fn target(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn init(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
    pub fn body(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VectorExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for VectorExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::VECTOR_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::VECTOR_EXPR }
}

impl VectorExpr {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VectorUpdateExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for VectorUpdateExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::VECTOR_UPDATE_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::VECTOR_UPDATE_EXPR }
}

impl VectorUpdateExpr {
    pub fn base(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn field_inits(&self) -> Vec<FieldInit> { support::children(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WhileExpr {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for WhileExpr {
    fn can_cast(kind: SK) -> bool { kind == SK::WHILE_EXPR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::WHILE_EXPR }
}

impl WhileExpr {
    pub fn condition(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

// Patterns

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for AppPat {
    fn can_cast(kind: SK) -> bool { kind == SK::APP_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::APP_PAT }
}

impl AppPat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AsPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for AsPat {
    fn can_cast(kind: SK) -> bool { kind == SK::AS_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::AS_PAT }
}

impl AsPat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BinPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for BinPat {
    fn can_cast(kind: SK) -> bool { kind == SK::BIN_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::BIN_PAT }
}

impl BinPat {
    pub fn lhs(&self) -> Option<Pat> { support::child(&self.syntax) }
    pub fn rhs(&self) -> Option<Pat> {
        let mut iter = self.syntax.children().filter_map(Pat::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for IdentPat {
    fn can_cast(kind: SK) -> bool { kind == SK::IDENT_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::IDENT_PAT }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for IndexPat {
    fn can_cast(kind: SK) -> bool { kind == SK::INDEX_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::INDEX_PAT }
}

impl IndexPat {
    pub fn index(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ListPat {
    fn can_cast(kind: SK) -> bool { kind == SK::LIST_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::LIST_PAT }
}

impl ListPat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiteralPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for LiteralPat {
    fn can_cast(kind: SK) -> bool { kind == SK::LITERAL_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::LITERAL_PAT }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RangeIndexPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for RangeIndexPat {
    fn can_cast(kind: SK) -> bool { kind == SK::RANGE_INDEX_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::RANGE_INDEX_PAT }
}

impl RangeIndexPat {
    pub fn hi(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn lo(&self) -> Option<Type> {
        let mut iter = self.syntax.children().filter_map(Type::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for StructPat {
    fn can_cast(kind: SK) -> bool { kind == SK::STRUCT_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::STRUCT_PAT }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TuplePat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TuplePat {
    fn can_cast(kind: SK) -> bool { kind == SK::TUPLE_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TUPLE_PAT }
}

impl TuplePat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypedPat {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPED_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPED_PAT }
}

impl TypedPat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TyvarPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TyvarPat {
    fn can_cast(kind: SK) -> bool { kind == SK::TYVAR_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYVAR_PAT }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VectorPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for VectorPat {
    fn can_cast(kind: SK) -> bool { kind == SK::VECTOR_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::VECTOR_PAT }
}

impl VectorPat {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WildPat {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for WildPat {
    fn can_cast(kind: SK) -> bool { kind == SK::WILD_PAT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::WILD_PAT }
}

// Type expressions

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeApp {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeApp {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_APP }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_APP }
}

impl TypeApp {
    pub fn r#type(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeArrow {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeArrow {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_ARROW }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_ARROW }
}

impl TypeArrow {
    pub fn params(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn ret(&self) -> Option<Type> {
        let mut iter = self.syntax.children().filter_map(Type::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeEffect {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeEffect {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_EFFECT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_EFFECT }
}

impl TypeEffect {
    pub fn ty(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeExistential {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeExistential {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_EXISTENTIAL }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_EXISTENTIAL }
}

impl TypeExistential {
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn constraint(&self) -> Option<Type> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Type> {
        let mut iter = self.syntax.children().filter_map(Type::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeForall {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeForall {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_FORALL }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_FORALL }
}

impl TypeForall {
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeNamed {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeNamed {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_NAMED }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_NAMED }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeParamList {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeParamList {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_PARAM_LIST }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_PARAM_LIST }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeTuple {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeTuple {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_TUPLE }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_TUPLE }
}

impl TypeTuple {
    pub fn r#type(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for TypeVar {
    fn can_cast(kind: SK) -> bool { kind == SK::TYPE_VAR }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::TYPE_VAR }
}

// Sub-structures

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArgList {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ArgList {
    fn can_cast(kind: SK) -> bool { kind == SK::ARG_LIST }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::ARG_LIST }
}

impl ArgList {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attribute {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Attribute {
    fn can_cast(kind: SK) -> bool { kind == SK::ATTRIBUTE }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::ATTRIBUTE }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockItem {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for BlockItem {
    fn can_cast(kind: SK) -> bool { kind == SK::BLOCK_ITEM }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::BLOCK_ITEM }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Body {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Body {
    fn can_cast(kind: SK) -> bool { kind == SK::BODY }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::BODY }
}

impl Body {
    pub fn expr(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldInit {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for FieldInit {
    fn can_cast(kind: SK) -> bool { kind == SK::FIELD_INIT }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::FIELD_INIT }
}

impl FieldInit {
    pub fn value(&self) -> Option<Expr> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchArm {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for MatchArm {
    fn can_cast(kind: SK) -> bool { kind == SK::MATCH_ARM }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::MATCH_ARM }
}

impl MatchArm {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
    pub fn guard(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn body(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Name {
    fn can_cast(kind: SK) -> bool { kind == SK::NAME }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::NAME }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamList {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for ParamList {
    fn can_cast(kind: SK) -> bool { kind == SK::PARAM_LIST }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::PARAM_LIST }
}

impl ParamList {
    pub fn pat(&self) -> Option<Pat> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Quantifier {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Quantifier {
    fn can_cast(kind: SK) -> bool { kind == SK::QUANTIFIER }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::QUANTIFIER }
}

impl Quantifier {
    pub fn type_param_list(&self) -> Option<TypeParamList> { support::child(&self.syntax) }
    pub fn constraint(&self) -> Option<Type> { support::child(&self.syntax) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Visibility {
    pub(crate) syntax: SyntaxNode,
}

impl AstNode for Visibility {
    fn can_cast(kind: SK) -> bool { kind == SK::VISIBILITY }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
    }
    fn syntax(&self) -> &SyntaxNode { &self.syntax }
    fn kind() -> SK { SK::VISIBILITY }
}

// Enum types (virtual alternation nodes)

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    LiteralExpr(LiteralExpr),
    IdentExpr(IdentExpr),
    TyvarExpr(TyvarExpr),
    RefExpr(RefExpr),
    BinExpr(BinExpr),
    PrefixExpr(PrefixExpr),
    CallExpr(CallExpr),
    FieldAccessExpr(FieldAccessExpr),
    IndexExpr(IndexExpr),
    SubrangeExpr(SubrangeExpr),
    VectorUpdateExpr(VectorUpdateExpr),
    IfExpr(IfExpr),
    MatchExpr(MatchExpr),
    TryExpr(TryExpr),
    BlockExpr(BlockExpr),
    LetExpr(LetExpr),
    VarExpr(VarExpr),
    ReturnExpr(ReturnExpr),
    ThrowExpr(ThrowExpr),
    ExitExpr(ExitExpr),
    AssertExpr(AssertExpr),
    AssignExpr(AssignExpr),
    CastExpr(CastExpr),
    ForeachExpr(ForeachExpr),
    WhileExpr(WhileExpr),
    RepeatExpr(RepeatExpr),
    TupleExpr(TupleExpr),
    ListExpr(ListExpr),
    VectorExpr(VectorExpr),
    StructExpr(StructExpr),
    UpdateExpr(UpdateExpr),
    SizeofExpr(SizeofExpr),
    ConstraintExpr(ConstraintExpr),
    ConfigExpr(ConfigExpr),
}

impl AstNode for Expr {
    fn can_cast(kind: SK) -> bool {
        matches!(kind, SK::LITERAL_EXPR | SK::IDENT_EXPR | SK::TYVAR_EXPR | SK::REF_EXPR | SK::BIN_EXPR | SK::PREFIX_EXPR | SK::CALL_EXPR | SK::FIELD_ACCESS_EXPR | SK::INDEX_EXPR | SK::SUBRANGE_EXPR | SK::VECTOR_UPDATE_EXPR | SK::IF_EXPR | SK::MATCH_EXPR | SK::TRY_EXPR | SK::BLOCK_EXPR | SK::LET_EXPR | SK::VAR_EXPR | SK::RETURN_EXPR | SK::THROW_EXPR | SK::EXIT_EXPR | SK::ASSERT_EXPR | SK::ASSIGN_EXPR | SK::CAST_EXPR | SK::FOREACH_EXPR | SK::WHILE_EXPR | SK::REPEAT_EXPR | SK::TUPLE_EXPR | SK::LIST_EXPR | SK::VECTOR_EXPR | SK::STRUCT_EXPR | SK::UPDATE_EXPR | SK::SIZEOF_EXPR | SK::CONSTRAINT_EXPR | SK::CONFIG_EXPR)
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        match syntax.kind() {
            SK::LITERAL_EXPR => Some(Expr::LiteralExpr(LiteralExpr { syntax })),
            SK::IDENT_EXPR => Some(Expr::IdentExpr(IdentExpr { syntax })),
            SK::TYVAR_EXPR => Some(Expr::TyvarExpr(TyvarExpr { syntax })),
            SK::REF_EXPR => Some(Expr::RefExpr(RefExpr { syntax })),
            SK::BIN_EXPR => Some(Expr::BinExpr(BinExpr { syntax })),
            SK::PREFIX_EXPR => Some(Expr::PrefixExpr(PrefixExpr { syntax })),
            SK::CALL_EXPR => Some(Expr::CallExpr(CallExpr { syntax })),
            SK::FIELD_ACCESS_EXPR => Some(Expr::FieldAccessExpr(FieldAccessExpr { syntax })),
            SK::INDEX_EXPR => Some(Expr::IndexExpr(IndexExpr { syntax })),
            SK::SUBRANGE_EXPR => Some(Expr::SubrangeExpr(SubrangeExpr { syntax })),
            SK::VECTOR_UPDATE_EXPR => Some(Expr::VectorUpdateExpr(VectorUpdateExpr { syntax })),
            SK::IF_EXPR => Some(Expr::IfExpr(IfExpr { syntax })),
            SK::MATCH_EXPR => Some(Expr::MatchExpr(MatchExpr { syntax })),
            SK::TRY_EXPR => Some(Expr::TryExpr(TryExpr { syntax })),
            SK::BLOCK_EXPR => Some(Expr::BlockExpr(BlockExpr { syntax })),
            SK::LET_EXPR => Some(Expr::LetExpr(LetExpr { syntax })),
            SK::VAR_EXPR => Some(Expr::VarExpr(VarExpr { syntax })),
            SK::RETURN_EXPR => Some(Expr::ReturnExpr(ReturnExpr { syntax })),
            SK::THROW_EXPR => Some(Expr::ThrowExpr(ThrowExpr { syntax })),
            SK::EXIT_EXPR => Some(Expr::ExitExpr(ExitExpr { syntax })),
            SK::ASSERT_EXPR => Some(Expr::AssertExpr(AssertExpr { syntax })),
            SK::ASSIGN_EXPR => Some(Expr::AssignExpr(AssignExpr { syntax })),
            SK::CAST_EXPR => Some(Expr::CastExpr(CastExpr { syntax })),
            SK::FOREACH_EXPR => Some(Expr::ForeachExpr(ForeachExpr { syntax })),
            SK::WHILE_EXPR => Some(Expr::WhileExpr(WhileExpr { syntax })),
            SK::REPEAT_EXPR => Some(Expr::RepeatExpr(RepeatExpr { syntax })),
            SK::TUPLE_EXPR => Some(Expr::TupleExpr(TupleExpr { syntax })),
            SK::LIST_EXPR => Some(Expr::ListExpr(ListExpr { syntax })),
            SK::VECTOR_EXPR => Some(Expr::VectorExpr(VectorExpr { syntax })),
            SK::STRUCT_EXPR => Some(Expr::StructExpr(StructExpr { syntax })),
            SK::UPDATE_EXPR => Some(Expr::UpdateExpr(UpdateExpr { syntax })),
            SK::SIZEOF_EXPR => Some(Expr::SizeofExpr(SizeofExpr { syntax })),
            SK::CONSTRAINT_EXPR => Some(Expr::ConstraintExpr(ConstraintExpr { syntax })),
            SK::CONFIG_EXPR => Some(Expr::ConfigExpr(ConfigExpr { syntax })),
            _ => None,
        }
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Expr::LiteralExpr(it) => &it.syntax,
            Expr::IdentExpr(it) => &it.syntax,
            Expr::TyvarExpr(it) => &it.syntax,
            Expr::RefExpr(it) => &it.syntax,
            Expr::BinExpr(it) => &it.syntax,
            Expr::PrefixExpr(it) => &it.syntax,
            Expr::CallExpr(it) => &it.syntax,
            Expr::FieldAccessExpr(it) => &it.syntax,
            Expr::IndexExpr(it) => &it.syntax,
            Expr::SubrangeExpr(it) => &it.syntax,
            Expr::VectorUpdateExpr(it) => &it.syntax,
            Expr::IfExpr(it) => &it.syntax,
            Expr::MatchExpr(it) => &it.syntax,
            Expr::TryExpr(it) => &it.syntax,
            Expr::BlockExpr(it) => &it.syntax,
            Expr::LetExpr(it) => &it.syntax,
            Expr::VarExpr(it) => &it.syntax,
            Expr::ReturnExpr(it) => &it.syntax,
            Expr::ThrowExpr(it) => &it.syntax,
            Expr::ExitExpr(it) => &it.syntax,
            Expr::AssertExpr(it) => &it.syntax,
            Expr::AssignExpr(it) => &it.syntax,
            Expr::CastExpr(it) => &it.syntax,
            Expr::ForeachExpr(it) => &it.syntax,
            Expr::WhileExpr(it) => &it.syntax,
            Expr::RepeatExpr(it) => &it.syntax,
            Expr::TupleExpr(it) => &it.syntax,
            Expr::ListExpr(it) => &it.syntax,
            Expr::VectorExpr(it) => &it.syntax,
            Expr::StructExpr(it) => &it.syntax,
            Expr::UpdateExpr(it) => &it.syntax,
            Expr::SizeofExpr(it) => &it.syntax,
            Expr::ConstraintExpr(it) => &it.syntax,
            Expr::ConfigExpr(it) => &it.syntax,
        }
    }
    fn kind() -> SK { unimplemented!("enum Expr has multiple kinds") }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pat {
    WildPat(WildPat),
    LiteralPat(LiteralPat),
    IdentPat(IdentPat),
    TyvarPat(TyvarPat),
    TypedPat(TypedPat),
    TuplePat(TuplePat),
    ListPat(ListPat),
    VectorPat(VectorPat),
    AppPat(AppPat),
    StructPat(StructPat),
    BinPat(BinPat),
    IndexPat(IndexPat),
    RangeIndexPat(RangeIndexPat),
    AsPat(AsPat),
}

impl AstNode for Pat {
    fn can_cast(kind: SK) -> bool {
        matches!(kind, SK::WILD_PAT | SK::LITERAL_PAT | SK::IDENT_PAT | SK::TYVAR_PAT | SK::TYPED_PAT | SK::TUPLE_PAT | SK::LIST_PAT | SK::VECTOR_PAT | SK::APP_PAT | SK::STRUCT_PAT | SK::BIN_PAT | SK::INDEX_PAT | SK::RANGE_INDEX_PAT | SK::AS_PAT)
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        match syntax.kind() {
            SK::WILD_PAT => Some(Pat::WildPat(WildPat { syntax })),
            SK::LITERAL_PAT => Some(Pat::LiteralPat(LiteralPat { syntax })),
            SK::IDENT_PAT => Some(Pat::IdentPat(IdentPat { syntax })),
            SK::TYVAR_PAT => Some(Pat::TyvarPat(TyvarPat { syntax })),
            SK::TYPED_PAT => Some(Pat::TypedPat(TypedPat { syntax })),
            SK::TUPLE_PAT => Some(Pat::TuplePat(TuplePat { syntax })),
            SK::LIST_PAT => Some(Pat::ListPat(ListPat { syntax })),
            SK::VECTOR_PAT => Some(Pat::VectorPat(VectorPat { syntax })),
            SK::APP_PAT => Some(Pat::AppPat(AppPat { syntax })),
            SK::STRUCT_PAT => Some(Pat::StructPat(StructPat { syntax })),
            SK::BIN_PAT => Some(Pat::BinPat(BinPat { syntax })),
            SK::INDEX_PAT => Some(Pat::IndexPat(IndexPat { syntax })),
            SK::RANGE_INDEX_PAT => Some(Pat::RangeIndexPat(RangeIndexPat { syntax })),
            SK::AS_PAT => Some(Pat::AsPat(AsPat { syntax })),
            _ => None,
        }
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Pat::WildPat(it) => &it.syntax,
            Pat::LiteralPat(it) => &it.syntax,
            Pat::IdentPat(it) => &it.syntax,
            Pat::TyvarPat(it) => &it.syntax,
            Pat::TypedPat(it) => &it.syntax,
            Pat::TuplePat(it) => &it.syntax,
            Pat::ListPat(it) => &it.syntax,
            Pat::VectorPat(it) => &it.syntax,
            Pat::AppPat(it) => &it.syntax,
            Pat::StructPat(it) => &it.syntax,
            Pat::BinPat(it) => &it.syntax,
            Pat::IndexPat(it) => &it.syntax,
            Pat::RangeIndexPat(it) => &it.syntax,
            Pat::AsPat(it) => &it.syntax,
        }
    }
    fn kind() -> SK { unimplemented!("enum Pat has multiple kinds") }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    TypeNamed(TypeNamed),
    TypeVar(TypeVar),
    TypeApp(TypeApp),
    TypeTuple(TypeTuple),
    TypeArrow(TypeArrow),
    TypeForall(TypeForall),
    TypeExistential(TypeExistential),
    TypeEffect(TypeEffect),
}

impl AstNode for Type {
    fn can_cast(kind: SK) -> bool {
        matches!(kind, SK::TYPE_NAMED | SK::TYPE_VAR | SK::TYPE_APP | SK::TYPE_TUPLE | SK::TYPE_ARROW | SK::TYPE_FORALL | SK::TYPE_EXISTENTIAL | SK::TYPE_EFFECT)
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        match syntax.kind() {
            SK::TYPE_NAMED => Some(Type::TypeNamed(TypeNamed { syntax })),
            SK::TYPE_VAR => Some(Type::TypeVar(TypeVar { syntax })),
            SK::TYPE_APP => Some(Type::TypeApp(TypeApp { syntax })),
            SK::TYPE_TUPLE => Some(Type::TypeTuple(TypeTuple { syntax })),
            SK::TYPE_ARROW => Some(Type::TypeArrow(TypeArrow { syntax })),
            SK::TYPE_FORALL => Some(Type::TypeForall(TypeForall { syntax })),
            SK::TYPE_EXISTENTIAL => Some(Type::TypeExistential(TypeExistential { syntax })),
            SK::TYPE_EFFECT => Some(Type::TypeEffect(TypeEffect { syntax })),
            _ => None,
        }
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Type::TypeNamed(it) => &it.syntax,
            Type::TypeVar(it) => &it.syntax,
            Type::TypeApp(it) => &it.syntax,
            Type::TypeTuple(it) => &it.syntax,
            Type::TypeArrow(it) => &it.syntax,
            Type::TypeForall(it) => &it.syntax,
            Type::TypeExistential(it) => &it.syntax,
            Type::TypeEffect(it) => &it.syntax,
        }
    }
    fn kind() -> SK { unimplemented!("enum Type has multiple kinds") }
}


// ════════════════════════════════════════════════════════════════
// Hand-written accessor methods (complex logic beyond codegen)
// ════════════════════════════════════════════════════════════════

// CST children are concrete def types (CALLABLE_DEF, etc.), not
// DEFINITION wrapper nodes. Use raw SyntaxNode iteration.

impl SourceFile {
    pub fn definitions(&self) -> Vec<Definition> {
        support::children::<Definition>(&self.syntax)
    }
    pub fn definition_nodes(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        self.syntax.children()
    }
    pub fn callable_defs(&self) -> Vec<CallableDef> {
        support::children::<CallableDef>(&self.syntax)
    }
    pub fn callable_specs(&self) -> Vec<CallableSpec> {
        support::children::<CallableSpec>(&self.syntax)
    }
}

// Two children of the same type (Expr) require positional access.

impl BinExpr {
    pub fn lhs(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn op(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia() && !matches!(t.kind(), SK::IDENT | SK::NUM_LIT))
    }
    pub fn rhs(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
}

impl CallExpr {
    pub fn callee(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn arg_list(&self) -> Option<ArgList> { support::child(&self.syntax) }
}

// Three Expr children: condition, then_branch, else_branch.

impl IfExpr {
    pub fn condition(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn then_branch(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(1)
    }
    pub fn else_branch(&self) -> Option<Expr> {
        let mut iter = self.syntax.children().filter_map(Expr::cast);
        iter.nth(2)
    }
}

impl BlockExpr {
    pub fn items(&self) -> Vec<BlockItem> {
        support::children::<BlockItem>(&self.syntax)
    }
}

impl MatchExpr {
    pub fn scrutinee(&self) -> Option<Expr> { support::child(&self.syntax) }
    pub fn arms(&self) -> Vec<MatchArm> {
        support::children::<MatchArm>(&self.syntax)
    }
}

// Raw IDENT token access for backward compatibility and display.

impl CallableDef {
    pub fn name_ident(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}

impl CallableSpec {
    pub fn name_ident(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}

impl NamedDef {
    pub fn name_ident(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}

impl IdentPat {
    pub fn name_ident(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}

impl AppPat {
    pub fn ctor_name(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}

impl TypeNamed {
    pub fn name_ident(&self) -> Option<SyntaxToken> { support::ident_token(&self.syntax) }
}
