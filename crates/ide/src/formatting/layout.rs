//! Width-driven expression layout. Only token gaps are rewritten; comments,
//! literals, parentheses and operator order remain exact source tokens.
use super::FormatOptions;
use std::collections::BTreeMap;
use syntax::{SyntaxKind as K, SyntaxNode, SyntaxToken};

pub(super) fn expressions(text: &str, options: &FormatOptions) -> String {
    let (root, errors) = syntax::parse_text(text);
    if !errors.is_empty() {
        return text.into();
    }
    // Instruction mapping rows deliberately carry a hand-aligned layout.
    if root.children().any(|node| node.children_with_tokens().any(|e| e.kind() == K::KW_MAPPING)) {
        return text.into();
    }
    let mut writer = Writer {
        source: text,
        options,
        output: String::new(),
        pending: String::new(),
        breaks: BTreeMap::new(),
    };
    writer.node(&root);
    writer.output.push_str(&writer.pending);
    writer.output
}

struct Writer<'a> {
    source: &'a str,
    options: &'a FormatOptions,
    output: String,
    pending: String,
    breaks: BTreeMap<usize, String>,
}

fn first(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() != K::WHITESPACE)
}

impl Writer<'_> {
    fn width(&self, text: &str) -> usize {
        text.chars().map(|c| if c == '\t' { self.options.tab_spaces() } else { 1 }).sum()
    }
    fn break_before(&mut self, token: &SyntaxToken, indent: &str) {
        self.breaks.insert(usize::from(token.text_range().start()), format!("\n{indent}"));
    }
    fn node(&mut self, node: &SyntaxNode) {
        if let Some(initial) = first(node) {
            let whitespace = self
                .breaks
                .get(&usize::from(initial.text_range().start()))
                .unwrap_or(&self.pending);
            let prefix = format!("{}{}", self.output.rsplit('\n').next().unwrap_or(""), whitespace);
            let line = prefix.rsplit('\n').next().unwrap_or("");
            let indent: String = line.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();
            let unit = if self.options.hard_tabs() {
                "\t".to_owned()
            } else {
                " ".repeat(self.options.tab_spaces())
            };
            let inner = format!("{indent}{unit}");
            let source = self.source
                [usize::from(node.text_range().start())..usize::from(node.text_range().end())]
                .trim_end();
            // Existing multiline expressions retain the author's grouping.
            // Children still receive their actual, newly computed columns.
            if !source.contains(['\n', '\r'])
                && self.width(line) + self.width(source) > self.options.max_width()
            {
                match node.kind() {
                    K::CALL_EXPR => {
                        if let Some(args) = node.children().find(|n| n.kind() == K::ARG_LIST) {
                            for arg in args.children() {
                                if let Some(token) = first(&arg) {
                                    self.break_before(&token, &inner);
                                }
                            }
                            if let Some(close) = args
                                .children_with_tokens()
                                .filter_map(|e| e.into_token())
                                .find(|t| t.kind() == K::R_PAREN)
                            {
                                self.break_before(&close, &indent);
                            }
                        }
                    }
                    K::IF_EXPR => {
                        let mut branch = false;
                        for child in node.children_with_tokens() {
                            match child {
                                rowan::NodeOrToken::Token(t) if t.kind() == K::KW_ELSE => {
                                    self.break_before(&t, &indent);
                                    branch = true;
                                }
                                rowan::NodeOrToken::Token(t) if t.kind() == K::KW_THEN => {
                                    branch = true
                                }
                                rowan::NodeOrToken::Node(n) if branch => {
                                    if let Some(t) = first(&n) {
                                        self.break_before(&t, &inner);
                                    }
                                    branch = false;
                                }
                                _ => {}
                            }
                        }
                    }
                    K::MATCH_EXPR => {
                        for arm in node.children().filter(|n| n.kind() == K::MATCH_ARM) {
                            if let Some(t) = first(&arm) {
                                self.break_before(&t, &inner);
                            }
                        }
                        if let Some(close) = node
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .find(|t| t.kind() == K::R_CURLY)
                        {
                            self.break_before(&close, &indent);
                        }
                    }
                    K::MATCH_ARM => {
                        if let Some(body) = node.children().last() {
                            if let Some(t) = first(&body) {
                                self.break_before(&t, &inner);
                            }
                        }
                    }
                    K::BIN_EXPR => {
                        for op in node
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .filter(|t| matches!(t.kind(), K::AMP | K::PIPE))
                        {
                            self.break_before(&op, &inner);
                        }
                    }
                    _ => {}
                }
            }
        }
        for child in node.children_with_tokens() {
            match child {
                rowan::NodeOrToken::Node(n) => self.node(&n),
                rowan::NodeOrToken::Token(t) if t.kind() == K::WHITESPACE => {
                    self.pending.push_str(t.text())
                }
                rowan::NodeOrToken::Token(t) => {
                    if let Some(ws) = self.breaks.remove(&usize::from(t.text_range().start())) {
                        self.output.push_str(&ws);
                    } else {
                        self.output.push_str(&self.pending);
                    }
                    self.pending.clear();
                    self.output.push_str(t.text());
                }
            }
        }
    }
}
