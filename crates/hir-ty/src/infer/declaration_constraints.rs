//! Declaration-site type constraints, shared with the expression solver.
//! A width restriction belongs to a type declaration, not to its spelling.
use super::*;
use base_db::FileText;
use hir_def::source_order::{self, bindings, Declaration};
use syntax::{SyntaxKind as SK, SyntaxNode};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Point {
    Ordered(Declaration),
    Local(usize),
}

#[derive(Clone, PartialEq, Eq)]
struct Definition {
    is_alias: bool,
    result_kind: Kind,
    category: SK,
    undefined_policy: Option<String>,
    params: Vec<String>,
    kinds: HashMap<String, Kind>,
    constraint: Option<ConstraintExpr>,
    bodies: Vec<Ty>,
}

fn is_definition(node: &SyntaxNode) -> bool {
    node.kind() == SK::TYPE_ALIAS_DEF
        || (node.kind() == SK::NAMED_DEF
            && [SK::KW_STRUCT, SK::KW_UNION, SK::KW_ENUM, SK::KW_NEWTYPE, SK::KW_BITFIELD]
                .into_iter()
                .any(|kind| node.children_with_tokens().any(|t| t.kind() == kind)))
}

fn is_bitfield(node: &SyntaxNode) -> bool {
    node.kind() == SK::NAMED_DEF && node.children_with_tokens().any(|t| t.kind() == SK::KW_BITFIELD)
}

fn definition(node: SyntaxNode, strict: bool) -> Option<Definition> {
    if !is_definition(&node) {
        return None;
    }
    let text = node.text().to_string();
    let start = usize::from(node.text_range().start());
    let separator =
        node.children_with_tokens().filter_map(|t| t.into_token()).find(|t| t.kind() == SK::EQ)?;
    let eq = usize::from(separator.text_range().start()) - start;
    let header: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| {
            !token.kind().is_trivia() && token.text_range().end() <= separator.text_range().start()
        })
        .collect();
    let result_kind = header
        .last()
        .and_then(|last| {
            let previous = header.get(header.len().checked_sub(2)?)?;
            matches!(previous.kind(), SK::COLON | SK::R_ARROW)
                .then(|| Kind::from_name(last.text()))
                .flatten()
        })
        .unwrap_or(Kind::Type);
    let parameters =
        node.children().find(|n| matches!(n.kind(), SK::PARAM_LIST | SK::TYPE_PARAM_LIST));
    let mut params = Vec::new();
    let mut explicit_naturals = Vec::new();
    let mut kinds = kinds::declared(&node);
    if let Some(parameters) = &parameters {
        let tokens: Vec<_> = parameters
            .descendants_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        let mut group = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            if token.kind() == SK::TY_VAR {
                let name = token.text().to_owned();
                params.push(name.clone());
                group.push(name);
            } else if token.kind() == SK::COLON {
                if let Some(kind) = tokens.get(i + 1).and_then(|t| Kind::from_name(t.text())) {
                    if tokens[i + 1].text() == "Nat" {
                        explicit_naturals.extend(group.iter().cloned());
                    }
                    for name in group.drain(..) {
                        kinds.insert(name, kind);
                    }
                }
            } else if token.kind() == SK::COMMA {
                group.clear();
            }
        }
    }
    let constraint = parameters.as_ref().and_then(|parameters| {
        let text = node
            .descendants_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| {
                !t.kind().is_trivia()
                    && t.text_range().start() >= parameters.text_range().end()
                    && t.text_range().end() <= separator.text_range().start()
            })
            .map(|t| t.text().to_owned())
            .collect::<Vec<_>>()
            .join(" ");
        text.split_once(',')
            .map(|(_, c)| parse_constraint_text(c.split("->").next().unwrap_or(c).trim()))
    });
    let bodies = if node.kind() == SK::TYPE_ALIAS_DEF {
        vec![type_from_type_text(&text[eq + 1..])]
    } else if is_bitfield(&node) {
        node.children()
            .find(|n| n.kind().is_type_node())
            .map(|ty| vec![type_from_cst_node(&ty)])
            .unwrap_or_default()
    } else {
        hir_def::item_tree::extract_field_decls(&node)
            .into_iter()
            .filter(|f| !f.type_text.is_empty())
            .map(|f| type_from_type_text(&f.type_text))
            .collect()
    };
    let explicit_kinds = kinds.clone();
    for body in &bodies {
        kinds::infer(body, &mut kinds);
    }
    let bodies = bodies.iter().map(|ty| kinds::normalize(ty, &kinds)).collect();
    let mut facts: Vec<_> =
        constraint.map(|c| kinds::normalize_constraint(&c, &kinds)).into_iter().collect();
    facts.extend(explicit_naturals.iter().map(|p| parse_constraint_text(&format!("{p} >= 0"))));
    if strict {
        // Omitted declaration kinds can be inferred as Nat from a vector
        // length position. Explicit Int parameters stay Int upstream.
        let mut inferred = HashSet::new();
        for body in &bodies {
            inferred_width_parameters(body, &mut inferred);
        }
        for param in &params {
            if !explicit_kinds.contains_key(param) && inferred.contains(param) {
                facts.push(parse_constraint_text(&format!("{param} >= 0")));
            }
        }
    }
    let constraint = (!facts.is_empty()).then_some(ConstraintExpr::And(facts));
    Some(Definition {
        is_alias: node.kind() == SK::TYPE_ALIAS_DEF,
        result_kind,
        category: node
            .children_with_tokens()
            .map(|t| t.kind())
            .find(|k| {
                matches!(
                    k,
                    SK::KW_STRUCT | SK::KW_UNION | SK::KW_ENUM | SK::KW_NEWTYPE | SK::KW_BITFIELD
                )
            })
            .unwrap_or(node.kind()),
        undefined_policy: attribute(&node, "undefined_gen"),
        params,
        kinds,
        constraint,
        bodies,
    })
}

