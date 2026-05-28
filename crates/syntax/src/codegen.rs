//! Grammar code generation from `sail.ungram`.
//!
//! specification and produces Rust source code for:
//! - `SyntaxKind` enum (`parser/src/syntax_kind/generated.rs`)
//! - AST node types (`syntax/src/ast/generated/nodes.rs`)
//!
//! Generated files are committed to git. Run the
//! `codegen_smoke_test` test to regenerate and verify.

use std::collections::BTreeSet;

/// Intermediate representation produced by `lower()`.
#[derive(Default, Debug)]
pub struct AstSrc {
    /// Composite node names (structs in generated AST).
    pub nodes: Vec<AstNodeSrc>,
    /// Alternation node names (enums in generated AST).
    pub enums: Vec<AstEnumSrc>,
}

/// A concrete AST node (struct).
#[derive(Debug)]
pub struct AstNodeSrc {
    pub name: String,
    pub fields: Vec<Field>,
}

/// An alternation AST node (enum).
#[derive(Debug)]
pub struct AstEnumSrc {
    pub name: String,
    pub variants: Vec<String>,
}

/// A field in an AST node.
#[derive(Debug)]
pub enum Field {
    /// A child token (terminal).
    Token(String),
    /// A child node (non-terminal).
    Node { name: String, ty: String, cardinality: Cardinality },
}

/// How many children of this type are expected.
#[derive(Debug, Clone, Copy)]
pub enum Cardinality {
    Optional,
    Many,
}

/// Lower an ungrammar `Grammar` into `AstSrc`.
/// Each grammar rule is classified as either:
/// - An **enum** (if the rule is a pure alternation: `A | B | C`)
/// - A **struct** (if the rule has sequences, tokens, labeled fields)
pub fn lower(grammar: &ungrammar::Grammar) -> AstSrc {
    let mut res = AstSrc::default();

    for node in grammar.iter() {
        let name = grammar[node].name.clone();
        let rule = &grammar[node].rule;

        if let Some(variants) = lower_enum(grammar, rule) {
            res.enums.push(AstEnumSrc { name, variants });
        } else {
            let mut fields = Vec::new();
            lower_rule(&mut fields, grammar, rule);
            res.nodes.push(AstNodeSrc { name, fields });
        }
    }

    res.nodes.sort_by(|a, b| a.name.cmp(&b.name));
    res.enums.sort_by(|a, b| a.name.cmp(&b.name));
    res
}

/// Check if a rule is a pure alternation (only `|` of Node references).
/// If so, return the variant names.
fn lower_enum(grammar: &ungrammar::Grammar, rule: &ungrammar::Rule) -> Option<Vec<String>> {
    let alts = match rule {
        ungrammar::Rule::Alt(alts) => alts,
        _ => return None,
    };

    let mut variants = Vec::new();
    for alt in alts {
        match alt {
            ungrammar::Rule::Node(node) => {
                variants.push(grammar[*node].name.clone());
            }
            _ => return None, // Not a pure alternation
        }
    }
    Some(variants)
}

/// Lower a rule into fields for a struct node.
fn lower_rule(fields: &mut Vec<Field>, grammar: &ungrammar::Grammar, rule: &ungrammar::Rule) {
    match rule {
        ungrammar::Rule::Labeled { label, rule } => {
            // labeled:Node → Field::Node { name: label, ty: node_name }
            match rule.as_ref() {
                ungrammar::Rule::Node(node) => {
                    fields.push(Field::Node {
                        name: label.clone(),
                        ty: grammar[*node].name.clone(),
                        cardinality: Cardinality::Optional,
                    });
                }
                ungrammar::Rule::Token(token) => {
                    fields.push(Field::Token(grammar[*token].name.clone()));
                }
                ungrammar::Rule::Opt(inner) => match inner.as_ref() {
                    ungrammar::Rule::Node(node) => {
                        fields.push(Field::Node {
                            name: label.clone(),
                            ty: grammar[*node].name.clone(),
                            cardinality: Cardinality::Optional,
                        });
                    }
                    _ => lower_rule(fields, grammar, inner),
                },
                ungrammar::Rule::Rep(inner) => match inner.as_ref() {
                    ungrammar::Rule::Node(node) => {
                        fields.push(Field::Node {
                            name: label.clone(),
                            ty: grammar[*node].name.clone(),
                            cardinality: Cardinality::Many,
                        });
                    }
                    _ => lower_rule(fields, grammar, rule),
                },
                _ => {
                    lower_rule(fields, grammar, rule);
                }
            }
        }
        ungrammar::Rule::Node(node) => {
            let name = grammar[*node].name.clone();
            fields.push(Field::Node {
                name: to_lower_snake_case(&name),
                ty: name,
                cardinality: Cardinality::Optional,
            });
        }
        ungrammar::Rule::Token(token) => {
            fields.push(Field::Token(grammar[*token].name.clone()));
        }
        ungrammar::Rule::Seq(rules) => {
            for r in rules {
                lower_rule(fields, grammar, r);
            }
        }
        ungrammar::Rule::Alt(_) => {
            // Nested alternation inside a struct — skip (handled by enum)
        }
        ungrammar::Rule::Opt(inner) => {
            lower_rule(fields, grammar, inner);
        }
        ungrammar::Rule::Rep(inner) => {
            // Repetition → Field::Node with Many cardinality
            match inner.as_ref() {
                ungrammar::Rule::Node(node) => {
                    let name = grammar[*node].name.clone();
                    fields.push(Field::Node {
                        name: pluralize(&to_lower_snake_case(&name)),
                        ty: name,
                        cardinality: Cardinality::Many,
                    });
                }
                _ => {
                    lower_rule(fields, grammar, inner);
                }
            }
        }
    }
}

/// Collect all node names that appear in the grammar.
/// Used to generate the composite SyntaxKind variants.
pub fn collect_node_names(ast: &AstSrc) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for node in &ast.nodes {
        names.insert(node.name.clone());
    }
    for en in &ast.enums {
        names.insert(en.name.clone());
    }
    names
}

/// PascalCase → UPPER_SNAKE_CASE.
pub fn to_upper_snake_case(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            buf.push('_');
        }
        prev_lower = c.is_ascii_lowercase();
        buf.push(c.to_ascii_uppercase());
    }
    buf
}

/// PascalCase → lower_snake_case.
pub fn to_lower_snake_case(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            buf.push('_');
        }
        prev_lower = c.is_ascii_lowercase();
        buf.push(c.to_ascii_lowercase());
    }
    buf
}

