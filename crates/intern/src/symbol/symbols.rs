//! Module defining all known symbols required by the rest of sail-lsp.
#![allow(non_upper_case_globals)]

use std::hash::{BuildHasher, BuildHasherDefault};

use dashmap::{DashMap, SharedValue};
use rustc_hash::FxHasher;

use crate::{symbol::TaggedArcPtr, Symbol};

macro_rules! define_symbols {
    (@WITH_NAME: $($alias:ident = $value:literal,)* @PLAIN: $($name:ident,)*) => {
        // The strings should be in `static`s so that symbol equality holds.
        $(
            pub const $name: Symbol = {
                static SYMBOL_STR: &str = stringify!($name);
                Symbol { repr: TaggedArcPtr::non_arc(&SYMBOL_STR) }
            };
        )*
        $(
            pub const $alias: Symbol = {
                static SYMBOL_STR: &str = $value;
                Symbol { repr: TaggedArcPtr::non_arc(&SYMBOL_STR) }
            };
        )*

        pub(super) fn prefill() -> DashMap<Symbol, (), BuildHasherDefault<FxHasher>> {
            let mut dashmap_ = <DashMap<Symbol, (), BuildHasherDefault<FxHasher>>>::with_hasher(BuildHasherDefault::default());

            let hasher_ = dashmap_.hasher().clone();
            let hash_one = |it_: &str| hasher_.hash_one(it_);
            {
                $(
                    let s = stringify!($name);
                    let hash_ = hash_one(s);
                    let shard_idx_ = dashmap_.determine_shard(hash_ as usize);
                    dashmap_.shards_mut()[shard_idx_].get_mut().insert(hash_, ($name, SharedValue::new(())), |(x, _)| hash_one(x.as_str()));
                )*
                $(
                    let s = $value;
                    let hash_ = hash_one(s);
                    let shard_idx_ = dashmap_.determine_shard(hash_ as usize);
                    dashmap_.shards_mut()[shard_idx_].get_mut().insert(hash_, ($alias, SharedValue::new(())), |(x, _)| hash_one(x.as_str()));
                )*
            }
            dashmap_
        }
    };
}

define_symbols! {
    @WITH_NAME:

    // Numeric integer constants (for Symbol::integer())
    INTEGER_0 = "0",
    INTEGER_1 = "1",
    INTEGER_2 = "2",
    INTEGER_3 = "3",
    INTEGER_4 = "4",
    INTEGER_5 = "5",
    INTEGER_6 = "6",
    INTEGER_7 = "7",
    INTEGER_8 = "8",
    INTEGER_9 = "9",
    INTEGER_10 = "10",
    INTEGER_11 = "11",
    INTEGER_12 = "12",
    INTEGER_13 = "13",
    INTEGER_14 = "14",
    INTEGER_15 = "15",

    // Special symbols
    __empty = "",
    MISSING_NAME = "[missing name]",
    underscore = "_",
    true_ = "true",
    false_ = "false",

    // Sail keywords that need aliasing (reserved in Rust)
    val_ = "val",
    type_ = "type",
    let_ = "let",
    if_ = "if",
    else_ = "else",
    match_ = "match",
    in_ = "in",
    return_ = "return",
    struct_ = "struct",
    enum_ = "enum",
    union_ = "union",
    ref_ = "ref",
    end_ = "end",
    assert_ = "assert",

    // Sail-specific type names
    bits_ = "bits",
    int_ = "int",
    nat_ = "nat",
    bool_ = "bool",
    unit_ = "unit",
    string_ = "string",
    real_ = "real",
    bit_ = "bit",
    range_ = "range",
    atom_ = "atom",
    vector_ = "vector",
    list_ = "list",
    option_ = "option",
    result_ = "result",

    @PLAIN:

    // Sail keywords (not reserved in Rust, can be used directly)
    function,
    clause,
    register,
    bitfield,
    mapping,
    scattered,
    overload,
    operator,
    default,
    effect,
    pure,
    monadic,
    cast,
    sizeof,
    constraint,
    forall,
    exist,
    throw,
    exit,
    foreach,
    from,
    to,
    by,
    while_,
    do_,
    repeat,
    until,
    with,
    newtype,
    outcome,
    instantiation,
    impl_,
    forwards,
    backwards,
    bidir,
    undefined,
    complete,
    incomplete,
    barr,
    depend,
    rreg,
    wreg,
    rmem,
    wmem,
    wmv,
    eamem,
    exmem,
    undef,
    unspec,
    nondet,
    escape,
    configuration,
    termination_measure,
    dec,
    inc,
    Order,
    Type,
    Int,
    Bool,
}
