//! Configuration-aware project evaluation, independent of the filesystem and LSP.
//! The manifest is syntax; the resolved graph is the semantic build configuration.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::{lex, DependencyKind, Tok};

type Result<T> = std::result::Result<T, String>;

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
    /// Empty means all modules; a selected module also selects its children.
    pub roots: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    Value(ProjectValue),
    Variable(String),
    List(Vec<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Binary(Box<Expr>, String, Box<Expr>),
    Call(String, Vec<Expr>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Decl {
    Variable(String, Expr),
    Implicit(Expr),
    Module(String, Vec<Decl>),
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
    pub parent: Option<usize>,
    /// Files owned by this module, excluding child modules.
    pub files: Vec<PathBuf>,
    /// Direct requirements, including parents and inherited declarations.
    pub requires: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProject {
    pub modules: Vec<ResolvedModule>,
    /// Selected module IDs in stable dependency order.
    pub module_order: Vec<usize>,
    pub files: Vec<PathBuf>,
}
impl ProjectManifest {
    pub fn parse(source: &str) -> Result<Self> {
        let tokens = lex(source).map_err(|e| e.to_string())?;
        let mut parser = Parser { tokens, pos: 0 };
        Ok(Self { declarations: parser.declarations(false)? })
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

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}
impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
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
    fn expect(&mut self, t: Tok) -> Result<()> {
        if self.eat(&t) {
            Ok(())
        } else {
            Err(format!("Expected {t:?}, found {:?} at token {}", self.peek(), self.pos))
        }
    }
    fn name(&mut self) -> Result<String> {
        match self.take() {
            Some(Tok::Id(n)) => Ok(n),
            t => Err(format!("Expected a module or variable name, found {t:?}")),
        }
    }
    fn declarations(&mut self, nested: bool) -> Result<Vec<Decl>> {
        let mut out = Vec::new();
        while self.peek().is_some() {
            if self.eat(&Tok::Semi) {
                continue;
            }
            if self.eat(&Tok::RCurly) {
                return if nested {
                    Ok(out)
                } else {
                    Err("Unexpected closing module brace".into())
                };
            }
            let decl = match self.take() {
                Some(Tok::KwVariable) if !nested => {
                    let name = self.name()?;
                    self.expect(Tok::Eq)?;
                    Decl::Variable(name, self.expr()?)
                }
                Some(Tok::Id(n)) if n == "implicit" && !nested => Decl::Implicit(self.expr()?),
                Some(Tok::Id(n)) => {
                    self.expect(Tok::LCurly)?;
                    Decl::Module(n, self.declarations(true)?)
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
                        && self.tokens.get(self.pos + 1) != Some(&Tok::LCurly)
                    {
                        self.take();
                    }
                    continue;
                }
                t => {
                    return Err(format!(
                        "Unexpected project declaration {t:?} at token {}",
                        self.pos
                    ))
                }
            };
            out.push(decl);
        }
        if nested {
            Err("Unclosed module brace".into())
        } else {
            Ok(out)
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
                && self.tokens.get(self.pos + 1) == Some(&Tok::LCurly))
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
        if self.eat(&Tok::KwIf) {
            let cond = self.expr()?;
            self.expect(Tok::KwThen)?;
            let yes = self.expr()?;
            self.expect(Tok::KwElse)?;
            return Ok(Expr::If(Box::new(cond), Box::new(yes), Box::new(self.expr()?)));
        }
        let lhs = self.path()?;
        if let Some(Tok::OpCompare(op)) = self.peek().cloned() {
            self.take();
            Ok(Expr::Binary(Box::new(lhs), op, Box::new(self.path()?)))
        } else {
            Ok(lhs)
        }
    }
    fn path(&mut self) -> Result<Expr> {
        let lhs = self.atom()?;
        if self.eat(&Tok::Slash) {
            Ok(Expr::Binary(Box::new(lhs), "/".into(), Box::new(self.path()?)))
        } else {
            Ok(lhs)
        }
    }
    fn atom(&mut self) -> Result<Expr> {
        Ok(match self.take() {
            Some(Tok::KwTrue) => Expr::Value(ProjectValue::Bool(true)),
            Some(Tok::KwFalse) => Expr::Value(ProjectValue::Bool(false)),
            Some(Tok::Var(v)) => Expr::Variable(v),
            Some(Tok::DotDot) => Expr::Value(ProjectValue::String("..".into())),
            Some(Tok::Id(n)) if self.eat(&Tok::LParen) => {
                let args = self.list(Tok::RParen)?;
                Expr::Call(n, args)
            }
            Some(Tok::Id(n)) if n == "__virtual" => {
                let Some(Tok::FileId(name)) = self.take() else {
                    return Err("Expected a filename after __virtual".into());
                };
                Expr::Value(ProjectValue::Virtual(name))
            }
            Some(Tok::Id(n) | Tok::FileId(n) | Tok::Str(n)) => Expr::Value(ProjectValue::String(n)),
            Some(Tok::LSquare) => Expr::List(self.list(Tok::RSquare)?),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                self.expect(Tok::RParen)?;
                e
            }
            t => {
                return Err(format!(
                    "Expected project expression, found {t:?} at token {}",
                    self.pos
                ))
            }
        })
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

fn eval(expr: &Expr, variables: &BTreeMap<String, ProjectValue>) -> Result<ProjectValue> {
    use ProjectValue as V;
    Ok(match expr {
        Expr::Value(v) => v.clone(),
        Expr::Variable(n) => {
            variables.get(n).cloned().ok_or_else(|| format!("Unknown project variable {n}"))?
        }
        Expr::List(xs) => V::List(xs.iter().map(|x| eval(x, variables)).collect::<Result<_>>()?),
        Expr::If(c, a, b) => match eval(c, variables)? {
            V::Bool(true) => eval(a, variables)?,
            V::Bool(false) => eval(b, variables)?,
            _ => return Err("Project condition must be boolean".into()),
        },
        Expr::Binary(a, op, b) => match (eval(a, variables)?, op.as_str(), eval(b, variables)?) {
            (V::String(a), "/", V::String(b)) => V::String(format!("{a}/{b}")),
            (V::String(a), "/", V::Virtual(b)) => V::Virtual(format!("{a}/{b}")),
            (V::Virtual(a), "==", V::Virtual(b)) => V::Bool(a == b),
            (V::Virtual(a), "!=", V::Virtual(b)) => V::Bool(a != b),
            (V::String(a), "==", V::String(b)) => V::Bool(a == b),
            (V::String(a), "!=", V::String(b)) => V::Bool(a != b),
            (V::Bool(a), "==", V::Bool(b)) => V::Bool(a == b),
            (V::Bool(a), "!=", V::Bool(b)) => V::Bool(a != b),
            _ => return Err(format!("Unsupported project operation or operand types: {op}")),
        },
        Expr::Call(name, args) => {
            let args = args.iter().map(|e| eval(e, variables)).collect::<Result<Vec<_>>>()?;
            match (name.as_str(), args.as_slice()) {
                ("tree" | "only", [V::String(n)]) => {
                    V::Selector { name: n.clone(), tree: name == "tree" }
                }
                ("error", [V::String(message)]) => return Err(message.clone()),
                _ => return Err(format!("Invalid project function or arguments: {name}")),
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
                    _ => return Err("Expected a module selector".into()),
                };
                out.push(Edge { kind, name, tree });
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
                Decl::Variable(name, expr) => {
                    if !self.declared.insert(name.clone()) {
                        return Err(format!("Duplicate project variable {name}"));
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
                    let id = parent.ok_or("Files must belong to a module")?;
                    for expr in exprs {
                        for value in flatten(eval(expr, &self.variables)?) {
                            let path = match value {
                                ProjectValue::String(path) => PathBuf::from(path),
                                ProjectValue::Virtual(name) => self.virtual_files.get(&name)
                                    .cloned().ok_or_else(|| format!(
                                        "Virtual file {name} is not registered; supply its generated source through workspace.generatedFiles"
                                    ))?,
                                _ => return Err("Expected a project file path".into()),
                            };
                            self.modules[id].files.push(path);
                        }
                    }
                }
                Decl::Module(name, body) => {
                    let id = self.modules.len();
                    if self.names.insert(name.clone(), id).is_some() {
                        return Err(format!("Duplicate module {name}"));
                    }
                    self.modules.push(ResolvedModule {
                        name: name.clone(),
                        parent,
                        files: Vec::new(),
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
                    format!(
                        "Module {} requires unknown module {}",
                        self.modules[id].name, edge.name
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
                    return Err(format!("Cyclic project modules: {}", cycle.join(" -> ")));
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
                let id =
                    *self.names.get(name).ok_or_else(|| format!("Unknown root module {name}"))?;
                descendants(id, &children, &mut active);
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
    #[test]
    fn real_riscv_project_selects_one_jalr_variant() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../sail-riscv/model/riscv.sail_project");
        let Ok(source) = std::fs::read_to_string(path) else { return };
        let p = resolve(&source, ProjectOptions::default());
        let paths = files(&p);
        assert!(paths.iter().any(|p| p.ends_with("jalr_seq.sail")));
        assert!(!paths.iter().any(|p| p.ends_with("jalr_rmem.sail")));
        assert!(paths.len() > 150, "{} files", paths.len());
    }

    #[test]
    #[ignore = "requires sail on PATH and the sibling sail-riscv checkout"]
    fn file_order_matches_sail_compiler_for_real_project() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../sail-riscv/model/riscv.sail_project")
            .canonicalize()
            .unwrap();
        let manifest = ProjectManifest::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for rmem in [false, true] {
            let output = std::process::Command::new("sail")
                .arg("--project")
                .arg(&path)
                .arg("--all-modules")
                .arg("--variable")
                .arg(format!("RMEM={rmem}"))
                .arg("--list-files")
                .output()
                .expect("run Sail compiler");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let expected: Vec<PathBuf> = String::from_utf8(output.stdout)
                .unwrap()
                .split_whitespace()
                .map(PathBuf::from)
                .collect();
            let options = ProjectOptions {
                variables: BTreeMap::from([("RMEM".into(), ProjectValue::Bool(rmem))]),
                ..Default::default()
            };
            let actual: Vec<_> = manifest
                .resolve(&options)
                .unwrap()
                .files
                .into_iter()
                .map(|p| path.parent().unwrap().join(p))
                .collect();
            assert_eq!(actual, expected, "RMEM={rmem}");
        }
    }
}
