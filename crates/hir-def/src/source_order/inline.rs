//! Declaration contexts for an inline `$project#` program. This is a local
//! execution order, not an ordering of unrelated files in an ad-hoc workspace.
use super::{Declaration, FileText};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub requires: HashMap<String, Vec<String>>,
    modules: Vec<Option<String>>,
}

#[salsa::tracked(returns(ref))]
pub fn project(db: &dyn salsa::Database, file: FileText) -> Option<Project> {
    let root = syntax::parse_query::semantic_parse_file(db, file).syntax_node()?;
    let defs: Vec<_> = root.children().collect();
    let requires = parse_project_pragma(&defs)?;
    let mut module = None;
    let modules = defs
        .iter()
        .map(|node| {
            if node.kind() == syntax::SyntaxKind::DIRECTIVE_DEF {
                let text = node.text().to_string();
                if let Some(rest) = text.trim().strip_prefix("$start_module#") {
                    module = Some(rest.trim().to_owned());
                } else if text.trim().starts_with("$end_module#") {
                    module = None;
                }
            }
            module.clone()
        })
        .collect();
    Some(Project { requires, modules })
}

/// Scope is part of the semantic query key; byte offsets and body text are not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    File(base_db::FileId),
    Inline { file: FileText, module: Option<String> },
}

pub fn scope(db: &dyn salsa::Database, at: Declaration) -> Scope {
    // The configured expansion graph is authoritative when present. Inline
    // pragmas supply a program only for an otherwise unconfigured buffer.
    if base_db::preprocess::WorkspaceSourceProject::try_get(db).is_none() {
        if let Some(project) = project(db, at.file) {
            return Scope::Inline {
                file: at.file,
                module: project.modules.get(at.index).cloned().flatten(),
            };
        }
    }
    let file = at.file.file_id(db);
    base_db::module_graph::file_scope(db, file).map_or(Scope::Global, |_| Scope::File(file))
}

impl Scope {
    pub fn is_global(&self) -> bool {
        matches!(self, Self::Global | Self::Inline { module: None, .. })
    }

    /// Inline scope checks use declaration ownership, even for two declarations
    /// in the same physical file. None means this is a configured file scope.
    pub fn inline_visible(
        &self,
        db: &dyn salsa::Database,
        owner: Declaration,
        private: bool,
        global: bool,
    ) -> Option<bool> {
        let Self::Inline { file, module: user } = self else { return None };
        if owner.file != *file {
            return None;
        }
        let project = project(db, *file).as_ref()?;
        let owner = if global { None } else { project.modules.get(owner.index).cloned().flatten() };
        Some(if private {
            owner == *user
        } else {
            owner.as_ref().is_none_or(|owner| {
                user.as_ref().is_some_and(|user| {
                    user == owner
                        || project.requires.get(user).is_some_and(|deps| deps.contains(owner))
                })
            })
        })
    }
}

/// Shared declaration visibility for bindings, scattered state and consumers
/// which must preserve the owner independently of the physical file.
pub fn declaration_visible(
    db: &dyn salsa::Database,
    from: Declaration,
    owner: Declaration,
    private: bool,
    global: bool,
) -> bool {
    if let Some(visible) = scope(db, from).inline_visible(db, owner, private, global) {
        return visible;
    }
    let from = from.file.file_id(db);
    let owner = owner.file.file_id(db);
    if global {
        !private || base_db::module_graph::file_scope(db, from).is_none()
    } else {
        base_db::module_graph::can_reference_file(db, from, owner)
            && (!private
                || base_db::module_graph::file_scope(db, from).is_none()
                || base_db::module_graph::same_module(db, from, owner))
    }
}

