//! Recognised Sail directive / attribute names.
//!
//! Extracted so the completion engine can suggest pragma names
//! without depending on sail_server. Covers Sail's directive list
//! plus the small set of attribute names commonly seen on `$[ ... ]`
//! annotations.

pub const KNOWN_PRAGMAS: &[&str] = &[
    "define",
    "anchor",
    "span",
    "include",
    "include_error",
    "ifdef",
    "ifndef",
    "iftarget",
    "else",
    "endif",
    "option",
    "optimize",
    "latex",
    "property",
    "counterexample",
    "suppress_warnings",
    "include_start",
    "include_end",
    "sail_internal",
    "target_set",
    "non_exec",
    // Sail attributes also commonly used
    "no_enum_number_conversions",
    "undefined_gen",
    "incomplete",
    "no_warn",
    "deprecated",
    "fold",
    "complete",
    "format",
    "infallible",
    "pure",
];