fn inferred_width_parameters(ty: &Ty, out: &mut HashSet<String>) {
    match ty.kind() {
        TyKind::Named(name, args) | TyKind::App { name, args, .. } => {
            if matches!(name.as_str(), "bits" | "bitvector" | "vector") {
                if let Some(arg) = args.first() {
                    let text = match arg {
                        TyArg::Type(t) => t.display_text(),
                        _ => arg.as_value_str().unwrap_or_default(),
                    };
                    fn natural_variable(expr: &NumericExpr) -> Option<&str> {
                        match expr {
                            NumericExpr::Var(name) => Some(name),
                            NumericExpr::Mul(a, b) => match (&**a, &**b) {
                                (NumericExpr::Const(n), value) | (value, NumericExpr::Const(n))
                                    if *n > 0 =>
                                {
                                    natural_variable(value)
                                }
                                _ => None,
                            },
                            _ => None,
                        }
                    }
                    if let Some(expr) = NumericExpr::parse(&text) {
                        if let Some(name) = natural_variable(&expr) {
                            out.insert(name.to_owned());
                        }
                    }
                }
            }
            for arg in args {
                if let TyArg::Type(ty) = arg {
                    inferred_width_parameters(ty, out);
                }
            }
        }
        TyKind::Tuple(types) => types.iter().for_each(|t| inferred_width_parameters(t, out)),
        TyKind::FnPtr(sig) => sig
            .params
            .iter()
            .chain(std::iter::once(&sig.ret))
            .for_each(|t| inferred_width_parameters(t, out)),
        TyKind::Exist { vars, inner, .. } => {
            let mut inner_vars = HashSet::new();
            inferred_width_parameters(inner, &mut inner_vars);
            out.extend(inner_vars.into_iter().filter(|n| !vars.contains(n)));
        }
        _ => {}
    }
}

fn attribute(node: &SyntaxNode, name: &str) -> Option<String> {
    let mut previous = node.prev_sibling();
    while let Some(attr) = previous.filter(|n| n.kind() == SK::ATTRIBUTE) {
        let tokens: Vec<_> =
            attr.descendants_with_tokens().filter_map(|e| e.into_token()).collect();
        if tokens.iter().find(|t| t.kind() == SK::IDENT).is_some_and(|t| t.text() == name) {
            return Some(
                tokens
                    .iter()
                    .find(|t| t.kind() == SK::STRING_LIT)
                    .map(|t| t.text().trim_matches('"').to_owned())
                    .unwrap_or_default(),
            );
        }
        previous = attr.prev_sibling();
    }
    None
}

fn all_known(values: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut result = Some(true);
    for value in values {
        if value == Some(false) {
            return Some(false);
        }
        if value.is_none() {
            result = None;
        }
    }
    result
}

