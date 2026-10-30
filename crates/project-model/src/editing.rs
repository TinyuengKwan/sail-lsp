//! What an editor needs from a manifest's text: what the cursor is on, what
//! may be typed there, and how to spell a path. Token-level, so it keeps
//! working while the manifest is half-written and does not evaluate.
use std::ops::Range;

use super::{lex, Tok, Token, RESERVED};

/// What a position in a manifest refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestSymbol {
    /// A module named where a module is expected: `requires A`, `only(A)`.
    Module { name: String, range: Range<usize> },
    /// A file path as written, relative to the manifest's directory.
    File { path: String, range: Range<usize> },
    /// A `$variable`.
    Variable { name: String, range: Range<usize> },
}

/// What may be typed at a position, and the text it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestCompletion {
    Modules { replace: Range<usize>, names: Vec<String> },
    Variables { replace: Range<usize>, names: Vec<String> },
}

/// Module headers (`Name {`), in declaration order.
pub fn module_declarations(source: &str) -> Vec<(String, Range<usize>)> {
    let Ok(tokens) = lex(source) else { return Vec::new() };
    tokens
        .windows(2)
        .filter_map(|w| match (&w[0].tok, &w[1].tok) {
            (Tok::Id(name), Tok::LCurly) => Some((name.clone(), w[0].range.clone())),
            _ => None,
        })
        .collect()
}

/// `variable NAME` declarations, in order.
pub fn variable_declarations(source: &str) -> Vec<(String, Range<usize>)> {
    let Ok(tokens) = lex(source) else { return Vec::new() };
    tokens
        .windows(2)
        .filter_map(|w| match (&w[0].tok, &w[1].tok) {
            (Tok::KwVariable, Tok::Id(name)) => Some((name.clone(), w[1].range.clone())),
            _ => None,
        })
        .collect()
}

/// Whether the tokens before `end` put a module name there: after
/// `requires`/`after`/`before`/`implicit`, possibly past earlier names and
/// commas, or inside `only(`/`tree(`.
fn expects_module(tokens: &[Token], end: usize) -> bool {
    let mut i = end;
    while i > 0 {
        i -= 1;
        match &tokens[i].tok {
            Tok::KwRequires | Tok::KwAfter | Tok::KwBefore => return true,
            Tok::Id(word) if word == "implicit" => return true,
            Tok::LParen => {
                return i > 0
                    && matches!(&tokens[i - 1].tok, Tok::Id(f) if f == "only" || f == "tree")
            }
            // A name followed by `{` is the next module's header.
            Tok::Id(_) if tokens.get(i + 1).is_some_and(|t| t.tok == Tok::LCurly) => return false,
            Tok::Id(_) | Tok::Comma | Tok::RParen | Tok::LCurly => {}
            _ => return false,
        }
    }
    false
}

/// What the cursor at `offset` is on.
pub fn symbol_at(source: &str, offset: usize) -> Option<ManifestSymbol> {
    let tokens = lex(source).ok()?;
    let i = tokens.iter().position(|t| t.range.start <= offset && offset <= t.range.end)?;
    let token = &tokens[i];
    match &token.tok {
        Tok::Var(name) => {
            Some(ManifestSymbol::Variable { name: name.clone(), range: token.range.clone() })
        }
        Tok::Id(name)
            if tokens.get(i + 1).is_none_or(|t| t.tok != Tok::LCurly)
                && expects_module(&tokens, i) =>
        {
            Some(ManifestSymbol::Module { name: name.clone(), range: token.range.clone() })
        }
        Tok::Str(path) if path.contains('.') => {
            Some(ManifestSymbol::File { path: path.clone(), range: token.range.clone() })
        }
        Tok::Id(_) | Tok::DotDot | Tok::FileId(_) => {
            // `dir/sub/name.sail` is several tokens joined by `/`.
            let piece = |t: &Token| matches!(t.tok, Tok::Id(_) | Tok::DotDot);
            let mut start = i;
            while start >= 2 && tokens[start - 1].tok == Tok::Slash && piece(&tokens[start - 2]) {
                start -= 2;
            }
            let mut end = i;
            while matches!(tokens.get(end).map(|t| &t.tok), Some(Tok::Id(_) | Tok::DotDot))
                && tokens.get(end + 1).is_some_and(|t| t.tok == Tok::Slash)
            {
                end += 2;
            }
            let Some(Tok::FileId(_)) = tokens.get(end).map(|t| &t.tok) else { return None };
            if start > 0 && matches!(&tokens[start - 1].tok, Tok::Id(v) if v == "__virtual") {
                return None;
            }
            let range = tokens[start].range.start..tokens[end].range.end;
            Some(ManifestSymbol::File { path: source[range.clone()].to_owned(), range })
        }
        _ => None,
    }
}

