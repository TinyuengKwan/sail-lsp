//! Snippet definition types.
//! Provides structured snippet definitions with scope awareness.
//! Snippets use VSCode snippet syntax: `$1`, `${2:default}`, `$0` (final cursor).

/// A code snippet template.
#[derive(Debug, Clone)]
pub struct Snippet {
    /// Trigger prefix (e.g., "fn", "match", "bf").
    pub prefix: &'static str,
    /// Snippet body with tab stops (VSCode snippet syntax).
    pub body: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Where this snippet is applicable.
    pub scope: SnippetScope,
}

/// Where a snippet can be triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetScope {
    /// Top-level item position (function, type, register, etc.)
    Item,
    /// Expression position.
    Expr,
    /// Type position.
    Type,
}

/// Built-in Sail snippet templates.
///
/// Ordered by expected usage frequency. Top-level items first,
/// then expression-level patterns.
pub static SAIL_SNIPPETS: &[Snippet] = &[
    Snippet {
        prefix: "funcdecl",
        body: "val ${1:name} : (${2:args}) -> ${3:result}\nfunction ${1:name}(${4:params}) = {\n\t$0\n}",
        description: "Function declaration + definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "fn",
        body: "function ${1:name}(${2:args}) = ${0:body}",
        description: "Function definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "scattered",
        body: "scattered function ${1:name}\n\nclause ${1:name}(${2:pat}) = $0\n\nend ${1:name}",
        description: "Scattered function definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "enumdef",
        body: "enum ${1:Name} = {\n\t${2:A},\n\t${3:B}\n}",
        description: "Enum definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "structdef",
        body: "struct ${1:Name} = {\n\t${2:field} : ${3:type}\n}",
        description: "Struct definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "uniondef",
        body: "union ${1:Name} = {\n\t${2:Variant} : ${3:type}\n}",
        description: "Union definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "bitfield",
        body: "bitfield ${1:Name} : bits(${2:32}) = {\n\t${3:field} : ${4:7 .. 0}\n}",
        description: "Bitfield definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "register",
        body: "register ${1:name} : ${2:type}",
        description: "Register definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "mapping",
        body: "mapping ${1:name} : ${2:type1} <-> ${3:type2} = {\n\t${4:pat1} <-> ${5:pat2}\n}",
        description: "Mapping definition",
        scope: SnippetScope::Item,
    },
    Snippet {
        prefix: "match",
        body: "match ${1:expr} {\n\t${2:pattern} => ${0:body}\n}",
        description: "Match expression",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "if",
        body: "if ${1:cond} then ${2:then_expr} else ${0:else_expr}",
        description: "If-then-else expression",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "foreach",
        body: "foreach (${1:i} from ${2:0} to ${3:n}) {\n\t$0\n}",
        description: "Foreach loop",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "tryc",
        body: "try {\n\t$0\n} catch {\n\t${1:_} => ()\n}",
        description: "Try-catch block",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "matchopt",
        body: "match ${1:x} {\n\tSome(${2:v}) => $3,\n\tNone() => $0\n}",
        description: "Match on option",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "let",
        body: "let ${1:name} = ${2:value} in ${0:body}",
        description: "Let binding",
        scope: SnippetScope::Expr,
    },
    Snippet {
        prefix: "bits",
        body: "bits(${1:width})",
        description: "Bitvector type",
        scope: SnippetScope::Type,
    },
    Snippet {
        prefix: "vector",
        body: "vector(${1:len}, ${2:order}, ${3:elem})",
        description: "Vector type",
        scope: SnippetScope::Type,
    },
];
