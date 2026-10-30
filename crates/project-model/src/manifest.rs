//! Configuration-aware project evaluation, independent of the filesystem and LSP.
//! The manifest is syntax; the resolved graph is the semantic build configuration.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::PathBuf;

use super::{lex, DependencyKind, ProjectError, ProjectErrorKind, Tok, Token};

type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectValue {
    Bool(bool),
    String(String),
    Virtual(String),
    List(Vec<ProjectValue>),
    Selector { name: String, tree: bool },
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectOptions {
    pub variables: BTreeMap<String, ProjectValue>,
    /// Registered virtual contents exposed through physical source files.
    /// Resolution never guesses a path for an unregistered virtual name.
    pub virtual_files: BTreeMap<String, PathBuf>,
    /// Empty means all modules. A root brings what it requires (its parents
    /// included), as `sail --project P A` does, but not its own children.
    pub roots: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Expr {
    kind: ExprKind,
    range: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum ExprKind {
    Value(ProjectValue),
    Variable(String),
    List(Vec<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Binary(Box<Expr>, String, Box<Expr>),
    Call(String, Vec<Expr>),
}
/// A name and where it is written.
type Name = (String, Range<usize>);
#[derive(Clone, Debug, PartialEq, Eq)]
enum Decl {
    Variable(Name, Expr),
    Implicit(Expr),
    /// The name, the offset just inside its `{`, and the body.
    Module(Name, usize, Vec<Decl>),
    Files(Vec<Expr>),
    Directory(Expr),
    Dependency(DependencyKind, Vec<Expr>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectManifest {
    declarations: Vec<Decl>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModule {
    pub name: String,
    /// Where the module's name is written in the manifest.
    pub name_range: Range<usize>,
    pub parent: Option<usize>,
    /// Files owned by this module, excluding child modules.
    pub files: Vec<PathBuf>,
    /// For each of `files`, the manifest expression it came from.
    pub file_ranges: Vec<Range<usize>>,
    /// Direct requirements, including parents and inherited declarations.
    pub requires: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProject {
    pub modules: Vec<ResolvedModule>,
    /// Selected module IDs in stable dependency order.
    pub module_order: Vec<usize>,
    /// Each selected file once, at its first occurrence.
    pub files: Vec<PathBuf>,
}
impl ResolvedProject {
    /// Every `files` entry of the selected modules, in load order, with its
    /// module. A path listed twice appears twice, as `sail --list-files` prints it.
    pub fn file_occurrences(&self) -> impl Iterator<Item = (usize, &PathBuf)> {
        self.module_order.iter().flat_map(|&id| self.modules[id].files.iter().map(move |f| (id, f)))
    }
}
impl ProjectManifest {
    /// Where to write `literal` so that `module` lists it, and the text to
    /// write: after the module's last `files` entry, or in a new `files`
    /// line at the top of its body. Purely syntactic, so it works for a
    /// manifest whose configuration does not evaluate.
    pub fn file_insertion(
        &self,
        source: &str,
        module: &str,
        literal: &str,
    ) -> Option<(usize, String)> {
        fn find<'a>(decls: &'a [Decl], module: &str) -> Option<(&'a Name, usize, &'a [Decl])> {
            decls.iter().find_map(|d| match d {
                Decl::Module(name, inside, body) if name.0 == module => {
                    Some((name, *inside, body.as_slice()))
                }
                Decl::Module(_, _, body) => find(body, module),
                _ => None,
            })
        }
        let ((_, name_range), inside, body) = find(&self.declarations, module)?;
        let last_entry = body
            .iter()
            .filter_map(|d| match d {
                Decl::Files(exprs) => exprs.last().map(|e| e.range.end),
                _ => None,
            })
            .max();
        if let Some(end) = last_entry {
            return Some((end, format!(", {literal}")));
        }
        let line_start = source[..name_range.start].rfind('\n').map_or(0, |i| i + 1);
        let indent: String =
            source[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        Some((inside, format!("\n{indent}  files {literal}")))
    }

    pub fn parse(source: &str) -> Result<Self> {
        let tokens = lex(source)?;
        let mut parser = Parser { tokens, pos: 0, end: source.len(), recover: false };
        Ok(Self { declarations: parser.declarations(None)? })
    }
    /// Resolve complete file expressions at the cursor. An unfinished later
    /// declaration must not disable navigation in already parsed entries.
    /// Dependency selection is irrelevant to navigation, so do not finish the graph.
    pub fn file_paths_at(source: &str, offset: usize, options: &ProjectOptions) -> Vec<PathBuf> {
        let Ok(tokens) = lex(source) else { return Vec::new() };
        let mut parser = Parser { tokens, pos: 0, end: source.len(), recover: true };
        let Ok(declarations) = parser.declarations(None) else { return Vec::new() };
        let mut evaluator = Evaluator {
            variables: options.variables.clone(),
            virtual_files: &options.virtual_files,
            declared: BTreeSet::new(),
            modules: Vec::new(),
            names: BTreeMap::new(),
            edges: Vec::new(),
        };
        let _ = evaluator.declarations(&declarations, None, &[]);
        evaluator
            .modules
            .iter()
            .flat_map(|m| m.files.iter().zip(&m.file_ranges))
            .filter(|(_, range)| range.contains(&offset))
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn resolve(&self, options: &ProjectOptions) -> Result<ResolvedProject> {
        let mut evaluator = Evaluator {
            variables: options.variables.clone(),
            virtual_files: &options.virtual_files,
            declared: BTreeSet::new(),
            modules: Vec::new(),
            names: BTreeMap::new(),
            edges: Vec::new(),
        };
        evaluator.declarations(&self.declarations, None, &[])?;
        evaluator.finish(&options.roots)
    }
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "end of file".into(),
        Some(Tok::Id(s) | Tok::FileId(s)) => format!("`{s}`"),
        Some(Tok::Str(s)) => format!("\"{s}\""),
        Some(Tok::Var(s)) => format!("`${s}`"),
        Some(t) => format!("`{}`", spelling(t)),
    }
}
fn spelling(t: &Tok) -> &str {
    match t {
        Tok::LCurly => "{",
        Tok::RCurly => "}",
        Tok::LSquare => "[",
        Tok::RSquare => "]",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::Comma => ",",
        Tok::Semi => ";",
        Tok::Eq => "=",
        Tok::OpCompare(op) => op,
        Tok::DotDot => "..",
        Tok::Slash => "/",
        Tok::KwFiles => "files",
        Tok::KwDirectory => "directory",
        Tok::KwRequires => "requires",
        Tok::KwAfter => "after",
        Tok::KwBefore => "before",
        Tok::KwIf => "if",
        Tok::KwThen => "then",
        Tok::KwElse => "else",
        Tok::KwTrue => "true",
        Tok::KwFalse => "false",
        Tok::KwVariable => "variable",
        Tok::KwTest => "__test",
        Tok::Id(s) | Tok::FileId(s) | Tok::Str(s) | Tok::Var(s) => s,
    }
}

struct Parser {
    recover: bool,
    tokens: Vec<Token>,
    pos: usize,
    /// Where end of file is, for an error there.
    end: usize,
}
impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }
    fn take(&mut self) -> Option<Tok> {
        let t = self.peek()?.clone();
        self.pos += 1;
        Some(t)
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    /// The range of the token at `pos`, or of end of file.
    fn range_at(&self, pos: usize) -> Range<usize> {
        self.tokens.get(pos).map_or(self.end..self.end, |t| t.range.clone())
    }
    /// From the token at `start` through the last one taken.
    fn since(&self, start: usize) -> Range<usize> {
        let begin = self.range_at(start).start;
        let end = self.pos.checked_sub(1).map_or(begin, |last| self.range_at(last).end);
        begin..end.max(begin)
    }
    /// A syntax error at the token at `pos`.
    fn error_at(&self, pos: usize, message: String) -> ProjectError {
        ProjectError::new(ProjectErrorKind::Syntax, self.range_at(pos), message)
    }
    fn expect(&mut self, t: Tok) -> Result<()> {
        if self.eat(&t) {
            Ok(())
        } else {
            Err(self.error_at(
                self.pos,
                format!("Expected `{}`, found {}", spelling(&t), describe(self.peek())),
            ))
        }
    }
    fn name(&mut self) -> Result<Name> {
        let range = self.range_at(self.pos);
        match self.take() {
            Some(Tok::Id(n)) => Ok((n, range)),
            t => Err(self.error_at(
                self.pos - 1,
                format!("Expected a variable name, found {}", describe(t.as_ref())),
            )),
        }
    }
    /// The declarations of a module body (`opened_at` is its `{`), or of the
    /// whole file.
    fn declarations(&mut self, opened_at: Option<usize>) -> Result<Vec<Decl>> {
        let nested = opened_at.is_some();
        let mut out = Vec::new();
        while self.peek().is_some() {
            if self.eat(&Tok::Semi) {
                continue;
            }
            let at = self.pos;
            if self.eat(&Tok::RCurly) {
                return if nested {
                    Ok(out)
                } else {
                    Err(self.error_at(at, "Unexpected `}` outside a module".into()))
                };
            }
            let parsed = (|| -> Result<Option<Decl>> {
                Ok(Some(match self.take() {
                    Some(Tok::KwVariable) if !nested => {
                        let name = self.name()?;
                        self.expect(Tok::Eq)?;
                        Decl::Variable(name, self.expr()?)
                    }
                    Some(Tok::Id(n)) if n == "implicit" && !nested => Decl::Implicit(self.expr()?),
                    Some(Tok::Id(n)) => {
                        let brace = self.pos;
                        if !self.eat(&Tok::LCurly) {
                            return Err(self.error_at(
                                brace,
                                format!(
                                    "Expected `{{` to open module `{n}`, found {}",
                                    describe(self.peek())
                                ),
                            ));
                        }
                        let inside = self.range_at(brace).end;
                        Decl::Module(
                            (n, self.range_at(at)),
                            inside,
                            self.declarations(Some(brace))?,
                        )
                    }
                    Some(Tok::KwFiles) if nested => Decl::Files(self.comma_block()?),
                    Some(Tok::KwDirectory) if nested => Decl::Directory(self.expr()?),
                    Some(t @ (Tok::KwRequires | Tok::KwAfter | Tok::KwBefore)) if nested => {
                        let kind = match t {
                            Tok::KwRequires => DependencyKind::Requires,
                            Tok::KwAfter => DependencyKind::After,
                            _ => DependencyKind::Before,
                        };
                        Decl::Dependency(kind, self.comma_block()?)
                    }
                    Some(Tok::KwTest) if !nested => {
                        // Compiler assertions do not contribute files or graph edges.
                        while matches!(self.peek(), Some(Tok::Id(_)))
                            && self.tokens.get(self.pos + 1).map(|t| &t.tok) != Some(&Tok::LCurly)
                        {
                            self.take();
                        }
                        return Ok(None);
                    }
                    t => {
                        let place = if nested { "in a module" } else { "at the top level" };
                        return Err(self
                            .error_at(at, format!("Unexpected {} {place}", describe(t.as_ref()))));
                    }
                }))
            })();
            match parsed {
                Ok(Some(decl)) => out.push(decl),
                Ok(None) => (),
                Err(_) if self.recover => return Ok(out),
                Err(error) => return Err(error),
            }
        }
        if self.recover {
            return Ok(out);
        }
        match opened_at {
            Some(brace) => Err(self.error_at(brace, "This `{` is never closed".into())),
            None => Ok(out),
        }
    }
    fn comma_block(&mut self) -> Result<Vec<Expr>> {
        let braced = self.eat(&Tok::LCurly);
        let mut values = vec![self.expr()?];
        while self.eat(&Tok::Comma) {
            if matches!(
                self.peek(),
                None | Some(
                    Tok::RCurly
                        | Tok::Semi
                        | Tok::KwFiles
                        | Tok::KwDirectory
                        | Tok::KwRequires
                        | Tok::KwAfter
                        | Tok::KwBefore
                )
            ) || (matches!(self.peek(), Some(Tok::Id(_)))
                && self.tokens.get(self.pos + 1).map(|t| &t.tok) == Some(&Tok::LCurly))
            {
                break;
            }
            values.push(self.expr()?);
        }
        if braced {
            self.expect(Tok::RCurly)?;
        }
        Ok(values)
    }
    fn expr(&mut self) -> Result<Expr> {
        let start = self.pos;
        if self.eat(&Tok::KwIf) {
            let cond = self.expr()?;
            self.expect(Tok::KwThen)?;
            let yes = self.expr()?;
            self.expect(Tok::KwElse)?;
            let no = self.expr()?;
            let kind = ExprKind::If(Box::new(cond), Box::new(yes), Box::new(no));
            return Ok(Expr { kind, range: self.since(start) });
        }
        let lhs = self.path()?;
        if let Some(Tok::OpCompare(op)) = self.peek().cloned() {
            self.take();
            let kind = ExprKind::Binary(Box::new(lhs), op, Box::new(self.path()?));
            Ok(Expr { kind, range: self.since(start) })
        } else {
            Ok(lhs)
        }
    }
    fn path(&mut self) -> Result<Expr> {
        let start = self.pos;
        let lhs = self.atom()?;
        if self.eat(&Tok::Slash) {
            let kind = ExprKind::Binary(Box::new(lhs), "/".into(), Box::new(self.path()?));
            Ok(Expr { kind, range: self.since(start) })
        } else {
            Ok(lhs)
        }
    }
    fn atom(&mut self) -> Result<Expr> {
        let start = self.pos;
        let kind = match self.take() {
            Some(Tok::KwTrue) => ExprKind::Value(ProjectValue::Bool(true)),
            Some(Tok::KwFalse) => ExprKind::Value(ProjectValue::Bool(false)),
            Some(Tok::Var(v)) => ExprKind::Variable(v),
            Some(Tok::DotDot) => ExprKind::Value(ProjectValue::String("..".into())),
            Some(Tok::Id(n)) if self.eat(&Tok::LParen) => {
                let args = self.list(Tok::RParen)?;
                ExprKind::Call(n, args)
            }
            Some(Tok::Id(n)) if n == "__virtual" => {
                let Some(Tok::FileId(name)) = self.take() else {
                    return Err(self.error_at(start, "Expected a filename after __virtual".into()));
                };
                ExprKind::Value(ProjectValue::Virtual(name))
            }
            Some(Tok::Id(n) | Tok::FileId(n) | Tok::Str(n)) => {
                ExprKind::Value(ProjectValue::String(n))
            }
            Some(Tok::LSquare) => ExprKind::List(self.list(Tok::RSquare)?),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                self.expect(Tok::RParen)?;
                e.kind
            }
            t => {
                return Err(self
                    .error_at(start, format!("Expected a value, found {}", describe(t.as_ref()))))
            }
        };
        Ok(Expr { kind, range: self.since(start) })
    }
    fn list(&mut self, end: Tok) -> Result<Vec<Expr>> {
        let mut out = Vec::new();
        if self.eat(&end) {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat(&end) {
                break;
            }
            self.expect(Tok::Comma)?;
            if self.eat(&end) {
                break;
            }
        }
        Ok(out)
    }
}

fn type_error(expr: &Expr, message: impl Into<String>) -> ProjectError {
    ProjectError::new(ProjectErrorKind::Type, expr.range.clone(), message)
}

fn eval(expr: &Expr, variables: &BTreeMap<String, ProjectValue>) -> Result<ProjectValue> {
    use ProjectValue as V;
    Ok(match &expr.kind {
        ExprKind::Value(v) => v.clone(),
        ExprKind::Variable(n) => variables.get(n).cloned().ok_or_else(|| {
            ProjectError::new(
                ProjectErrorKind::UnknownVariable,
                expr.range.clone(),
                format!("Unknown project variable ${n}"),
            )
        })?,
        ExprKind::List(xs) => {
            V::List(xs.iter().map(|x| eval(x, variables)).collect::<Result<_>>()?)
        }
        ExprKind::If(c, a, b) => match eval(c, variables)? {
            V::Bool(true) => eval(a, variables)?,
            V::Bool(false) => eval(b, variables)?,
            _ => return Err(type_error(c, "Expected boolean value in if")),
        },
        ExprKind::Binary(a, op, b) => {
            match (eval(a, variables)?, op.as_str(), eval(b, variables)?) {
                (V::String(a), "/", V::String(b)) => V::String(format!("{a}/{b}")),
                (V::String(a), "/", V::Virtual(b)) => V::Virtual(format!("{a}/{b}")),
                (V::Virtual(a), "==", V::Virtual(b)) => V::Bool(a == b),
                (V::Virtual(a), "!=", V::Virtual(b)) => V::Bool(a != b),
                (V::String(a), "==", V::String(b)) => V::Bool(a == b),
                (V::String(a), "!=", V::String(b)) => V::Bool(a != b),
                (V::Bool(a), "==", V::Bool(b)) => V::Bool(a == b),
                (V::Bool(a), "!=", V::Bool(b)) => V::Bool(a != b),
                _ => {
                    return Err(type_error(
                        expr,
                        format!("Unsupported project operation or operand types: {op}"),
                    ))
                }
            }
        }
        ExprKind::Call(name, args) => {
            let values = args.iter().map(|e| eval(e, variables)).collect::<Result<Vec<_>>>()?;
            match (name.as_str(), values.as_slice()) {
                ("tree" | "only", [V::String(n)]) => {
                    V::Selector { name: n.clone(), tree: name == "tree" }
                }
                ("error", [V::String(message)]) => {
                    return Err(ProjectError::new(
                        ProjectErrorKind::Explicit,
                        expr.range.clone(),
                        message.clone(),
                    ))
                }
                _ => {
                    return Err(type_error(
                        expr,
                        format!("Invalid project function or arguments: {name}"),
                    ))
                }
            }
        }
    })
}
fn flatten(value: ProjectValue) -> Vec<ProjectValue> {
    match value {
        ProjectValue::List(xs) => xs.into_iter().flat_map(flatten).collect(),
        v => vec![v],
    }
}
#[derive(Clone)]
struct Edge {
    kind: DependencyKind,
    name: String,
    tree: bool,
    /// The selector expression that named it.
    range: Range<usize>,
}
struct Evaluator<'a> {
    variables: BTreeMap<String, ProjectValue>,
    virtual_files: &'a BTreeMap<String, PathBuf>,
    declared: BTreeSet<String>,
    modules: Vec<ResolvedModule>,
    names: BTreeMap<String, usize>,
    edges: Vec<Vec<Edge>>,
}
impl Evaluator<'_> {
    fn selectors(&self, exprs: &[Expr], kind: DependencyKind) -> Result<Vec<Edge>> {
        let mut out = Vec::new();
        for expr in exprs {
            for v in flatten(eval(expr, &self.variables)?) {
                let (name, tree) = match v {
                    ProjectValue::String(n) => (n, true),
                    ProjectValue::Selector { name, tree } => (name, tree),
                    _ => return Err(type_error(expr, "Expected a module selector")),
                };
                out.push(Edge { kind, name, tree, range: expr.range.clone() });
            }
        }
        Ok(out)
    }
    fn declarations(
        &mut self,
        decls: &[Decl],
        parent: Option<usize>,
        inherited: &[Edge],
    ) -> Result<()> {
        let mut edges = inherited.to_vec();
        for decl in decls {
            match decl {
                Decl::Variable((name, range), expr) => {
                    if !self.declared.insert(name.clone()) {
                        return Err(ProjectError::new(
                            ProjectErrorKind::Duplicate,
                            range.clone(),
                            format!("Duplicate project variable {name}"),
                        ));
                    }
                    let value = eval(expr, &self.variables)?;
                    self.variables.entry(name.clone()).or_insert(value);
                }
                Decl::Implicit(expr) => edges
                    .extend(self.selectors(std::slice::from_ref(expr), DependencyKind::Requires)?),
                Decl::Directory(expr) => {
                    // Sail currently evaluates this declaration but does not
                    // apply it to file paths (project.ml: structure_visitor).
                    let _ = eval(expr, &self.variables)?;
                }
                Decl::Dependency(kind, exprs) => edges.extend(self.selectors(exprs, *kind)?),
                Decl::Files(exprs) => {
                    let Some(id) = parent else {
                        let range = exprs.first().map_or(0..0, |e| e.range.clone());
                        return Err(ProjectError::new(
                            ProjectErrorKind::Syntax,
                            range,
                            "Files must belong to a module",
                        ));
                    };
                    for expr in exprs {
                        for value in flatten(eval(expr, &self.variables)?) {
                            let path = match value {
                                ProjectValue::String(path) => PathBuf::from(path),
                                ProjectValue::Virtual(name) => self.virtual_files.get(&name)
                                    .cloned().ok_or_else(|| ProjectError::new(
                                        ProjectErrorKind::VirtualFile,
                                        expr.range.clone(),
                                        format!("Virtual file {name} is not registered; supply its generated source through workspace.generatedFiles"),
                                    ))?,
                                _ => return Err(type_error(expr, "Expected a project file path")),
                            };
                            self.modules[id].files.push(path);
                            self.modules[id].file_ranges.push(expr.range.clone());
                        }
                    }
                }
                Decl::Module((name, range), _, body) => {
                    let id = self.modules.len();
                    if self.names.contains_key(name) {
                        return Err(ProjectError::new(
                            ProjectErrorKind::Duplicate,
                            range.clone(),
                            format!("Duplicate module name '{name}'"),
                        ));
                    }
                    self.names.insert(name.clone(), id);
                    self.modules.push(ResolvedModule {
                        name: name.clone(),
                        name_range: range.clone(),
                        parent,
                        files: Vec::new(),
                        file_ranges: Vec::new(),
                        requires: Vec::new(),
                    });
                    self.edges.push(Vec::new());
                    self.declarations(body, Some(id), &edges)?;
                }
            }
        }
        if let Some(id) = parent {
            self.edges[id] = edges;
        }
        Ok(())
    }
    fn finish(mut self, roots: &[String]) -> Result<ResolvedProject> {
        let len = self.modules.len();
        let mut children = vec![Vec::new(); len];
        for (id, m) in self.modules.iter().enumerate() {
            if let Some(p) = m.parent {
                children[p].push(id);
            }
        }
        fn descendants(id: usize, children: &[Vec<usize>], out: &mut BTreeSet<usize>) {
            if out.insert(id) {
                for &child in &children[id] {
                    descendants(child, children, out);
                }
            }
        }
        let mut order_edges = vec![BTreeSet::new(); len];
        for id in 0..len {
            let mut requires = BTreeSet::new();
            let mut p = self.modules[id].parent;
            while let Some(parent) = p {
                requires.insert(parent);
                p = self.modules[parent].parent;
            }
            for edge in &self.edges[id] {
                let target = *self.names.get(&edge.name).ok_or_else(|| {
                    ProjectError::new(
                        ProjectErrorKind::UnknownModule,
                        edge.range.clone(),
                        format!("Module {} does not exist", edge.name),
                    )
                })?;
                let mut targets = BTreeSet::from([target]);
                if edge.tree {
                    targets.clear();
                    descendants(target, &children, &mut targets);
                }
                match edge.kind {
                    DependencyKind::Requires => requires.extend(targets),
                    DependencyKind::After => order_edges[id].extend(targets),
                    DependencyKind::Before => {
                        for target in targets {
                            order_edges[target].insert(id);
                        }
                    }
                }
            }
            order_edges[id].extend(&requires);
            self.modules[id].requires = requires.into_iter().collect();
        }
        fn visit(
            id: usize,
            edges: &[BTreeSet<usize>],
            state: &mut [u8],
            stack: &mut Vec<usize>,
            out: &mut Vec<usize>,
            modules: &[ResolvedModule],
        ) -> Result<()> {
            match state[id] {
                2 => return Ok(()),
                1 => {
                    let start = stack.iter().position(|n| *n == id).unwrap_or(0);
                    let mut cycle: Vec<_> =
                        stack[start..].iter().map(|n| modules[*n].name.as_str()).collect();
                    cycle.push(&modules[id].name);
                    return Err(ProjectError::new(
                        ProjectErrorKind::Cycle,
                        modules[id].name_range.clone(),
                        format!(
                            "Cyclic modules detected: {} depends on itself (cycle: {})",
                            modules[id].name,
                            cycle.join(" -> ")
                        ),
                    ));
                }
                _ => {}
            }
            state[id] = 1;
            stack.push(id);
            for &dep in &edges[id] {
                visit(dep, edges, state, stack, out, modules)?;
            }
            stack.pop();
            state[id] = 2;
            out.push(id);
            Ok(())
        }
        let mut order = Vec::new();
        let mut state = vec![0; len];
        for id in 0..len {
            visit(id, &order_edges, &mut state, &mut Vec::new(), &mut order, &self.modules)?;
        }
        let mut active = BTreeSet::new();
        if roots.is_empty() {
            active.extend(0..len);
        } else {
            for name in roots {
                let id = *self.names.get(name).ok_or_else(|| ProjectError {
                    kind: ProjectErrorKind::UnknownRoot,
                    range: None,
                    message: format!("Unknown root module {name} in workspace.projectModules"),
                })?;
                active.insert(id);
            }
            let mut pending: Vec<_> = active.iter().copied().collect();
            while let Some(id) = pending.pop() {
                for &dep in &self.modules[id].requires {
                    if active.insert(dep) {
                        pending.push(dep);
                    }
                }
            }
        }
        order.retain(|id| active.contains(id));
        let mut seen = BTreeSet::new();
        let files = order
            .iter()
            .flat_map(|&id| self.modules[id].files.iter().cloned())
            .filter(|p| seen.insert(p.clone()))
            .collect();
        Ok(ResolvedProject { modules: self.modules, module_order: order, files })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn navigation_evaluates_file_expressions_before_incomplete_declarations() {
        let source = r#"variable BASE = "lib"
variable PICK = true
A { files $BASE / "x.sail", if $PICK then ["y.sail", "z.sail"] else missing() }
B { requires "#;
        let options = super::ProjectOptions::default();
        assert!(super::ProjectManifest::parse(source).is_err());
        assert_eq!(
            super::ProjectManifest::file_paths_at(source, source.find("x.sail").unwrap(), &options),
            vec![std::path::PathBuf::from("lib/x.sail")]
        );
        assert_eq!(
            super::ProjectManifest::file_paths_at(source, source.find("y.sail").unwrap(), &options),
            vec![std::path::PathBuf::from("y.sail"), std::path::PathBuf::from("z.sail")]
        );
        let mut options = options;
        options.variables.insert("BASE".into(), super::ProjectValue::String("override".into()));
        assert_eq!(
            super::ProjectManifest::file_paths_at(source, source.find("x.sail").unwrap(), &options),
            vec![std::path::PathBuf::from("override/x.sail")]
        );
    }
    use super::*;

    #[test]
    fn virtual_files_use_registered_sources_and_preserve_value_kind() {
        let options = ProjectOptions {
            virtual_files: BTreeMap::from([(
                "generated/defs.sail".into(),
                PathBuf::from("build/actual.sail"),
            )]),
            ..Default::default()
        };
        let project = resolve(
            "variable SAME = __virtual a.sail == __virtual a.sail\nMain { files [generated / __virtual defs.sail, if $SAME then plain.sail else __virtual missing.sail] }",
            options,
        );
        assert_eq!(files(&project), ["build/actual.sail", "plain.sail"]);
        let unregistered = ProjectManifest::parse("Main { files __virtual defs.sail }").unwrap();
        assert!(unregistered
            .resolve(&ProjectOptions::default())
            .unwrap_err()
            .message
            .contains("defs.sail"));
        assert!(ProjectManifest::parse("Main { files __virtual }").is_err());
    }

    fn resolve(text: &str, options: ProjectOptions) -> ResolvedProject {
        ProjectManifest::parse(text).unwrap().resolve(&options).unwrap()
    }
    fn files(p: &ResolvedProject) -> Vec<String> {
        p.files.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }
    #[test]
    fn evaluates_config_and_short_circuits_inactive_branch() {
        let text = "variable RMEM = false\nM { files if $RMEM then rmem.sail else seq.sail, if true then tail.sail else error(\"inactive\") }";
        assert_eq!(files(&resolve(text, ProjectOptions::default())), ["seq.sail", "tail.sail"]);
        let options = ProjectOptions {
            variables: BTreeMap::from([("RMEM".into(), ProjectValue::Bool(true))]),
            ..Default::default()
        };
        assert_eq!(files(&resolve(text, options)), ["rmem.sail", "tail.sail"]);
    }
    #[test]
    fn evaluates_variables_lists_paths_and_nested_conditionals() {
        let p = resolve("variable ARCH = A64\nvariable BASE = lib\nM { directory $BASE files if $ARCH == A32 then narrow.sail else if true then [cpu / wide.sail, end.sail] else [] }", ProjectOptions::default());
        assert_eq!(files(&p), ["cpu/wide.sail", "end.sail"]);
        let p = resolve("M { files \"模型/测试.sail\" }", ProjectOptions::default());
        assert_eq!(files(&p), ["模型/测试.sail"]);
    }
    #[test]
    fn requires_sorts_and_selects_but_after_only_orders() {
        let text = "C { requires B after A files c.sail } A { files a.sail } B { files b.sail }";
        let p = resolve(text, ProjectOptions { roots: vec!["C".into()], ..Default::default() });
        assert_eq!(files(&p), ["b.sail", "c.sail"]);
        assert_eq!(p.modules[0].requires, [2]);
        assert_eq!(
            files(&resolve(text, ProjectOptions::default())),
            ["a.sail", "b.sail", "c.sail"]
        );
    }
    #[test]
    fn parent_edges_and_selectors_preserve_module_ownership() {
        let text = "Main { requires only(P) files main.sail } P { files p.sail Child { files child.sail } }";
        let p = resolve(text, ProjectOptions { roots: vec!["Main".into()], ..Default::default() });
        assert_eq!(files(&p), ["p.sail", "main.sail"]);
        assert_eq!(p.modules[1].files, [PathBuf::from("p.sail")]);
        let text = text.replace("only(P)", "tree(P)");
        let p = resolve(&text, ProjectOptions { roots: vec!["Main".into()], ..Default::default() });
        assert_eq!(files(&p), ["p.sail", "child.sail", "main.sail"]);
    }
    #[test]
    fn implicit_and_parent_requirements_reach_children() {
        let p = resolve(
            "Lib { files lib.sail } implicit only(Lib) P { files p.sail C { files c.sail } }",
            ProjectOptions { roots: vec!["C".into()], ..Default::default() },
        );
        assert_eq!(files(&p), ["lib.sail", "p.sail", "c.sail"]);
        assert_eq!(p.modules[2].requires, [0, 1]);
    }
    #[test]
    fn rejects_invalid_graphs_and_configuration_without_union_fallback() {
        for text in [
            "M { files if $MISSING then a.sail else b.sail }",
            "M { files if A then a.sail else b.sail }",
            "M { requires Missing files a.sail }",
            "A { requires B } B { requires A }",
            "A { before B } B { before A }",
            "A {} A {}",
            "variable V = true variable V = false A {}",
        ] {
            assert!(
                ProjectManifest::parse(text).unwrap().resolve(&ProjectOptions::default()).is_err(),
                "{text}"
            );
        }
        assert!(ProjectManifest::parse("M { files a.sail").is_err());
        assert!(ProjectManifest::parse("M { files if true then a.sail }").is_err());
    }

    /// Every error says what kind of mistake it is and where it is, so the
    /// editor can underline it in the manifest.
    #[test]
    fn errors_carry_their_kind_and_range() {
        use ProjectErrorKind as K;
        let at = |text: &str, roots: &[&str]| {
            let error = ProjectManifest::parse(text)
                .and_then(|m| {
                    m.resolve(&ProjectOptions {
                        roots: roots.iter().map(|r| r.to_string()).collect(),
                        ..Default::default()
                    })
                })
                .unwrap_err();
            let spelled = error.range.clone().map(|r| text[r].to_owned());
            (error.kind, spelled)
        };
        let some = |s: &str| Some(s.to_owned());
        assert_eq!(at("M { files a.sail", &[]), (K::Syntax, some("{")));
        assert_eq!(at("M { files a.sail } }", &[]), (K::Syntax, some("}")));
        assert_eq!(at("M { files \"a\\q.sail\" }", &[]), (K::Syntax, some("\\")));
        assert_eq!(at("variable default = true", &[]), (K::Syntax, some("default")));
        assert_eq!(at("M\n{ files ; }", &[]), (K::Syntax, some(";")));
        assert_eq!(at("A {}\nB { requires A, Nope }", &[]), (K::UnknownModule, some("Nope")));
        assert_eq!(at("A { requires B }\nB { requires A }", &[]), (K::Cycle, some("A")));
        assert_eq!(at("A {}\nA {}", &[]), (K::Duplicate, some("A")));
        assert_eq!(at("variable V = true\nvariable V = false", &[]), (K::Duplicate, some("V")));
        assert_eq!(
            at("M { files if $X then a.sail else b.sail }", &[]),
            (K::UnknownVariable, some("$X"))
        );
        assert_eq!(
            at("M { files if a.sail then a.sail else b.sail }", &[]),
            (K::Type, some("a.sail"))
        );
        assert_eq!(
            at("M { files [a.sail, error(\"no\")] }", &[]),
            (K::Explicit, some("error(\"no\")"))
        );
        assert_eq!(
            at("M { files __virtual gen.sail }", &[]),
            (K::VirtualFile, some("__virtual gen.sail"))
        );
        assert_eq!(at("M {}", &["N"]), (K::UnknownRoot, None));
    }

    #[test]
    fn modules_and_files_remember_where_they_are_written() {
        let text = "Lib {\n  files a.sail, if true then [b.sail, c.sail] else []\n}";
        let project = resolve(text, ProjectOptions::default());
        let m = &project.modules[0];
        assert_eq!(&text[m.name_range.clone()], "Lib");
        let spelled: Vec<_> = m.file_ranges.iter().map(|r| &text[r.clone()]).collect();
        let branch = "if true then [b.sail, c.sail] else []";
        assert_eq!(spelled, ["a.sail", branch, branch]);
    }
}