/// What may be typed at `offset`.
pub fn completions_at(source: &str, offset: usize) -> Option<ManifestCompletion> {
    let before = source.get(..offset)?;
    let variables = || {
        let mut found = variable_declarations(source);
        if found.is_empty() {
            // The whole text may not lex yet (a lone `$`); what precedes it does.
            found = variable_declarations(before.trim_end_matches('$'));
        }
        let mut names: Vec<_> = found.into_iter().map(|(name, _)| name).collect();
        names.dedup();
        names
    };
    // A lone `$` does not lex; the name typed after one does.
    if before.ends_with('$') {
        return Some(ManifestCompletion::Variables { replace: offset..offset, names: variables() });
    }
    // Only the text before the cursor decides the context, so a broken
    // remainder of the manifest does not stop completion.
    let tokens = lex(before).ok()?;
    let (end, replace) = match tokens.last() {
        Some(t) if t.range.end == offset => match &t.tok {
            Tok::Var(_) => {
                let replace = t.range.start + 1..offset;
                return Some(ManifestCompletion::Variables { replace, names: variables() });
            }
            Tok::Id(_) => (tokens.len() - 1, t.range.clone()),
            _ => (tokens.len(), offset..offset),
        },
        _ => (tokens.len(), offset..offset),
    };
    if !expects_module(&tokens, end) {
        return None;
    }
    let mut names: Vec<_> = module_declarations(source).into_iter().map(|(name, _)| name).collect();
    if names.is_empty() {
        names = module_declarations(before).into_iter().map(|(name, _)| name).collect();
    }
    // A module requiring itself or a module it is inside is a cycle or
    // already implied.
    let inside = enclosing_modules(&tokens[..end]);
    names.retain(|name| !inside.contains(name));
    Some(ManifestCompletion::Modules { replace, names })
}

/// The modules whose bodies are open at the end of `tokens`.
fn enclosing_modules(tokens: &[Token]) -> Vec<String> {
    let mut open: Vec<Option<String>> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match &token.tok {
            Tok::LCurly => open.push(match i.checked_sub(1).map(|p| &tokens[p].tok) {
                Some(Tok::Id(name)) => Some(name.clone()),
                _ => None,
            }),
            Tok::RCurly => drop(open.pop()),
            _ => {}
        }
    }
    open.into_iter().flatten().collect()
}