// Project semantic metadata independently of body text and source offsets.
// A body edit in a provider must not invalidate every consumer of its types.
#[salsa::tracked(returns(ref))]
fn file_metadata(db: &dyn salsa::Database, file: FileText) -> Vec<Option<Definition>> {
    syntax::parse_query::semantic_parse_file(db, file)
        .syntax_node()
        .into_iter()
        .flat_map(|root| {
            root.children()
                .map(|node| definition(node, base_db::strict_bitvector(db)))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[salsa::tracked(returns(ref))]
fn declaration_metadata(
    db: &dyn salsa::Database,
    file: FileText,
    index: usize,
) -> Option<Definition> {
    file_metadata(db, file).get(index).cloned().flatten()
}

struct Checker<'a> {
    db: Option<&'a dyn salsa::Database>,
    defs: &'a [SyntaxNode],
    metadata: HashMap<Point, Option<Definition>>,
    table: InferenceTable,
    numbers: HashMap<(Point, String), Option<num_bigint::BigInt>>,
    strict_bitvector: bool,
}

impl Checker<'_> {
    // Sail checks a type's RHS before installing that type. Its recursive
    // restriction is lexical availability, not Rust-style infinite layout:
    // list(S) and other nested self references also fail.
    fn unavailable_type(&self, at: Point, ty: &Ty) -> Option<(String, Point)> {
        match ty.kind() {
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
                if !hir_def::is_builtin_type_name(name) && self.resolve(at, name).is_none() {
                    let origin = match at {
                        Point::Ordered(declaration) => {
                            let db = self.db?;
                            let name = hir_def::Name::new(name);
                            bindings::declared_before(
                                db,
                                declaration,
                                name.clone(),
                                bindings::Namespace::Type,
                            )
                            .or_else(|| {
                                let candidates = bindings::declarations_named_at(
                                    db,
                                    declaration,
                                    name,
                                    bindings::Namespace::Type,
                                );
                                candidates
                                    .iter()
                                    .copied()
                                    .find(|d| *d == declaration)
                                    .or_else(|| candidates.first().copied())
                            })
                            .map(Point::Ordered)
                        }
                        Point::Local(_) => self.defs.iter().enumerate().find_map(|(i, node)| {
                            (is_definition(node)
                                && syntax::definition_name(node).as_deref() == Some(name))
                            .then_some(Point::Local(i))
                        }),
                    };
                    if let Some(origin) = origin {
                        return Some((name.clone(), origin));
                    }
                }
                args.iter().find_map(|arg| match arg {
                    TyArg::Type(ty) => self.unavailable_type(at, ty),
                    _ => None,
                })
            }
            TyKind::Exist { inner, .. } => self.unavailable_type(at, inner),
            TyKind::Tuple(ts) => ts.iter().find_map(|ty| self.unavailable_type(at, ty)),
            TyKind::FnPtr(sig) => sig
                .params
                .iter()
                .chain(std::iter::once(&sig.ret))
                .find_map(|ty| self.unavailable_type(at, ty)),
            TyKind::Bidir { lhs, rhs } => {
                self.unavailable_type(at, lhs).or_else(|| self.unavailable_type(at, rhs))
            }
            _ => None,
        }
    }

    fn related(&self, diagnostic: &mut Diagnostic, origin: Point, message: &str) {
        if let Some(node) = self.node(origin) {
            let range = node
                .children()
                .find(|n| n.kind() == SK::NAME)
                .map_or(node.text_range(), |n| n.text_range());
            if let Point::Ordered(origin) = origin {
                diagnostic.related_in_files.push((
                    origin.file.file_id(self.db.unwrap()),
                    range,
                    message.to_owned(),
                ));
            } else {
                diagnostic.related.push((range, message.to_owned()));
            }
        }
    }
    fn node(&self, point: Point) -> Option<SyntaxNode> {
        match point {
            Point::Ordered(at) => source_order::node(self.db?, at),
            Point::Local(i) => self.defs.get(i).cloned(),
        }
    }

    fn metadata(&mut self, point: Point) -> Option<Definition> {
        if !self.metadata.contains_key(&point) {
            let value = match point {
                Point::Ordered(at) => declaration_metadata(self.db?, at.file, at.index).clone(),
                Point::Local(_) => {
                    self.node(point).and_then(|node| definition(node, self.strict_bitvector))
                }
            };
            self.metadata.insert(point, value);
        }
        self.metadata.get(&point).cloned().flatten()
    }

    fn resolve(&self, at: Point, name: &str) -> Option<Point> {
        match at {
            Point::Ordered(at) => bindings::resolve_before(
                self.db?,
                at,
                hir_def::Name::new(name),
                bindings::Namespace::Type,
            )
            .map(Point::Ordered),
            Point::Local(i) => self.defs[..i].iter().enumerate().rev().find_map(|(i, node)| {
                (is_definition(node) && syntax::definition_name(node).as_deref() == Some(name))
                    .then_some(Point::Local(i))
            }),
        }
    }

    // Evaluate aliases in their own declaration environments. A later alias
    // replacement must not reinterpret an earlier range or supporting alias.
    fn number(&mut self, at: Point, text: &str, depth: usize) -> Option<num_bigint::BigInt> {
        if depth > 64 {
            return None;
        }
        if let Some(db) = self.db {
            db.unwind_if_revision_cancelled();
        }
        let key = (at, text.to_owned());
        if let Some(value) = self.numbers.get(&key) {
            return value.clone();
        }
        // Memoise repeated alias operands; cyclic aliases remain unknown.
        self.numbers.insert(key.clone(), None);
        let expr = NumericExpr::parse(text)?;
        let mut subst = Subst::default();
        for (token, span) in parser::tokenize(text) {
            if !matches!(token, parser::Token::Id(_)) {
                continue;
            }
            let name = &text[span.start..span.end];
            let Some(origin) = self.resolve(at, name) else { continue };
            let Some(def) =
                self.metadata(origin).filter(|def| def.is_alias && def.params.is_empty())
            else {
                continue;
            };
            let Some(body) = def.bodies.first() else { continue };
            if let Some(value) = self.number(origin, &body.display_text(), depth + 1) {
                subst.values.insert(name.to_owned(), value.to_string());
            }
        }
        let value = integer::eval(&expr, &subst, &[]);
        self.numbers.insert(key, value.clone());
        value
    }

    fn bit_width(&mut self, at: Point, ty: &Ty, depth: usize) -> Option<num_bigint::BigInt> {
        if depth > 64 {
            return None;
        }
        let (name, args) = match ty.kind() {
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
            _ => return None,
        };
        let argument_text = |arg: &TyArg| match arg {
            TyArg::Type(ty) => ty.display_text(),
            _ => arg.as_value_str().unwrap_or_default(),
        };
        if matches!(name.as_str(), "bits" | "bitvector") {
            return self.number(at, &argument_text(args.first()?), depth + 1);
        }
        let origin = self.resolve(at, name)?;
        let def = self.metadata(origin).filter(|def| def.is_alias)?;
        let args: Vec<_> = args
            .iter()
            .map(|arg| {
                self.number(at, &argument_text(arg), depth + 1)
                    .map(|n| TyArg::Nexp(NumericExpr::from_integer(n)))
                    .unwrap_or_else(|| arg.clone())
            })
            .collect();
        let body = substitution::apply_arguments(def.bodies.first()?, &def.params, &args)?;
        self.bit_width(origin, &body, depth + 1)
    }

    fn check_bitfield(&mut self, at: Point, node: &SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
        if !is_bitfield(node) {
            return;
        }
        let width = node
            .children()
            .find(|n| n.kind().is_type_node())
            .and_then(|n| self.bit_width(at, &type_from_cst_node(&n), 0));
        for field in hir_def::item_tree::extract_field_decls(node) {
            let range = base_db::text_range(field.name_span.start, field.name_span.end);
            if field.name.as_str() == "bits" {
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::SailError("reserved-bitfield-field"),
                    "Field with name 'bits' found in bitfield definition. This name is reserved for all bits of the bitfield.".to_owned(),
                    range, Severity::Error,
                ));
            }
            let Some(segments) = hir_def::bitfield::range_indices(&field.type_text) else {
                continue;
            };
            for (hi, lo) in segments {
                let hi = self.number(at, &hi, 0);
                let lo = self.number(at, &lo, 0);
                if hi.as_ref().zip(lo.as_ref()).is_some_and(|(hi, lo)| hi < lo) {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::SailError("bitfield-reversed-range"),
                        "Bitfield range must have its high index greater than or equal to its low index".to_owned(),
                        range, Severity::Error,
                    ));
                }
                let mut endpoints: Vec<_> = hi.into_iter().chain(lo).collect();
                endpoints.sort();
                endpoints.dedup();
                for value in endpoints {
                    if value < num_bigint::BigInt::from(0)
                        || width.as_ref().is_some_and(|w| &value >= w)
                    {
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::SailError("bitfield-index-out-of-range"),
                            format!("Bitvector index {value} is outside the bitfield's bounds"),
                            range,
                            Severity::Error,
                        ));
                    }
                }
            }
        }
    }

    fn record_undefined(&mut self, at: Point, def: &Definition, depth: usize) -> Option<bool> {
        let facts: Vec<_> = def.constraint.clone().into_iter().collect();
        all_known(def.bodies.iter().map(|body| {
            let allowed = self.undefined_allowed(at, body, &facts, depth + 1);
            if allowed != Some(true) {
                return allowed;
            }
            self.inhabited(at, body, &facts, depth + 1, true)
        }))
    }

    // Eligibility for a generated undefined function is checked at the
    // declaration, before concrete type arguments are substituted. In
    // particular, a record with an unconstrained width does not acquire a
    // generator merely because a later use instantiates that width to 1.
    fn undefined_allowed(
        &mut self,
        at: Point,
        ty: &Ty,
        facts: &[ConstraintExpr],
        depth: usize,
    ) -> Option<bool> {
        if depth > 64 {
            return None;
        }
        match ty.kind() {
            TyKind::Error | TyKind::Infer(_) => None,
            TyKind::Exist { .. } | TyKind::Param(_) | TyKind::FnPtr(_) | TyKind::Bidir { .. } => {
                Some(false)
            }
            TyKind::Tuple(types) => {
                all_known(types.iter().map(|t| self.undefined_allowed(at, t, facts, depth + 1)))
            }
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
                let mut parameter_kinds =
                    kinds::arguments(name, args.len()).map(|k| k.to_vec()).unwrap_or_default();
                if let Some(origin) = self.resolve(at, name) {
                    let def = self.metadata(origin)?;
                    parameter_kinds = def
                        .params
                        .iter()
                        .map(|p| def.kinds.get(p).copied().unwrap_or(Kind::Type))
                        .collect();
                    if def.is_alias {
                        let ty =
                            substitution::apply_arguments(def.bodies.first()?, &def.params, args)?;
                        return self.undefined_allowed(origin, &ty, facts, depth + 1);
                    }
                    let generated = match def.undefined_policy.as_deref() {
                        Some("skip") => Some(true),
                        Some("forbid") => Some(false),
                        _ => match def.category {
                            SK::KW_UNION | SK::KW_NEWTYPE => Some(false),
                            SK::KW_STRUCT => self.record_undefined(origin, &def, depth),
                            SK::KW_ENUM | SK::KW_BITFIELD => Some(true),
                            _ => None,
                        },
                    };
                    if generated != Some(true) {
                        return generated;
                    }
                } else if !matches!(
                    name.as_str(),
                    "int"
                        | "nat"
                        | "bool"
                        | "real"
                        | "string"
                        | "unit"
                        | "bit"
                        | "bits"
                        | "bitvector"
                        | "vector"
                        | "range"
                        | "option"
                ) {
                    return None;
                }
                all_known(args.iter().enumerate().filter_map(|(i, a)| match a {
                    TyArg::Type(_) if parameter_kinds.get(i).is_some_and(|k| *k != Kind::Type) => {
                        None
                    }
                    TyArg::Type(t) => Some(self.undefined_allowed(at, t, facts, depth + 1)),
                    _ => None,
                }))
            }
            _ => Some(true),
        }
    }

    fn inhabited(
        &mut self,
        at: Point,
        ty: &Ty,
        facts: &[ConstraintExpr],
        depth: usize,
        require_proof: bool,
    ) -> Option<bool> {
        if depth > 64 {
            return None;
        }
        match ty.kind() {
            TyKind::Error | TyKind::Infer(_) => None,
            TyKind::Tuple(types) => all_known(
                types.iter().map(|t| self.inhabited(at, t, facts, depth + 1, require_proof)),
            ),
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
                if let Some(origin) = self.resolve(at, name) {
                    let def = self.metadata(origin)?;
                    let bodies: Option<Vec<_>> = def
                        .bodies
                        .iter()
                        .map(|body| substitution::apply_arguments(body, &def.params, args))
                        .collect();
                    if def.is_alias || def.category == SK::KW_STRUCT {
                        return all_known(bodies?.iter().map(|body| {
                            self.inhabited(origin, body, facts, depth + 1, require_proof)
                        }));
                    }
                    return None;
                }
                if matches!(name.as_str(), "bits" | "bitvector" | "vector") {
                    let arg = args.first()?;
                    let text = match arg {
                        TyArg::Type(t) => t.display_text(),
                        _ => arg.as_value_str()?,
                    };
                    let width = self
                        .number(at, &text, 0)
                        .map(NumericExpr::from_integer)
                        .or_else(|| NumericExpr::parse(&text))?;
                    let goal = ConstraintExpr::Compare {
                        lhs: width,
                        op: CompareOp::Gte,
                        rhs: NumericExpr::Const(0),
                    };
                    return match solver::solve(&goal, &Subst::default(), facts) {
                        ConstraintStatus::Satisfied => Some(true),
                        ConstraintStatus::Failed => Some(false),
                        ConstraintStatus::Unknown if require_proof => Some(false),
                        _ => None,
                    };
                }
                Some(true)
            }
            _ => Some(true),
        }
    }

    fn arity_error(&mut self, at: Point, ty: &Ty, depth: usize) -> Option<(Point, String)> {
        if depth > 64 {
            return None;
        }
        match ty.kind() {
            TyKind::Named(name, args) | TyKind::App { name, args, .. } => {
                let origin = self.resolve(at, name);
                let definition = origin.and_then(|origin| self.metadata(origin));
                let expected = definition
                    .as_ref()
                    .map(|def| def.params.len())
                    .or_else(|| (name == "vector").then_some(2));
                // Legacy vector(n, order, element) is elaborated to vector(n, element).
                let legacy_vector = origin.is_none()
                    && name == "vector"
                    && args.len() == 3
                    && matches!(args.get(1), Some(TyArg::Order(_)));
                if let Some(expected) = expected {
                    if args.len() != expected && !legacy_vector {
                        return Some((
                            origin.unwrap_or(at),
                            format!(
                                "Type constructor {name} expected {expected} arguments, given {}",
                                args.len()
                            ),
                        ));
                    }
                }
                args.iter().enumerate().find_map(|(index, arg)| {
                    let kind = if let Some(def) = &definition {
                        def.params.get(index).and_then(|param| def.kinds.get(param)).copied()
                    } else {
                        kinds::arguments(name, args.len()).and_then(|ks| ks.get(index)).copied()
                    };
                    // Numeric expressions may still use the shared type-expression
                    // representation. Only Type-kinded arguments contain types.
                    if matches!(kind, Some(Kind::Int | Kind::Bool | Kind::Order)) {
                        return None;
                    }
                    match arg {
                        TyArg::Type(ty) => self.arity_error(at, ty, depth + 1),
                        _ => None,
                    }
                })
            }
            TyKind::Tuple(items) => items.iter().find_map(|ty| self.arity_error(at, ty, depth + 1)),
            TyKind::FnPtr(sig) => sig
                .params
                .iter()
                .chain(std::iter::once(&sig.ret))
                .find_map(|ty| self.arity_error(at, ty, depth + 1)),
            TyKind::Exist { inner, .. } => self.arity_error(at, inner, depth + 1),
            TyKind::Bidir { lhs, rhs } => self
                .arity_error(at, lhs, depth + 1)
                .or_else(|| self.arity_error(at, rhs, depth + 1)),
            _ => None,
        }
    }

    fn check_type(
        &mut self,
        at: Point,
        ty: &Ty,
        assumptions: &[ConstraintExpr],
        visiting: &mut HashSet<Point>,
        require_proof: bool,
    ) -> Option<(Point, ConstraintExpr)> {
        if let Some(db) = self.db {
            db.unwind_if_revision_cancelled();
        }
        match ty.kind() {
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
                for arg in args {
                    if let TyArg::Type(ty) = arg {
                        if let Some(failure) =
                            self.check_type(at, ty, assumptions, visiting, require_proof)
                        {
                            return Some(failure);
                        }
                    }
                }
                let selected = self.resolve(at, name);
                if selected.is_none()
                    && self.strict_bitvector
                    && matches!(name.as_str(), "bits" | "bitvector")
                {
                    let arg = args.first()?;
                    let text = match arg {
                        TyArg::Type(ty) => ty.display_text(),
                        _ => arg.as_value_str()?,
                    };
                    let width = self
                        .number(at, &text, 0)
                        .map(NumericExpr::from_integer)
                        .or_else(|| NumericExpr::parse(&text))?;
                    let goal = ConstraintExpr::Compare {
                        lhs: width,
                        op: CompareOp::Gte,
                        rhs: NumericExpr::from_integer(0.into()),
                    };
                    let status = solver::solve(&goal, &Subst::default(), assumptions);
                    // A nominal definition's parameters are already fixed by
                    // its header. Its fields must be well-formed for every
                    // allowed instantiation, even if no value is constructed.
                    if status == ConstraintStatus::Failed
                        || (require_proof && status != ConstraintStatus::Satisfied)
                    {
                        return Some((at, goal));
                    }
                    return None;
                }
                let selected = selected?;
                let definition = self.metadata(selected)?;
                if definition.params.len() != args.len() || !visiting.insert(selected) {
                    return None;
                }
                let result = (|| {
                    let args: Vec<_> = args
                        .iter()
                        .zip(&definition.params)
                        .map(|(arg, param)| {
                            let arg = kinds::argument(
                                arg,
                                definition.kinds.get(param).copied(),
                                &HashMap::new(),
                            );
                            if definition.kinds.get(param) == Some(&Kind::Int) {
                                if let Some(n) =
                                    arg.as_value_str().and_then(|s| self.number(at, &s, 0))
                                {
                                    return TyArg::Nexp(NumericExpr::from_integer(n));
                                }
                            }
                            arg
                        })
                        .collect();
                    if let Some(constraint) = &definition.constraint {
                        // Reuse the capture-avoiding substitution of existential
                        // predicates; this wrapper introduces no new binders.
                        let wrapper = Ty::new(TyKind::Exist {
                            vars: Vec::new(),
                            constraint: constraint.clone(),
                            inner: Ty::scalar(Scalar::Unit),
                        });
                        if let Some(instantiated) =
                            substitution::apply_arguments(&wrapper, &definition.params, &args)
                        {
                            if let TyKind::Exist { constraint, .. } = instantiated.kind() {
                                if solver::solve(constraint, &Subst::default(), assumptions)
                                    == ConstraintStatus::Failed
                                {
                                    return Some((selected, constraint.clone()));
                                }
                            }
                        }
                    }
                    // A non-parameterised nominal definition has already had
                    // its fields checked at its declaration. Expanding large
                    // closed unions at every use adds no new evidence.
                    for body in definition
                        .bodies
                        .iter()
                        .filter(|_| definition.is_alias || !definition.params.is_empty())
                    {
                        let instantiated =
                            substitution::apply_arguments(body, &definition.params, &args)?;
                        // The RHS retains its declaration's environment, including
                        // private support types, not the caller's later declarations.
                        if let Some(failure) = self.check_type(
                            selected,
                            &instantiated,
                            assumptions,
                            visiting,
                            require_proof,
                        ) {
                            return Some(failure);
                        }
                    }
                    None
                })();
                visiting.remove(&selected);
                result
            }
            TyKind::Exist { vars, constraint, inner } => {
                let opened = self.table.open_existential(vars, constraint, inner);
                let mut assumptions = assumptions.to_vec();
                assumptions.push(opened.constraint);
                self.check_type(at, &opened.inner_ty, &assumptions, visiting, require_proof)
            }
            TyKind::Tuple(types) => types
                .iter()
                .find_map(|ty| self.check_type(at, ty, assumptions, visiting, require_proof)),
            TyKind::FnPtr(sig) => sig
                .params
                .iter()
                .chain(std::iter::once(&sig.ret))
                .find_map(|ty| self.check_type(at, ty, assumptions, visiting, require_proof)),
            TyKind::Bidir { lhs, rhs } => self
                .check_type(at, lhs, assumptions, visiting, require_proof)
                .or_else(|| self.check_type(at, rhs, assumptions, visiting, require_proof)),
            _ => None,
        }
    }
}