/// Rust keywords that must be escaped with `r#` when used as identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

/// Escape a field name if it's a Rust keyword.
fn escape_keyword(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// Simple pluralization for field names.
fn pluralize(s: &str) -> String {
    if s.ends_with('s') || s.ends_with("sh") || s.ends_with("ch") {
        format!("{s}es")
    } else if s.ends_with('y') && !s.ends_with("ey") {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{s}s")
    }
}

/// Generate the list of composite SyntaxKind variant names from AstSrc.
///
/// These are the node variants (not tokens, not sentinels). The full
/// SyntaxKind enum also includes hand-written token variants.
///
/// Returns variant names in UPPER_SNAKE_CASE (e.g., "CALLABLE_DEF").
/// Nodes that appear in sail.ungram but are NOT SyntaxKind variants.
/// These are abstract grouping nodes or sub-structure helpers that
/// exist only in the ungrammar for organization.
const UNGRAM_ONLY_NODES: &[&str] = &[
    "Expr",      // alternation — not a concrete CST node kind
    "Pat",       // alternation
    "Type",      // alternation
    "FieldPat",  // sub-structure inside StructPat (no SyntaxKind)
    "TypeParam", // sub-structure inside TypeParamList (no SyntaxKind)
];

pub fn generate_composite_kinds(ast: &AstSrc) -> Vec<String> {
    let mut kinds = Vec::new();
    // Struct nodes
    for node in &ast.nodes {
        if UNGRAM_ONLY_NODES.contains(&node.name.as_str()) {
            continue;
        }
        kinds.push(to_upper_snake_case(&node.name));
    }
    // Enum nodes (alternations that ARE concrete SyntaxKind variants)
    for en in &ast.enums {
        if UNGRAM_ONLY_NODES.contains(&en.name.as_str()) {
            continue;
        }
        kinds.push(to_upper_snake_case(&en.name));
    }
    kinds.sort();
    kinds
}

/// Validate that generated composite kinds match the current hand-written
/// SyntaxKind enum. Returns the set of differences.
pub fn diff_composite_kinds(generated: &[String], current: &[&str]) -> (Vec<String>, Vec<String>) {
    let gen_set: BTreeSet<_> = generated.iter().cloned().collect();
    let cur_set: BTreeSet<_> = current.iter().map(|s| s.to_string()).collect();

    let only_in_generated: Vec<_> = gen_set.difference(&cur_set).cloned().collect();
    let only_in_current: Vec<_> = cur_set.difference(&gen_set).cloned().collect();

    (only_in_generated, only_in_current)
}

const BEGIN_SENTINEL: &str = "    // --- BEGIN GENERATED COMPOSITE KINDS ---";
const END_SENTINEL: &str = "    // --- END GENERATED COMPOSITE KINDS ---";

/// Rewrite the composite-kinds section of `generated.rs` in place.
///
/// Replaces everything between the sentinel comments with the
/// sorted composite-kind variants derived from `sail.ungram`.
///
/// Returns the full file text after replacement.
pub fn write_syntax_kinds(template: &str, ast: &AstSrc) -> String {
    let begin =
        template.find(BEGIN_SENTINEL).expect("missing BEGIN GENERATED COMPOSITE KINDS sentinel");
    let end = template.find(END_SENTINEL).expect("missing END GENERATED COMPOSITE KINDS sentinel");

    let before = &template[..begin + BEGIN_SENTINEL.len()];
    let after = &template[end..];

    // Group generated kinds by category for readability
    let kinds = generate_composite_kinds(ast);

    // Categorize each kind (mirrors the existing hand-written grouping)
    let mut top_level = Vec::new();
    let mut expressions = Vec::new();
    let mut patterns = Vec::new();
    let mut types = Vec::new();
    let mut sub_structures = Vec::new();

    for kind in &kinds {
        if kind.ends_with("_DEF")
            || kind.ends_with("_SPEC")
            || kind == "SOURCE_FILE"
            || kind == "DEFINITION"
        {
            top_level.push(kind.as_str());
        } else if kind.ends_with("_EXPR") {
            expressions.push(kind.as_str());
        } else if kind.ends_with("_PAT") {
            patterns.push(kind.as_str());
        } else if kind.starts_with("TYPE_") {
            types.push(kind.as_str());
        } else {
            sub_structures.push(kind.as_str());
        }
    }

    let mut generated = String::new();
    generated.push('\n');
    if !top_level.is_empty() {
        generated.push_str("\n    // Top-level\n");
        for k in &top_level {
            generated.push_str(&format!("    {k},\n"));
        }
    }
    if !expressions.is_empty() {
        generated.push_str("\n    // Expressions\n");
        for k in &expressions {
            generated.push_str(&format!("    {k},\n"));
        }
    }
    if !patterns.is_empty() {
        generated.push_str("\n    // Patterns\n");
        for k in &patterns {
            generated.push_str(&format!("    {k},\n"));
        }
    }
    if !types.is_empty() {
        generated.push_str("\n    // Type expressions\n");
        for k in &types {
            generated.push_str(&format!("    {k},\n"));
        }
    }
    if !sub_structures.is_empty() {
        generated.push_str("\n    // Sub-structures\n");
        for k in &sub_structures {
            generated.push_str(&format!("    {k},\n"));
        }
    }
    generated.push('\n');

    format!("{before}{generated}{after}")
}

/// Enum types from ungram that don't have their own SyntaxKind.
/// These are pure alternations and get generated as Rust enums.
const ENUM_TYPES: &[&str] = &["Expr", "Pat", "Type"];

/// Struct nodes from ungram that don't have a SyntaxKind.
/// These are sub-structure helpers that exist only for organization.
const SKIP_STRUCT_NODES: &[&str] = &["FieldPat", "TypeParam"];

/// Nodes where codegen should NOT generate field accessors.
/// These have hand-written accessors in HAND_WRITTEN_ACCESSORS
/// because their CST structure doesn't map cleanly to ungram fields.
const SKIP_ACCESSOR_NODES: &[&str] = &[
    "SourceFile", // definitions() needs raw SyntaxNode iterator
    "BinExpr",    // lhs/rhs need positional access + op() token accessor
    "CallExpr",   // callee() needs raw child access
    "IfExpr",     // condition/then/else need special handling
    "BlockExpr",  // items() uses BlockItem children directly
    "MatchExpr",  // arms() uses MatchArm children directly
];

/// Check if a type name refers to an enum type (no SyntaxKind).
fn is_enum_type(name: &str) -> bool {
    ENUM_TYPES.contains(&name)
}

/// Check if a type name refers to a concrete AST node (has SyntaxKind).
fn is_concrete_node(name: &str) -> bool {
    !is_enum_type(name) && !SKIP_STRUCT_NODES.contains(&name)
}

/// Generate the complete `nodes.rs` file from `AstSrc`.
///
/// Produces:
/// - Struct + AstNode impl + typed accessors for each concrete node
/// - Enum + AstNode impl for each enum type (Expr, Pat, Type)
pub fn generate_nodes(ast: &AstSrc) -> String {
    let mut buf = String::new();

    // File header
    buf.push_str("//! Generated from `sail.ungram`, do not edit by hand.\n");
    buf.push_str("//!\n");
    buf.push_str("//! Run `cargo test -p syntax -- codegen` to validate.\n");
    buf.push('\n');
    buf.push_str("#![allow(dead_code)]\n");
    buf.push('\n');
    buf.push_str("use parser::SyntaxKind as SK;\n");
    buf.push_str("use crate::syntax_node::{SyntaxNode, SyntaxToken};\n");
    buf.push('\n');
    buf.push_str("use super::super::{support, AstNode};\n");

    // Determine which traits are actually used by generated impls
    let mut used_traits: BTreeSet<&str> = BTreeSet::new();
    for node in &ast.nodes {
        if UNGRAM_ONLY_NODES.contains(&node.name.as_str())
            || SKIP_STRUCT_NODES.contains(&node.name.as_str())
        {
            continue;
        }
        for trait_name in traits_for_node(&node.fields) {
            used_traits.insert(trait_name);
        }
    }
    if !used_traits.is_empty() {
        let trait_list: Vec<&str> = used_traits.into_iter().collect();
        buf.push_str(&format!("use super::super::traits::{{{}}};\n", trait_list.join(", ")));
    }
    buf.push('\n');

    // Build lookup: node name -> fields (for accessor generation)
    let node_map: std::collections::HashMap<&str, &[Field]> =
        ast.nodes.iter().map(|n| (n.name.as_str(), n.fields.as_slice())).collect();

    // Build lookup: enum name -> variants
    let enum_map: std::collections::HashMap<&str, &[String]> =
        ast.enums.iter().map(|e| (e.name.as_str(), e.variants.as_slice())).collect();

    // Collect all concrete struct nodes (have SyntaxKind)
    let kinds = generate_composite_kinds(ast);
    let kind_to_pascal: std::collections::HashMap<String, String> = ast
        .nodes
        .iter()
        .map(|n| (to_upper_snake_case(&n.name), n.name.clone()))
        .chain(ast.enums.iter().map(|e| (to_upper_snake_case(&e.name), e.name.clone())))
        .collect();

    // Categorize for section headers
    let mut top_level = Vec::new();
    let mut expressions = Vec::new();
    let mut patterns = Vec::new();
    let mut types = Vec::new();
    let mut sub_structures = Vec::new();

    for kind in &kinds {
        let pascal = match kind_to_pascal.get(kind) {
            Some(p) => p.clone(),
            None => continue,
        };
        let entry = (pascal, kind.clone());
        if kind.ends_with("_DEF")
            || kind.ends_with("_SPEC")
            || kind == "SOURCE_FILE"
            || kind == "DEFINITION"
        {
            top_level.push(entry);
        } else if kind.ends_with("_EXPR") {
            expressions.push(entry);
        } else if kind.ends_with("_PAT") {
            patterns.push(entry);
        } else if kind.starts_with("TYPE_") {
            types.push(entry);
        } else {
            sub_structures.push(entry);
        }
    }

    // Generate struct nodes with accessors
    fn emit_struct_section(
        buf: &mut String,
        header: &str,
        entries: &[(String, String)],
        node_map: &std::collections::HashMap<&str, &[Field]>,
        enum_map: &std::collections::HashMap<&str, &[String]>,
    ) {
        if entries.is_empty() {
            return;
        }
        buf.push_str(&format!("// {header}\n\n"));
        for (pascal, kind) in entries {
            emit_struct_node(buf, pascal, kind, node_map, enum_map);
        }
    }

    emit_struct_section(&mut buf, "Top-level definitions", &top_level, &node_map, &enum_map);
    emit_struct_section(&mut buf, "Expressions", &expressions, &node_map, &enum_map);
    emit_struct_section(&mut buf, "Patterns", &patterns, &node_map, &enum_map);
    emit_struct_section(&mut buf, "Type expressions", &types, &node_map, &enum_map);
    emit_struct_section(&mut buf, "Sub-structures", &sub_structures, &node_map, &enum_map);

    // Generate enum types (Expr, Pat, Type)
    buf.push_str("// Enum types (virtual alternation nodes)\n\n");
    for &enum_name in ENUM_TYPES {
        if let Some(variants) = enum_map.get(enum_name) {
            emit_enum_node(&mut buf, enum_name, variants);
        }
    }

    // Hand-written accessors for complex cases that codegen can't handle.
    // These override or supplement the generated accessors above.
    buf.push_str(HAND_WRITTEN_ACCESSORS);

    buf
}

/// Hand-written accessor methods that require complex logic beyond
/// what the codegen generates from field metadata alone.
///
/// These are appended verbatim to the generated nodes.rs.
/// The generated accessors use typed AST nodes (Name, Expr, etc.),
/// while these hand-written ones often return raw SyntaxNode/SyntaxToken
/// for cases where the CST structure doesn't map cleanly to ungram fields.
const HAND_WRITTEN_ACCESSORS: &str = r#"
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
"#;

/// Emit a single struct node: struct definition + AstNode impl + accessors.
fn emit_struct_node(
    buf: &mut String,
    pascal: &str,
    kind: &str,
    node_map: &std::collections::HashMap<&str, &[Field]>,
    #[allow(unused)] _enum_map: &std::collections::HashMap<&str, &[String]>,
) {
    // Struct definition
    buf.push_str("#[derive(Debug, Clone, PartialEq, Eq, Hash)]\n");
    buf.push_str(&format!("pub struct {pascal} {{\n"));
    buf.push_str("    pub(crate) syntax: SyntaxNode,\n");
    buf.push_str("}\n\n");

    // AstNode impl
    buf.push_str(&format!("impl AstNode for {pascal} {{\n"));
    buf.push_str(&format!("    fn can_cast(kind: SK) -> bool {{ kind == SK::{kind} }}\n"));
    buf.push_str("    fn cast(syntax: SyntaxNode) -> Option<Self> {\n");
    buf.push_str(
        "        if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }\n",
    );
    buf.push_str("    }\n");
    buf.push_str("    fn syntax(&self) -> &SyntaxNode { &self.syntax }\n");
    buf.push_str(&format!("    fn kind() -> SK {{ SK::{kind} }}\n"));
    buf.push_str("}\n\n");

    // Generate typed accessors from fields
    let pascal_ref: &str = pascal;
    let fields = match node_map.get(pascal_ref) {
        Some(f) => *f,
        None => return, // Enum-based SyntaxKind (Definition, BlockItem) — no fields
    };

    // Generate trait impls based on field analysis
    let node_traits = traits_for_node(fields);
    for trait_name in &node_traits {
        buf.push_str(&format!("impl {trait_name} for {pascal} {{}}\n"));
    }
    if !node_traits.is_empty() {
        buf.push('\n');
    }

    // Skip accessor generation for nodes with hand-written accessors
    if SKIP_ACCESSOR_NODES.contains(&pascal) {
        return;
    }

    let node_fields: Vec<_> = fields
        .iter()
        .filter_map(|f| match f {
            Field::Node { name, ty, cardinality } => {
                Some((name.as_str(), ty.as_str(), *cardinality))
            }
            Field::Token(_) => None,
        })
        .collect();

    if node_fields.is_empty() {
        return;
    }

    // Track type positions (for Nth child of same type) and seen method names
    let mut type_positions: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut accessors = Vec::new();
    for &(name, ty, cardinality) in &node_fields {
        // Skip fields whose type is a skipped node
        if SKIP_STRUCT_NODES.contains(&ty) {
            continue;
        }

        let escaped_name = escape_keyword(name);

        // Skip duplicate method names (from ungram repetitions that
        // produce multiple unnamed fields of the same type)
        if !seen_names.insert(escaped_name.clone()) {
            // Still count position for this type
            *type_positions.entry(ty).or_insert(0) += 1;
            continue;
        }

        match cardinality {
            Cardinality::Many => {
                if is_enum_type(ty) {
                    // For Many + enum type, skip — hand-written accessors handle this
                    continue;
                }
                if !is_concrete_node(ty) {
                    continue;
                }
                accessors.push(format!(
                    "    pub fn {escaped_name}(&self) -> Vec<{ty}> {{ support::children(&self.syntax) }}"
                ));
            }
            Cardinality::Optional => {
                let pos = type_positions.entry(ty).or_insert(0);
                let current_pos = *pos;
                *type_positions.get_mut(ty).unwrap() += 1;

                if is_enum_type(ty) {
                    if current_pos == 0 {
                        accessors.push(format!(
                            "    pub fn {escaped_name}(&self) -> Option<{ty}> {{ support::child(&self.syntax) }}"
                        ));
                    } else {
                        accessors.push(format!(
                            "    pub fn {escaped_name}(&self) -> Option<{ty}> {{\n\
                             \x20       let mut iter = self.syntax.children().filter_map({ty}::cast);\n\
                             \x20       iter.nth({current_pos})\n\
                             \x20   }}"
                        ));
                    }
                } else if is_concrete_node(ty) {
                    if current_pos == 0 {
                        accessors.push(format!(
                            "    pub fn {escaped_name}(&self) -> Option<{ty}> {{ support::child(&self.syntax) }}"
                        ));
                    } else {
                        accessors.push(format!(
                            "    pub fn {escaped_name}(&self) -> Option<{ty}> {{\n\
                             \x20       let items: Vec<{ty}> = support::children(&self.syntax);\n\
                             \x20       items.into_iter().nth({current_pos})\n\
                             \x20   }}"
                        ));
                    }
                }
            }
        }
    }

    if !accessors.is_empty() {
        buf.push_str(&format!("impl {pascal} {{\n"));
        for accessor in &accessors {
            buf.push_str(accessor);
            buf.push('\n');
        }
        buf.push_str("}\n\n");
    }
}

/// Emit an enum AST node (Expr, Pat, Type).
fn emit_enum_node(buf: &mut String, name: &str, variants: &[String]) {
    // Enum definition
    buf.push_str("#[derive(Debug, Clone, PartialEq, Eq, Hash)]\n");
    buf.push_str(&format!("pub enum {name} {{\n"));
    for v in variants {
        buf.push_str(&format!("    {v}({v}),\n"));
    }
    buf.push_str("}\n\n");

    // AstNode impl
    buf.push_str(&format!("impl AstNode for {name} {{\n"));

    // can_cast
    buf.push_str("    fn can_cast(kind: SK) -> bool {\n");
    buf.push_str("        matches!(kind, ");
    let kind_arms: Vec<String> = variants
        .iter()
        .filter(|v| is_concrete_node(v))
        .map(|v| format!("SK::{}", to_upper_snake_case(v)))
        .collect();
    buf.push_str(&kind_arms.join(" | "));
    buf.push_str(")\n");
    buf.push_str("    }\n");

    // cast
    buf.push_str("    fn cast(syntax: SyntaxNode) -> Option<Self> {\n");
    buf.push_str("        match syntax.kind() {\n");
    for v in variants {
        if !is_concrete_node(v) {
            continue;
        }
        let upper = to_upper_snake_case(v);
        buf.push_str(&format!("            SK::{upper} => Some({name}::{v}({v} {{ syntax }})),\n"));
    }
    buf.push_str("            _ => None,\n");
    buf.push_str("        }\n");
    buf.push_str("    }\n");

    // syntax
    buf.push_str("    fn syntax(&self) -> &SyntaxNode {\n");
    buf.push_str("        match self {\n");
    for v in variants {
        buf.push_str(&format!("            {name}::{v}(it) => &it.syntax,\n"));
    }
    buf.push_str("        }\n");
    buf.push_str("    }\n");

    // kind — enums have multiple kinds, so this is a placeholder
    buf.push_str(&format!(
        "    fn kind() -> SK {{ unimplemented!(\"enum {name} has multiple kinds\") }}\n"
    ));

    buf.push_str("}\n\n");
}

/// Write the complete nodes.rs file.
///
/// Unlike the old `write_ast_nodes` which used sentinel-based replacement,
/// this generates the entire file from scratch. Hand-written accessors
/// that need complex logic are appended at the bottom.
pub fn write_ast_nodes(ast: &AstSrc) -> String {
    generate_nodes(ast)
}

/// Token types to generate, with (struct_name, syntax_kind_variant).
///
/// These are the meaningful terminal tokens referenced in sail.ungram.
/// Trivia tokens (WHITESPACE, LINE_COMMENT, BLOCK_COMMENT) and
/// DOC_COMMENT are also included for completeness.
const TOKEN_TYPES: &[(&str, &str)] = &[
    // Identifiers
    ("Ident", "IDENT"),
    ("TyVar", "TY_VAR"),
    // Literals
    ("BinLit", "BIN_LIT"),
    ("HexLit", "HEX_LIT"),
    ("NumLit", "NUM_LIT"),
    ("RealLit", "REAL_LIT"),
    ("StringLit", "STRING_LIT"),
    ("MultilineStringLit", "MULTILINE_STRING_LIT"),
    // Comments
    ("LineComment", "LINE_COMMENT"),
    ("BlockComment", "BLOCK_COMMENT"),
    ("DocComment", "DOC_COMMENT"),
    // Whitespace
    ("Whitespace", "WHITESPACE"),
];

/// Generate the complete `tokens.rs` file.
///
/// Produces a struct + Display + AstToken impl for each token type.
pub fn generate_tokens() -> String {
    let mut buf = String::new();

    // File header
    buf.push_str("//! Generated token wrappers from `sail.ungram`.\n");
    buf.push_str("//!\n");
    buf.push_str("//! Each token type wraps a `SyntaxToken` and implements `AstToken`.\n");
    buf.push('\n');
    buf.push_str("use parser::SyntaxKind;\n");
    buf.push('\n');
    buf.push_str("use crate::syntax_node::SyntaxToken;\n");
    buf.push_str("use crate::ast::AstToken;\n");
    buf.push('\n');

    // Macro definition (mirrors the existing hand-written one)
    buf.push_str("macro_rules! ast_token {\n");
    buf.push_str("    ($name:ident, $kind:ident) => {\n");
    buf.push_str("        #[derive(Debug, Clone, PartialEq, Eq, Hash)]\n");
    buf.push_str("        pub struct $name {\n");
    buf.push_str("            pub(crate) syntax: SyntaxToken,\n");
    buf.push_str("        }\n");
    buf.push('\n');
    buf.push_str("        impl std::fmt::Display for $name {\n");
    buf.push_str(
        "            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n",
    );
    buf.push_str("                std::fmt::Display::fmt(&self.syntax, f)\n");
    buf.push_str("            }\n");
    buf.push_str("        }\n");
    buf.push('\n');
    buf.push_str("        impl AstToken for $name {\n");
    buf.push_str("            fn can_cast(kind: SyntaxKind) -> bool {\n");
    buf.push_str("                kind == SyntaxKind::$kind\n");
    buf.push_str("            }\n");
    buf.push_str("            fn cast(syntax: SyntaxToken) -> Option<Self> {\n");
    buf.push_str("                if Self::can_cast(syntax.kind()) {\n");
    buf.push_str("                    Some(Self { syntax })\n");
    buf.push_str("                } else {\n");
    buf.push_str("                    None\n");
    buf.push_str("                }\n");
    buf.push_str("            }\n");
    buf.push_str("            fn syntax(&self) -> &SyntaxToken {\n");
    buf.push_str("                &self.syntax\n");
    buf.push_str("            }\n");
    buf.push_str("        }\n");
    buf.push_str("    };\n");
    buf.push_str("}\n");

    // Group tokens by category
    let categories: &[(&str, &[usize])] = &[
        ("Identifiers", &[0, 1]),
        ("Literals", &[2, 3, 4, 5, 6, 7]),
        ("Comments", &[8, 9, 10]),
        ("Whitespace", &[11]),
    ];

    for (category, indices) in categories {
        buf.push_str(&format!("\n// {category}\n"));
        for &idx in *indices {
            let (name, kind) = TOKEN_TYPES[idx];
            buf.push_str(&format!("ast_token!({name}, {kind});\n"));
        }
    }

    buf
}

/// Write the complete tokens.rs file.
pub fn write_tokens() -> String {
    generate_tokens()
}

/// Keyword-to-macro-arm mappings: (macro_pattern, SyntaxKind variant).
///
/// These define the `T![keyword]` arms of the T_! macro. Each entry
/// maps a bare keyword token to its SyntaxKind variant.
const KEYWORD_MACRO_ARMS: &[(&str, &str)] = &[
    ("and", "KW_AND"),
    ("as", "KW_AS"),
    ("assert", "KW_ASSERT"),
    ("backwards", "KW_BACKWARDS"),
    ("bitfield", "KW_BITFIELD"),
    ("bool", "KW_BOOL"),
    ("by", "KW_BY"),
    ("cast", "KW_CAST"),
    ("catch", "KW_CATCH"),
    ("clause", "KW_CLAUSE"),
    ("constraint", "KW_CONSTRAINT"),
    ("dec", "KW_DEC"),
    ("default", "KW_DEFAULT"),
    ("do", "KW_DO"),
    ("downto", "KW_DOWNTO"),
    ("effect", "KW_EFFECT"),
    ("else", "KW_ELSE"),
    ("end", "KW_END"),
    ("enum", "KW_ENUM"),
    ("exit", "KW_EXIT"),
    ("false", "KW_FALSE"),
    ("forall", "KW_FORALL"),
    ("foreach", "KW_FOREACH"),
    ("forwards", "KW_FORWARDS"),
    ("from", "KW_FROM"),
    ("function", "KW_FUNCTION"),
    ("if", "KW_IF"),
    ("in", "KW_IN"),
    ("inc", "KW_INC"),
    ("infix", "KW_INFIX"),
    ("infixl", "KW_INFIXL"),
    ("infixr", "KW_INFIXR"),
    ("int", "KW_INT"),
    ("let", "KW_LET"),
    ("mapping", "KW_MAPPING"),
    ("match", "KW_MATCH"),
    ("mutual", "KW_MUTUAL"),
    ("newtype", "KW_NEWTYPE"),
    ("order", "KW_ORDER"),
    ("outcome", "KW_OUTCOME"),
    ("overload", "KW_OVERLOAD"),
    ("private", "KW_PRIVATE"),
    ("pure", "KW_PURE"),
    ("ref", "KW_REF"),
    ("register", "KW_REGISTER"),
    ("repeat", "KW_REPEAT"),
    ("return", "KW_RETURN"),
    ("scattered", "KW_SCATTERED"),
    ("sizeof", "KW_SIZEOF"),
    ("struct", "KW_STRUCT"),
    ("switch", "KW_SWITCH"),
    ("then", "KW_THEN"),
    ("throw", "KW_THROW"),
    ("to", "KW_TO"),
    ("true", "KW_TRUE"),
    ("try", "KW_TRY"),
    ("type", "KW_TYPE"),
    ("undefined", "KW_UNDEFINED"),
    ("union", "KW_UNION"),
    ("until", "KW_UNTIL"),
    ("val", "KW_VAL"),
    ("var", "KW_VAR"),
    ("when", "KW_WHEN"),
    ("while", "KW_WHILE"),
    ("with", "KW_WITH"),
];

/// Punctuation-to-macro-arm mappings: (macro_pattern, SyntaxKind variant).
///
/// These define the `T![+]`, `T!['(']` etc. arms of the T_! macro.
/// Patterns with `'` quoting are used for brackets to avoid ambiguity.
const PUNCTUATION_MACRO_ARMS: &[(&str, &str)] = &[
    ("'('", "L_PAREN"),
    ("')'", "R_PAREN"),
    ("'['", "L_BRACK"),
    ("']'", "R_BRACK"),
    ("'{'", "L_CURLY"),
    ("'}'", "R_CURLY"),
    ("<", "L_ANGLE"),
    (">", "R_ANGLE"),
    ("->", "R_ARROW"),
    ("<-", "L_ARROW"),
    ("=>", "FAT_R_ARROW"),
    ("<->", "DOUBLE_ARROW"),
    (":=", "COLON_EQ"),
    (",", "COMMA"),
    (":", "COLON"),
    (";", "SEMICOLON"),
    (".", "DOT"),
    ("^", "CARET"),
    ("@", "AT"),
    ("<=", "LE"),
    (">=", "GE"),
    ("%", "PERCENT"),
    ("*", "STAR"),
    ("/", "SLASH"),
    ("=", "EQ"),
    ("==", "EQ_EQ"),
    ("!=", "NEQ"),
    ("&", "AMP"),
    ("|", "PIPE"),
    ("::", "SCOPE"),
    ("+", "PLUS"),
    ("-", "MINUS"),
    ("_", "UNDERSCORE"),
];

/// Generate the T_! macro match arms from the keyword/punctuation tables.
///
/// Returns a string containing the complete `macro_rules! T_` definition
/// with arms generated from `KEYWORD_MACRO_ARMS` and `PUNCTUATION_MACRO_ARMS`.
pub fn generate_t_macro() -> String {
    let mut buf = String::new();
    buf.push_str("/// Token shorthand macro.\n");
    buf.push_str("///\n");
    buf.push_str("/// Usage: `T![function]`, `T![+]`, `T!['(']`, etc.\n");
    buf.push_str("#[macro_export]\n");
    buf.push_str("macro_rules! T_ {\n");

    // Keywords
    buf.push_str("    // Keywords\n");
    for &(pattern, kind) in KEYWORD_MACRO_ARMS {
        buf.push_str(&format!("    [{pattern}] => {{ $crate::SyntaxKind::{kind} }};\n"));
    }

    // Punctuation
    buf.push_str("\n    // Punctuation\n");
    for &(pattern, kind) in PUNCTUATION_MACRO_ARMS {
        buf.push_str(&format!("    [{pattern}] => {{ $crate::SyntaxKind::{kind} }};\n"));
    }

    buf.push_str("}\n");
    buf
}

/// Trait-to-field mapping for auto-generated trait impls.
///
/// Each entry is (trait_name, field_name, field_type) — if a node
/// has a field matching (field_name, field_type), a blanket `impl
/// TraitName for NodeType {}` is generated in nodes.rs.
const TRAIT_FIELD_MAP: &[(&str, &str, &str)] = &[
    ("HasName", "name", "Name"),
    ("HasAttrs", "attributes", "Attribute"),
    ("HasVisibility", "visibility", "Visibility"),
];

/// Detect which traits a node should implement based on its fields.
fn traits_for_node(fields: &[Field]) -> Vec<&'static str> {
    let mut traits = Vec::new();
    for &(trait_name, field_name, field_ty) in TRAIT_FIELD_MAP {
        for f in fields {
            match f {
                Field::Node { name, ty, .. } if name == field_name && ty == field_ty => {
                    traits.push(trait_name);
                    break;
                }
                _ => {}
            }
        }
    }
    traits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_snake_case() {
        assert_eq!(to_upper_snake_case("CallableDef"), "CALLABLE_DEF");
        assert_eq!(to_upper_snake_case("SourceFile"), "SOURCE_FILE");
        assert_eq!(to_upper_snake_case("BinExpr"), "BIN_EXPR");
        assert_eq!(to_upper_snake_case("IfExpr"), "IF_EXPR");
        assert_eq!(to_upper_snake_case("TypeVar"), "TYPE_VAR");
    }

    #[test]
    fn lower_sail_ungram() {
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);

        // Should have both nodes (structs) and enums
        assert!(!ast.nodes.is_empty(), "should have struct nodes");
        assert!(!ast.enums.is_empty(), "should have enum nodes");

        // Expr, Pat, Type should be enums (alternations)
        let enum_names: Vec<_> = ast.enums.iter().map(|e| e.name.as_str()).collect();
        assert!(enum_names.contains(&"Expr"), "Expr should be an enum");
        assert!(enum_names.contains(&"Pat"), "Pat should be an enum");
        assert!(enum_names.contains(&"Type"), "Type should be an enum");
        assert!(enum_names.contains(&"Definition"), "Definition should be an enum");

        // CallableDef, IfExpr should be structs (sequences)
        let node_names: Vec<_> = ast.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(node_names.contains(&"CallableDef"), "CallableDef should be a struct");
        assert!(node_names.contains(&"IfExpr"), "IfExpr should be a struct");
        assert!(node_names.contains(&"MatchArm"), "MatchArm should be a struct");
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn generated_kinds_match_current() {
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);
        let generated = generate_composite_kinds(&ast);

        // Current hand-written composite SyntaxKind variants
        // (from parser/src/syntax_kind.rs lines 180-273)
        let current = vec![
            "SOURCE_FILE",
            "DEFINITION",
            "CALLABLE_DEF",
            "CALLABLE_SPEC",
            "TYPE_ALIAS_DEF",
            "NAMED_DEF",
            "SCATTERED_DEF",
            "SCATTERED_CLAUSE_DEF",
            "DEFAULT_DEF",
            "FIXITY_DEF",
            "INSTANTIATION_DEF",
            "DIRECTIVE_DEF",
            "END_DEF",
            "CONSTRAINT_DEF",
            "TERMINATION_MEASURE_DEF",
            "OUTCOME_DEF",
            "LITERAL_EXPR",
            "IDENT_EXPR",
            "TYVAR_EXPR",
            "REF_EXPR",
            "BIN_EXPR",
            "PREFIX_EXPR",
            "CALL_EXPR",
            "FIELD_ACCESS_EXPR",
            "INDEX_EXPR",
            "SUBRANGE_EXPR",
            "VECTOR_UPDATE_EXPR",
            "IF_EXPR",
            "MATCH_EXPR",
            "TRY_EXPR",
            "BLOCK_EXPR",
            "LET_EXPR",
            "VAR_EXPR",
            "RETURN_EXPR",
            "THROW_EXPR",
            "EXIT_EXPR",
            "ASSERT_EXPR",
            "ASSIGN_EXPR",
            "CAST_EXPR",
            "FOREACH_EXPR",
            "WHILE_EXPR",
            "REPEAT_EXPR",
            "TUPLE_EXPR",
            "LIST_EXPR",
            "VECTOR_EXPR",
            "STRUCT_EXPR",
            "UPDATE_EXPR",
            "SIZEOF_EXPR",
            "CONSTRAINT_EXPR",
            "CONFIG_EXPR",
            "WILD_PAT",
            "LITERAL_PAT",
            "IDENT_PAT",
            "TYVAR_PAT",
            "TYPED_PAT",
            "TUPLE_PAT",
            "LIST_PAT",
            "VECTOR_PAT",
            "APP_PAT",
            "STRUCT_PAT",
            "BIN_PAT",
            "INDEX_PAT",
            "RANGE_INDEX_PAT",
            "AS_PAT",
            "TYPE_NAMED",
            "TYPE_VAR",
            "TYPE_APP",
            "TYPE_TUPLE",
            "TYPE_ARROW",
            "TYPE_FORALL",
            "TYPE_EXISTENTIAL",
            "TYPE_EFFECT",
            "MATCH_ARM",
            "BLOCK_ITEM",
            "FIELD_INIT",
            "PARAM_LIST",
            "ARG_LIST",
            "TYPE_PARAM_LIST",
            "QUANTIFIER",
            "ATTRIBUTE",
            "NAME",
            "BODY",
            "VISIBILITY",
        ];

        let (only_gen, only_cur) = diff_composite_kinds(&generated, &current);

        // Report differences for debugging (non-fatal for now)
        if !only_gen.is_empty() {
            eprintln!("In generated but not in current SyntaxKind: {:?}", only_gen);
        }
        if !only_cur.is_empty() {
            eprintln!("In current SyntaxKind but not generated: {:?}", only_cur);
        }

        // The generated set should cover all current composites
        // (missing means sail.ungram is incomplete)
        assert!(only_cur.is_empty(), "sail.ungram is missing rules for: {:?}", only_cur);
    }

    #[test]
    fn node_names_match_syntax_kind() {
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);
        let names = collect_node_names(&ast);

        // Verify key composite SyntaxKind names are present
        let expected_upper: Vec<&str> = vec![
            "SOURCE_FILE",
            "CALLABLE_DEF",
            "CALLABLE_SPEC",
            "BIN_EXPR",
            "IF_EXPR",
            "MATCH_EXPR",
            "WILD_PAT",
            "IDENT_PAT",
            "TYPE_NAMED",
            "TYPE_ARROW",
            "MATCH_ARM",
            "PARAM_LIST",
        ];
        for upper in expected_upper {
            // Convert UPPER_SNAKE to PascalCase to find in names
            let pascal = upper
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().to_string() + &c.as_str().to_lowercase())
                        .unwrap_or_default()
                })
                .collect::<String>();
            assert!(
                names.contains(&pascal),
                "missing node for SyntaxKind::{upper} (expected PascalCase: {pascal})"
            );
        }
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn codegen_syntax_kinds_roundtrip() {
        // Verify that write_syntax_kinds produces
        // output identical to the current generated.rs.
        //
        // If this test fails, sail.ungram and generated.rs are out
        // of sync. Run with UPDATE_EXPECT=1 to regenerate.
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);

        let current = include_str!("../../parser/src/syntax_kind/generated.rs");
        let regenerated = write_syntax_kinds(current, &ast);

        if regenerated != current {
            // Show the diff for debugging
            let current_lines: Vec<_> = current.lines().collect();
            let regen_lines: Vec<_> = regenerated.lines().collect();
            let mut diffs = Vec::new();
            for (i, (a, b)) in current_lines.iter().zip(regen_lines.iter()).enumerate() {
                if a != b {
                    diffs.push(format!("  line {}: -{a}\n  line {}:  +{b}", i + 1, i + 1));
                }
            }
            if current_lines.len() != regen_lines.len() {
                diffs.push(format!(
                    "  line count: current={}, regenerated={}",
                    current_lines.len(),
                    regen_lines.len()
                ));
            }

            if std::env::var("UPDATE_EXPECT").is_ok() {
                let path =
                    concat!(env!("CARGO_MANIFEST_DIR"), "/../parser/src/syntax_kind/generated.rs");
                std::fs::write(path, &regenerated).expect("failed to write generated.rs");
                eprintln!("Updated generated.rs");
            } else {
                panic!(
                    "generated.rs is out of date with sail.ungram.\n\
                     Diffs (first 20):\n{}\n\n\
                     Run with UPDATE_EXPECT=1 to regenerate.",
                    diffs.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
                );
            }
        }
    }

    #[test]
    fn parser_composite_kinds_in_generated() {
        // Verify that every composite SyntaxKind used
        // by parsing.rs exists in the generated file (and thus
        // in sail.ungram).
        //
        // parser must only use kinds the grammar defines.
        let parser_src = include_str!("parsing.rs");
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);
        let generated = generate_composite_kinds(&ast);
        let generated_set: std::collections::HashSet<_> = generated.iter().collect();

        // Token-level kinds that are NOT composite nodes (hand-maintained).
        // These appear in parsing/mod.rs but are not generated from sail.ungram.
        let token_prefixes = [
            "KW_",
            "IDENT",
            "TY_VAR",
            "BIN_LIT",
            "HEX_LIT",
            "NUM_LIT",
            "REAL_LIT",
            "STRING_LIT",
            "MULTILINE_STRING",
            "WHITESPACE",
            "LINE_COMMENT",
            "BLOCK_COMMENT",
            "DOC_COMMENT",
            "ERROR",
            "EOF",
            "TOMBSTONE",
        ];
        let token_exact = [
            "L_PAREN",
            "R_PAREN",
            "L_CURLY",
            "R_CURLY",
            "L_BRACK",
            "R_BRACK",
            "COMMA",
            "SEMI",
            "COLON",
            "DOT",
            "EQ",
            "LT",
            "GT",
            "AMP",
            "PIPE",
            "CARET",
            "TILDE",
            "STAR",
            "PLUS",
            "MINUS",
            "SLASH",
            "PERCENT",
            "BANG",
            "AT",
            "HASH",
            "DOLLAR",
            "QUESTION",
            "ARROW",
            "R_ARROW",
            "L_ARROW",
            "FAT_R_ARROW",
            "DOUBLE_ARROW",
            "COLON_EQ",
            "EQ_EQ",
            "NOT_EQ",
            "NEQ",
            "LE",
            "GE",
            "AND_AND",
            "PIPE_PIPE",
            "COLON_COLON",
            "DOT_DOT",
            "SCOPE",
            "UNDERSCORE",
            "UNIT",
            "STRUCTURED_DIRECTIVE_START",
            "DIRECTIVE",
            "L_CURLY_BAR",
            "R_CURLY_BAR",
            "L_BRACKET_BAR",
            "R_BRACKET_BAR",
            "L_ANGLE",
            "R_ANGLE",
            "SEMICOLON",
        ];

        let is_token_kind = |name: &str| -> bool {
            token_exact.contains(&name) || token_prefixes.iter().any(|p| name.starts_with(p))
        };

        // Extract all `SK::IDENT_LIKE` references from parser source
        let mut missing = Vec::new();
        for line in parser_src.lines() {
            for segment in line.split("SK::") {
                // Take the identifier part
                let kind_name: String =
                    segment.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
                if kind_name.is_empty() {
                    continue;
                }
                if is_token_kind(&kind_name) {
                    continue;
                }
                if !generated_set.contains(&kind_name) {
                    missing.push(kind_name);
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "parsing/mod.rs uses composite SyntaxKinds not in sail.ungram: {:?}",
            missing
        );
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn codegen_ast_nodes_roundtrip() {
        // Verify that generate_nodes() produces output identical to
        // the current nodes.rs. If this fails, either sail.ungram
        // changed or the codegen logic changed.
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();
        let ast = lower(&grammar);

        let current = include_str!("ast/generated/nodes.rs");
        let regenerated = write_ast_nodes(&ast);

        if regenerated != current {
            let current_lines: Vec<_> = current.lines().collect();
            let regen_lines: Vec<_> = regenerated.lines().collect();
            let mut diffs = Vec::new();
            for (i, (a, b)) in current_lines.iter().zip(regen_lines.iter()).enumerate() {
                if a != b {
                    diffs.push(format!("  line {}: -{a}\n  line {}:  +{b}", i + 1, i + 1));
                }
            }
            if current_lines.len() != regen_lines.len() {
                diffs.push(format!(
                    "  line count: current={}, regenerated={}",
                    current_lines.len(),
                    regen_lines.len()
                ));
            }

            if std::env::var("UPDATE_EXPECT").is_ok() {
                let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ast/generated/nodes.rs");
                std::fs::write(path, &regenerated).expect("failed to write nodes.rs");
                eprintln!("Updated nodes.rs");
            } else {
                panic!(
                    "nodes.rs is out of date with sail.ungram.\n\
                     Diffs (first 20):\n{}\n\n\
                     Run with UPDATE_EXPECT=1 to regenerate.",
                    diffs.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
                );
            }
        }
    }

    #[test]
    fn codegen_t_macro_roundtrip() {
        // Verify that generate_t_macro() produces output matching the
        // current T_! macro in generated.rs.
        let generated = generate_t_macro();

        // Extract the current T_! macro from generated.rs
        let current_file = include_str!("../../parser/src/syntax_kind/generated.rs");
        let macro_start = current_file
            .find("/// Token shorthand macro.")
            .expect("T_! macro not found in generated.rs");
        let current_macro = &current_file[macro_start..];

        // The generated macro text should be a prefix of the remainder
        // (generated.rs may have more content after the macro).
        assert!(
            current_macro.starts_with(&generated),
            "T_! macro is out of date with codegen.\n\
             Generated length: {}, current match length: {}\n\
             First divergence at byte: {}",
            generated.len(),
            current_macro.len(),
            generated
                .bytes()
                .zip(current_macro.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(generated.len())
        );
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn codegen_tokens_roundtrip() {
        // Verify that generate_tokens() produces output
        // identical to the current tokens.rs.
        let current = include_str!("ast/generated/tokens.rs");
        let regenerated = write_tokens();

        if regenerated != current {
            let current_lines: Vec<_> = current.lines().collect();
            let regen_lines: Vec<_> = regenerated.lines().collect();
            let mut diffs = Vec::new();
            for (i, (a, b)) in current_lines.iter().zip(regen_lines.iter()).enumerate() {
                if a != b {
                    diffs.push(format!("  line {}: -{a}\n  line {}:  +{b}", i + 1, i + 1));
                }
            }
            if current_lines.len() != regen_lines.len() {
                diffs.push(format!(
                    "  line count: current={}, regenerated={}",
                    current_lines.len(),
                    regen_lines.len()
                ));
            }

            if std::env::var("UPDATE_EXPECT").is_ok() {
                let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ast/generated/tokens.rs");
                std::fs::write(path, &regenerated).expect("failed to write tokens.rs");
                eprintln!("Updated tokens.rs");
            } else {
                panic!(
                    "tokens.rs is out of date.\n\
                     Diffs (first 20):\n{}\n\n\
                     Run with UPDATE_EXPECT=1 to regenerate.",
                    diffs.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
                );
            }
        }
    }
}