/// `path` as a manifest spells it: bare when Sail's project lexer reads it
/// back as the same path (`dir/sub/name.sail`), quoted otherwise.
pub fn path_literal(path: &str) -> String {
    let ident = |s: &str| {
        s.chars().next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
            && s.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    };
    let word = |s: &str| s.chars().all(|c| c == '_' || c.is_ascii_alphanumeric());
    let segments: Vec<_> = path.split('/').collect();
    let (last, dirs) = segments.split_last().expect("split yields one segment");
    let bare_dir =
        |s: &&str| *s == ".." || (ident(s) && !RESERVED.contains(s) && super::keyword(s).is_none());
    let bare_file =
        last.rsplit_once('.').is_some_and(|(stem, ext)| word(stem) && !ext.is_empty() && word(ext));
    if bare_file && dirs.iter().all(bare_dir) {
        path.to_owned()
    } else {
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectManifest, ProjectOptions};

    /// `$0` marks the cursor.
    fn at(text: &str) -> (String, usize) {
        let offset = text.find("$0").unwrap();
        (text.replacen("$0", "", 1), offset)
    }

    #[test]
    fn the_cursor_names_a_module_file_or_variable() {
        let module = |text: &str| {
            let (text, offset) = at(text);
            match symbol_at(&text, offset) {
                Some(ManifestSymbol::Module { name, range }) => {
                    Some((name, text[range].to_owned()))
                }
                _ => None,
            }
        };
        assert_eq!(module("A {}\nB { requires A, C$0x }").unwrap().0, "Cx");
        assert_eq!(module("A {}\nB { after only(A$0) }").unwrap().0, "A");
        assert_eq!(module("L {}\nimplicit L$0\n").unwrap().0, "L");
        assert_eq!(module("A$0 { files a.sail }"), None, "a header declares, it does not refer");
        assert_eq!(module("A { requires B\n  C$0 { } }"), None, "the next header ends the list");

        let file = |text: &str| {
            let (text, offset) = at(text);
            match symbol_at(&text, offset) {
                Some(ManifestSymbol::File { path, .. }) => Some(path),
                _ => None,
            }
        };
        assert_eq!(file("M { files core/s$0ub/x.sail }").unwrap(), "core/sub/x.sail");
        assert_eq!(file("M { files core/sub/x.s$0ail }").unwrap(), "core/sub/x.sail");
        assert_eq!(file("M { files ../x$0.sail }").unwrap(), "../x.sail");
        assert_eq!(file("M { files \"a b$0.sail\" }").unwrap(), "a b.sail");
        assert_eq!(file("M { files __virtual g$0.sail }"), None);

        let (text, offset) = at("variable X = true\nM { files if $X$0 then a.sail else b.sail }");
        assert!(
            matches!(symbol_at(&text, offset), Some(ManifestSymbol::Variable { name, .. }) if name == "X")
        );
        assert_eq!(
            variable_declarations(&text)
                .iter()
                .map(|(n, r)| (n.as_str(), &text[r.clone()]))
                .collect::<Vec<_>>(),
            [("X", "X")]
        );
    }

    #[test]
    fn completion_offers_modules_where_a_module_goes() {
        let complete = |text: &str| {
            let (text, offset) = at(text);
            completions_at(&text, offset).map(|c| match c {
                ManifestCompletion::Modules { replace, names } => {
                    (text[replace].to_owned(), names.join(" "))
                }
                ManifestCompletion::Variables { replace, names } => {
                    (format!("${}", &text[replace]), names.join(" "))
                }
            })
        };
        let modules = "Lib { files lib.sail }\nCore { files core.sail }\n";
        let c = |s: &str| complete(&format!("{modules}{s}"));
        assert_eq!(c("Main { requires $0 }"), Some((String::new(), "Lib Core".into())));
        assert_eq!(c("Main { requires Lib, Co$0 }"), Some(("Co".into(), "Lib Core".into())));
        assert_eq!(
            c("Main { after tree(L$0"),
            Some(("L".into(), "Lib Core".into())),
            "a broken rest is fine"
        );
        assert_eq!(c("Main { files a$0"), None);
        let vars = "variable RMEM = false\nvariable ARCH = A64\n";
        assert_eq!(
            complete(&format!("{vars}M {{ files if $$0")),
            Some(("$".into(), "RMEM ARCH".into()))
        );
        assert_eq!(
            complete(&format!("{vars}M {{ files if $RM$0 then")),
            Some(("$RM".into(), "RMEM ARCH".into()))
        );
    }

    #[test]
    fn a_path_is_bare_only_when_it_reads_back_as_itself() {
        assert_eq!(path_literal("core/sub/x.sail"), "core/sub/x.sail");
        assert_eq!(path_literal("../x_2.sail"), "../x_2.sail");
        assert_eq!(path_literal("dir with space/x.sail"), "\"dir with space/x.sail\"");
        assert_eq!(path_literal("default/x.sail"), "\"default/x.sail\"", "a reserved word");
        assert_eq!(path_literal("files/x.sail"), "\"files/x.sail\"", "a keyword");
        assert_eq!(path_literal("a-b.sail"), "\"a-b.sail\"");
        assert_eq!(path_literal("q\"uote.sail"), "\"q\\\"uote.sail\"");
        for path in ["core/sub/x.sail", "dir with space/x.sail", "default/x.sail", "q\"uote.sail"] {
            let manifest = format!("M {{ files {} }}", path_literal(path));
            let project = ProjectManifest::parse(&manifest)
                .unwrap()
                .resolve(&ProjectOptions::default())
                .unwrap();
            assert_eq!(project.files, [std::path::PathBuf::from(path)], "{manifest}");
        }
    }

    #[test]
    fn a_new_file_goes_after_the_module_s_last_entry() {
        let insert = |text: &str, module: &str| {
            let manifest = ProjectManifest::parse(text).unwrap();
            let (at, new) = manifest.file_insertion(text, module, "new.sail").unwrap();
            format!("{}{new}{}", &text[..at], &text[at..])
        };
        assert_eq!(
            insert("A {\n  files a.sail, if $X then b.sail else c.sail\n  B { files b2.sail }\n}", "A"),
            "A {\n  files a.sail, if $X then b.sail else c.sail, new.sail\n  B { files b2.sail }\n}"
        );
        assert_eq!(
            insert("A {\n  B { files b2.sail }\n}", "B"),
            "A {\n  B { files b2.sail, new.sail }\n}"
        );
        assert_eq!(
            insert("A {\n  requires B\n}\nB {}", "A"),
            "A {\n  files new.sail\n  requires B\n}\nB {}"
        );
        assert_eq!(insert("A { files { a.sail } }", "A"), "A { files { a.sail, new.sail } }");
        assert!(ProjectManifest::parse("A {}")
            .unwrap()
            .file_insertion("A {}", "Nope", "x.sail")
            .is_none());
    }
}