/// Check explicit type applications and declaration RHSs. Default mode reports
/// refuted constraints; strict bitvector declarations additionally require a
/// proof of nonnegative width under their declared parameter constraints.
pub fn check(
    input: Option<(&dyn salsa::Database, FileText)>,
    defs: &[SyntaxNode],
) -> Vec<Diagnostic> {
    let ordered = input.filter(|(db, file)| source_order::program(*db, *file).is_some());
    let mut checker = Checker {
        db: input.map(|(db, _)| db),
        defs,
        metadata: HashMap::new(),
        table: InferenceTable::new(),
        numbers: HashMap::new(),
        strict_bitvector: input.is_some_and(|(db, _)| base_db::strict_bitvector(db)),
    };
    for node in defs {
        checker.table.reserve_source_names(&node.text().to_string());
    }
    let mut diagnostics = Vec::new();
    let mut global_facts = Vec::new();
    for (index, node) in defs.iter().enumerate() {
        if let Some(previous) = index.checked_sub(1).and_then(|i| defs.get(i)) {
            if previous.kind() == SK::CONSTRAINT_DEF {
                let text = previous.text().to_string();
                global_facts.push(parse_constraint_text(
                    text.trim().strip_prefix("constraint").unwrap_or(&text).trim(),
                ));
            }
        }
        let at = match ordered {
            Some((db, file)) => {
                match source_order::at_offset(db, file, usize::from(node.text_range().start())) {
                    Some(at) => Point::Ordered(at),
                    None => continue,
                }
            }
            None => Point::Local(index),
        };
        checker.check_bitfield(at, node, &mut diagnostics);
        let metadata = checker.metadata(at);
        let has = |kind| node.children_with_tokens().any(|it| it.kind() == kind);
        let undefined_error = if has(SK::KW_REGISTER)
            && !has(SK::EQ)
            && attribute(node, "initialized_elsewhere").is_none()
        {
            node.children().find(|n| n.kind().is_type_node()).and_then(|syntax| {
                let ty = type_from_cst_node(&syntax);
                (checker.undefined_allowed(at, &ty, &global_facts, 0) == Some(false)
                    || checker.inhabited(at, &ty, &global_facts, 0, false) == Some(false))
                .then(|| {
                    format!(
                        "Must provide a default register value for a register of type {}",
                        syntax.text()
                    )
                })
            })
        } else if let Some(def) = metadata.as_ref().filter(|def| {
            def.category == SK::KW_STRUCT && def.undefined_policy.as_deref() == Some("generate")
        }) {
            (checker.record_undefined(at, def, 0) == Some(false)).then(|| {
                format!(
                    "Cannot generate undefined function for struct {}",
                    syntax::definition_name(node).unwrap_or_default()
                )
            })
        } else {
            None
        };
        if let Some(message) = undefined_error {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("type-error"),
                message,
                node.text_range(),
                Severity::Error,
            ));
        }
        let mut own_facts = global_facts.clone();
        own_facts.extend(metadata.as_ref().and_then(|m| m.constraint.clone()));
        let definition_site = metadata.is_some();
        let type_valued = metadata.as_ref().is_none_or(|def| def.result_kind == Kind::Type);
        let mut checks = Vec::new();
        if let Some(metadata) = metadata {
            if let Some((name, origin)) =
                metadata.bodies.iter().find_map(|ty| checker.unavailable_type(at, ty))
            {
                let message = if origin == at && metadata.is_alias {
                    format!("Failed to infer kind for this type: {name} is not available within its own definition")
                } else {
                    format!("Types are not well-formed within this type definition. Note that recursive types are forbidden. Type {name} is not available at this declaration")
                };
                let range = if origin == at && metadata.is_alias {
                    node.descendants_with_tokens()
                        .filter_map(|element| element.into_token())
                        .skip_while(|token| token.kind() != SK::EQ)
                        .skip(1)
                        .find(|token| token.kind() == SK::IDENT && token.text() == name)
                        .map_or(node.text_range(), |token| token.text_range())
                } else {
                    node.text_range()
                };
                let mut diagnostic = Diagnostic::new(
                    DiagnosticCode::SailError("ill-formed-type"),
                    message,
                    range,
                    Severity::Error,
                );
                checker.related(&mut diagnostic, origin, "type declaration is unavailable here");
                diagnostics.push(diagnostic);
            }
            // Parameterised alias RHSs can be raw CST tokens. The same type
            // parser used by inference handles those without losing binders.
            checks.extend(
                metadata.bodies.into_iter().map(|ty| (node.text_range(), ty, own_facts.clone())),
            );
        }
        // Check a whole type, preserving lexical quantifier scopes. Flattening
        // predicates from TYPE_APP ancestors captures shadowed binders and can
        // make inconsistent assumptions incorrectly prove any obligation.
        let is_type = |kind| {
            matches!(
                kind,
                SK::TYPE_NAMED
                    | SK::TYPE_APP
                    | SK::TYPE_TUPLE
                    | SK::TYPE_ARROW
                    | SK::TYPE_FORALL
                    | SK::TYPE_EXISTENTIAL
                    | SK::TYPE_VAR
                    | SK::TYPE_EFFECT
            )
        };
        for root in node.descendants().filter(|n| is_type(n.kind())) {
            // The grammar also uses type-expression nodes for bitfield numeric
            // endpoints. Those are checked as ranges, not type applications.
            if is_bitfield(node)
                && root.ancestors().take_while(|n| n != node).any(|n| n.kind() == SK::FIELD_INIT)
            {
                continue;
            }
            if root.ancestors().skip(1).take_while(|n| n != node).any(|n| is_type(n.kind())) {
                continue;
            }
            let mut facts = own_facts.clone();
            let mut quantifiers = Vec::new();
            let mut constraints = Vec::new();
            let body = collect_forall_from_cst(&root, &mut quantifiers, &mut constraints)
                .unwrap_or_else(|| root.clone());
            let ty = type_from_cst_node(&body);
            if quantifiers.is_empty() {
                facts.extend(constraints.into_iter().map(|c| c.expr));
                checks.push((root.text_range(), ty, facts));
            } else {
                let predicate =
                    ConstraintExpr::And(constraints.into_iter().map(|c| c.expr).collect());
                let opened = checker.table.open_existential(&quantifiers, &predicate, &ty);
                facts.push(opened.constraint);
                checks.push((root.text_range(), opened.inner_ty, facts));
            }
        }
        let mut reported = HashSet::new();
        for (range, ty, facts) in checks {
            if let Some((origin, message)) =
                type_valued.then(|| checker.arity_error(at, &ty, 0)).flatten()
            {
                let mut diagnostic = Diagnostic::new(
                    DiagnosticCode::SailError("ill-formed-type"),
                    message,
                    range,
                    Severity::Error,
                );
                if origin != at {
                    checker.related(&mut diagnostic, origin, "type constructor declared here");
                }
                diagnostics.push(diagnostic);
                continue;
            }

            if let Some((origin, constraint)) =
                checker.check_type(at, &ty, &facts, &mut HashSet::new(), definition_site)
            {
                if !reported.insert((range, origin)) {
                    continue;
                }
                let mut diagnostic = Diagnostic::new(
                    DiagnosticCode::SailError("ill-formed-type"),
                    format!(
                        "Well-formedness check failed for type: constraint {} does not hold",
                        checker.table.display_constraint(&constraint)
                    ),
                    range,
                    Severity::Error,
                );
                checker.related(&mut diagnostic, origin, "type constraint declared here");
                diagnostics.push(diagnostic);
            }
        }
    }
    diagnostics
}

