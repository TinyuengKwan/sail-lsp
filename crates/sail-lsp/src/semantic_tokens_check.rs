//! What the `semanticTokens.*` settings change on the wire.
//!
//! The ide says what each range is; `to_proto::semantic_tokens` decides
//! what of that a client is sent, by the settings. Each toggle has a test
//! that fails without it, and one request goes through the real server to
//! show the settings reach the conversion.

use ide::syntax_highlighting::{Highlight, HlMod, HlRange, HlRanges, HlTag};

use crate::config::SemanticTokensConfig;
use crate::lsp::semantic_tokens::{self, SUPPORTED_MODIFIERS, SUPPORTED_TYPES};
use crate::to_proto;

fn ranges(highlights: &[Highlight]) -> HlRanges {
    let data = highlights
        .iter()
        .enumerate()
        .map(|(i, &highlight)| HlRange { line: 0, col: i as u32 * 2, length: 1, highlight })
        .collect();
    HlRanges { data }
}

fn types(config: &SemanticTokensConfig, highlights: &[Highlight]) -> Vec<&'static str> {
    to_proto::semantic_tokens(&ranges(highlights), config)
        .iter()
        .map(|t| SUPPORTED_TYPES[t.token_type as usize].as_str())
        .collect()
}

#[test]
fn strings_toggle_drops_string_literals_and_their_escapes() {
    let highlights =
        [HlTag::StringLiteral.into(), HlTag::EscapeSequence.into(), HlTag::Keyword.into()];
    let on = SemanticTokensConfig::default();
    assert_eq!(types(&on, &highlights), ["string", "escapeSequence", "keyword"]);
    let off = SemanticTokensConfig { strings: false, ..on };
    assert_eq!(types(&off, &highlights), ["keyword"]);
}

#[test]
fn operators_toggle_drops_operators() {
    let highlights = [HlTag::Operator.into(), HlTag::Function.into()];
    let on = SemanticTokensConfig::default();
    assert_eq!(types(&on, &highlights), ["operator", "function"]);
    let off = SemanticTokensConfig { operators: false, ..on };
    assert_eq!(types(&off, &highlights), ["function"]);
}

#[test]
fn punctuation_is_off_by_default_and_a_toggle_sends_it() {
    let highlights = [HlTag::Punctuation.into(), HlTag::Variable.into()];
    let default = SemanticTokensConfig::default();
    assert_eq!(types(&default, &highlights), ["variable"]);
    let on = SemanticTokensConfig { punctuation: true, ..default };
    assert_eq!(types(&on, &highlights), ["punctuation", "variable"]);
}

/// A client that declines non-standard tokens is sent each custom type's
/// standard superType — the one package.json declares — or nothing, and
/// no custom modifier.
#[test]
fn non_standard_tokens_off_falls_back_to_the_standard_legend() {
    let highlights = [
        HlTag::Register | HlMod::Static | HlMod::Mutable,
        HlTag::BuiltinType.into(),
        HlTag::Attribute.into(),
        HlTag::EscapeSequence.into(),
        HlTag::Punctuation.into(),
        HlTag::Keyword | HlMod::ControlFlow,
    ];
    let config = SemanticTokensConfig { punctuation: true, ..Default::default() };
    assert_eq!(
        types(&config, &highlights),
        ["register", "builtinType", "attribute", "escapeSequence", "punctuation", "keyword"]
    );
    let standard = SemanticTokensConfig { non_standard_tokens: false, ..config };
    let tokens = to_proto::semantic_tokens(&ranges(&highlights), &standard);
    let sent: Vec<&str> =
        tokens.iter().map(|t| SUPPORTED_TYPES[t.token_type as usize].as_str()).collect();
    assert_eq!(
        sent,
        ["variable", "type", "decorator", "string", "keyword"],
        "punctuation has no fallback"
    );
    let modifiers = |token: &lsp_types::SemanticToken| -> Vec<&str> {
        SUPPORTED_MODIFIERS
            .iter()
            .enumerate()
            .filter(|(i, _)| token.token_modifiers_bitset & (1 << i) != 0)
            .map(|(_, m)| m.as_str())
            .collect()
    };
    assert_eq!(modifiers(&tokens[0]), ["static"], "mutable is this server's own");
    assert_eq!(modifiers(&tokens[4]), Vec::<&str>::new(), "controlFlow is this server's own");
    // Every custom type the legend declares has a fallback decision, and
    // every standard one stands for itself.
    for ty in SUPPORTED_TYPES {
        let fallback = semantic_tokens::standard_fallback_type(ty.clone());
        assert!(
            fallback.is_none_or(|f| SUPPORTED_TYPES[..SUPPORTED_TYPES.len() - 5].contains(&f)),
            "{}: falls back to a non-standard type",
            ty.as_str()
        );
    }
}

/// The settings reach the conversion: through the real server, a register
/// is `register` by default and `variable` once non-standard tokens are
/// declined.
#[test]
fn the_server_honours_non_standard_tokens() {
    let source = "register R : int\nfunction f() -> int = R\n";
    let mut server =
        crate::server_check::ServerFixture::new_project("tokens", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let params = serde_json::json!({ "textDocument": { "uri": server.url("a.sail") } });
    let type_of_r = |server: &mut crate::server_check::ServerFixture| {
        let tokens = server.ask("textDocument/semanticTokens/full", params.clone());
        let data = tokens["data"].as_array().unwrap();
        // The second token on line 0 is `R` (after `register`).
        let r = &data[5..10];
        assert_eq!(r[0], 0, "{tokens}");
        SUPPORTED_TYPES[r[3].as_u64().unwrap() as usize].as_str().to_owned()
    };
    assert_eq!(type_of_r(&mut server), "register");
    server.configure(serde_json::json!({"semanticTokens": {"nonStandardTokens": false}}));
    assert_eq!(type_of_r(&mut server), "variable");
}