#[salsa::tracked(returns(ref))]
pub(super) fn declarations(db: &dyn salsa::Database, file: FileText) -> Vec<Declaration> {
    syntax::parse_query::semantic_parse_file(db, file)
        .syntax_node()
        .into_iter()
        .flat_map(|root| {
            root.children()
                .enumerate()
                .map(|(index, _)| Declaration { file, index })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Parse `$project#` into a module -> requires map.
///
/// Handles the two forms the module pragma uses: module declarations
/// `Name { requires A, B }` and variable bindings
/// `variable mods = [A, B]`, whose value can be referenced as `$mods`
/// inside a `requires` clause (project.ml).
pub fn parse_project_pragma(defs: &[syntax::SyntaxNode]) -> Option<HashMap<String, Vec<String>>> {
    use parser::SyntaxKind as SK;
    let mut modules: HashMap<String, Vec<String>> = HashMap::new();
    let mut variables: HashMap<String, Vec<String>> = HashMap::new();
    let mut found = false;

    for def in defs.iter().filter(|d| d.kind() == SK::DIRECTIVE_DEF) {
        let text = def.text().to_string();
        let text = text.trim();
        let Some(body) = text.strip_prefix("$project#") else { continue };
        found = true;

        let toks = project_tokens(body);
        let mut i = 0;
        while i < toks.len() {
            if toks[i] == "variable" && i + 2 < toks.len() && toks[i + 2] == "=" {
                let name = toks[i + 1].clone();
                if toks.get(i + 3).map(String::as_str) == Some("[") {
                    let (items, next) = project_bracket_list(&toks, i + 3);
                    variables.insert(name, items);
                    i = next;
                } else {
                    // `variable b = false` — a scalar binding.
                    variables.insert(name, toks.get(i + 3).cloned().into_iter().collect());
                    i += 4;
                }
                continue;
            }
            let name = toks[i].clone();
            i += 1;
            let mut deps = Vec::new();
            if i < toks.len() && toks[i] == "{" {
                i += 1;
                while i < toks.len() && toks[i] != "}" {
                    if toks[i] == "requires" {
                        i += 1;
                        // `requires if $b then X else Y` — pick the branch the
                        // variable selects (project.ml). A variable bound to
                        // `false` has no items, so `$b` being unset or empty
                        // means the else branch.
                        let mut skip_branch = false;
                        while i < toks.len() && toks[i] != "}" {
                            let item = toks[i].clone();
                            i += 1;
                            if item == "," {
                                continue;
                            }
                            if item == "if" {
                                let cond = toks.get(i).cloned().unwrap_or_default();
                                i += 1;
                                let truthy = cond
                                    .strip_prefix('$')
                                    .and_then(|v| variables.get(v))
                                    .is_some_and(|items| {
                                        items.first().map(String::as_str) == Some("true")
                                    });
                                skip_branch = !truthy;
                                continue;
                            }
                            if item == "then" {
                                continue;
                            }
                            if item == "else" {
                                skip_branch = !skip_branch;
                                continue;
                            }
                            if skip_branch {
                                continue;
                            }
                            match item.strip_prefix('$') {
                                Some(var) => {
                                    deps.extend(variables.get(var).cloned().unwrap_or_default())
                                }
                                None => deps.push(item),
                            }
                        }
                    } else {
                        i += 1;
                    }
                }
                i += 1; // closing `}`
            }
            if !name.is_empty() {
                modules.insert(name, deps);
            }
        }
    }

    found.then_some(modules)
}

/// Split a `$project#` body into words and punctuation.
fn project_tokens(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in body.chars() {
        if ch.is_whitespace() || matches!(ch, '{' | '}' | '[' | ']' | ',' | '=') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if !ch.is_whitespace() {
                out.push(ch.to_string());
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Read `[A, B]` starting at `start`, returning the items and the index just
/// past the closing bracket.
fn project_bracket_list(toks: &[String], start: usize) -> (Vec<String>, usize) {
    let mut items = Vec::new();
    let mut i = start;
    if i < toks.len() && toks[i] == "[" {
        i += 1;
        while i < toks.len() && toks[i] != "]" {
            if toks[i] != "," {
                items.push(toks[i].clone());
            }
            i += 1;
        }
        i += 1;
    }
    (items, i)
}