/// Check a typed undefined value in the lexical declaration environment.
/// Missing external definitions and unresolved inference inputs stay unknown.
pub fn undefined_error(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    ty: &Ty,
    facts: &[ConstraintExpr],
) -> Option<String> {
    let root = syntax::parse_query::semantic_parse_file(db, file).syntax_node()?;
    let defs: Vec<_> = root.children().collect();
    let at = if source_order::program(db, file).is_some() {
        Point::Ordered(source_order::at_offset(db, file, offset)?)
    } else {
        Point::Local(defs.iter().position(|n| {
            usize::from(n.text_range().start()) <= offset
                && offset < usize::from(n.text_range().end())
        })?)
    };
    let mut checker = Checker {
        db: Some(db),
        defs: &defs,
        metadata: HashMap::new(),
        table: InferenceTable::new(),
        numbers: HashMap::new(),
        strict_bitvector: base_db::strict_bitvector(db),
    };
    if checker.undefined_allowed(at, ty, facts, 0) == Some(false) {
        Some(format!("Type {} cannot be undefined", ty.display_text()))
    } else if checker.inhabited(at, ty, facts, 0, false) == Some(false) {
        Some(format!("Type {} could be empty", ty.display_text()))
    } else {
        None
    }
}

/// Compare closed signature types using each declaration's alias environment.
/// Only a structural contradiction is an error; deferred numeric obligations
/// are not converted into proof by this declaration diagnostic.
pub fn instantiated_signature_mismatch(
    db: &dyn salsa::Database,
    outcome_at: Declaration,
    use_at: Declaration,
    actual_at: Declaration,
    expected: &str,
    actual: &str,
    replacements: &[(String, String)],
) -> bool {
    fn normalize(db: &dyn salsa::Database, at: Declaration, text: &str) -> Ty {
        let env = dependencies::ordered::environment(db, at, dependencies::names_in(text));
        let mut table = InferenceTable::with_aliases(env.0.env.type_aliases.clone());
        table.set_alias_schemes(env.0.env.alias_schemes.clone());
        fn walk(table: &InferenceTable, ty: &Ty, depth: usize) -> Ty {
            if depth > 64 {
                return Ty::error();
            }
            let ty = table.normalize_alias_deep(ty);
            match ty.kind() {
                TyKind::FnPtr(sig) => Ty::function(
                    sig.params.iter().map(|ty| walk(table, ty, depth + 1)).collect(),
                    walk(table, &sig.ret, depth + 1),
                ),
                TyKind::Tuple(types) => {
                    Ty::tuple(types.iter().map(|ty| walk(table, ty, depth + 1)).collect())
                }
                TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
                    Ty::new(TyKind::Named(
                        name.clone(),
                        args.iter()
                            .map(|arg| match arg {
                                TyArg::Type(ty) => TyArg::Type(walk(table, ty, depth + 1)),
                                _ => arg.clone(),
                            })
                            .collect(),
                    ))
                }
                TyKind::Bidir { lhs, rhs } => {
                    Ty::bidir(walk(table, lhs, depth + 1), walk(table, rhs, depth + 1))
                }
                TyKind::Exist { vars, constraint, inner } => {
                    Ty::exist(vars.clone(), constraint.clone(), walk(table, inner, depth + 1))
                }
                _ => ty,
            }
        }
        walk(&table, &type_from_type_text(text), 0)
    }
    let expected = normalize(db, outcome_at, expected);
    let (params, args): (Vec<_>, Vec<_>) = replacements
        .iter()
        .map(|(param, value)| (param.clone(), TyArg::Type(normalize(db, use_at, value))))
        .unzip();
    let Some(expected) = substitution::apply_arguments(&expected, &params, &args) else {
        return false;
    };
    let actual = normalize(db, actual_at, actual);
    InferenceTable::new().check_compatibility(&expected, &actual) == unify::Compatibility::Mismatch
}
