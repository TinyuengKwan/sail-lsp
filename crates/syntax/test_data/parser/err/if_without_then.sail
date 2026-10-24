// Upstream requires `then`: `If_ exp Then exp` (parser.mly:663-673), and
// `sail --just-check` calls this a Syntax error at the token that should
// have been `then`. The parser stays lenient — a half-typed `if` must still
// give the IDE a tree — and `syntax::validation` reports it, which is
// rust-analyzer's division of labour.
function t() = { if c { g(1) } }
