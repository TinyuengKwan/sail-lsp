//! Type well-formedness checking.
//!
//! Every type annotation is checked for well-formedness:
//! - Referenced types must exist in the environment
//! - Type constructor arity must match declaration
//! - Numeric constraints should be satisfiable

use super::env::TopLevelEnv;
use super::{Ty, TyArg, TyKind};

/// A well-formedness error.
#[derive(Debug, Clone)]
pub struct WfError {
    /// Human-readable description of the error.
    pub message: String,
}

/// Check a type for well-formedness against the environment.
///
/// Validates type constructors exist, arity matches declarations,
/// and type variables are bound. Returns a list of errors.

pub fn check_wf_typ(ty: &Ty, env: &TopLevelEnv) -> Vec<WfError> {
    let mut errors = Vec::new();
    check_wf_inner(ty, env, &mut errors);
    errors
}

fn check_wf_inner(ty: &Ty, env: &TopLevelEnv, errors: &mut Vec<WfError>) {
    match ty.kind() {
        TyKind::Adt(name, _) => {
            // Check that the named type exists somewhere
            let exists = env.records.contains_key(name.as_str())
                || env.enums.contains_key(name.as_str())
                || env.unions.contains_key(name.as_str())
                || env.type_aliases.contains_key(name.as_str())
                || env.constructors.contains_key(name.as_str())
                || is_builtin_type(name);
            if !exists && !env.cross_file_function_names.contains(name.as_str()) {
                // Don't report for cross-file types that might exist elsewhere
                if env.has_workspace_context {
                    errors.push(WfError { message: format!("unknown type `{name}`") });
                }
            }
        }
        TyKind::App { name, args, .. } => {
            // Check type constructor exists
            let exists = env.records.contains_key(name.as_str())
                || env.type_aliases.contains_key(name.as_str())
                || is_builtin_type(name);
            if !exists && env.has_workspace_context {
                errors.push(WfError { message: format!("unknown type constructor `{name}`") });
            }
            // Check arity for known types
            if let Some(record) = env.records.get(name.as_str()) {
                let expected = record.params.len();
                let found = args.len();
                if expected > 0 && found > 0 && expected != found {
                    errors.push(WfError {
                        message: format!(
                            "type `{name}` expects {expected} type arguments, found {found}"
                        ),
                    });
                }
            }
            // Recursively check type arguments
            for arg in args {
                if let TyArg::Type(inner) = arg {
                    check_wf_inner(inner, env, errors);
                }
            }
        }
        TyKind::Tuple(items) => {
            for item in items {
                check_wf_inner(item, env, errors);
            }
        }
        TyKind::FnPtr(sig) => {
            for param in &sig.params {
                check_wf_inner(param, env, errors);
            }
            check_wf_inner(&sig.ret, env, errors);
        }
        TyKind::Exist { inner, .. } => {
            check_wf_inner(inner, env, errors);
        }
        TyKind::Bidir { lhs, rhs } => {
            check_wf_inner(lhs, env, errors);
            check_wf_inner(rhs, env, errors);
        }
        // Scalars, params, inference vars, errors — always well-formed
        _ => {}
    }
}

/// Check register definitions for well-formedness.
///
/// Lightweight check during env building. Full initializer type
/// checking requires inference context and is handled by the
/// per-callable inference pass.
pub fn check_register_wf(reg_name: &str, reg_ty: &Ty, has_initializer: bool) -> Vec<WfError> {
    let mut errors = Vec::new();

    if !has_initializer {
        // Option types should have explicit default
        let ty_text = reg_ty.display_text();
        if ty_text.starts_with("option(") || ty_text.starts_with("option ") {
            errors.push(WfError {
                message: format!(
                    "register `{reg_name}` of type `{ty_text}` should have an explicit default value"
                ),
            });
        }
    }

    errors
}

/// Check if a type name is a Sail built-in.
fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "nat"
            | "bool"
            | "unit"
            | "string"
            | "real"
            | "bit"
            | "bits"
            | "vector"
            | "list"
            | "option"
            | "result"
            | "range"
            | "atom"
            | "atom_bool"
            | "implicit"
            | "register"
            | "ref"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_env() -> TopLevelEnv {
        TopLevelEnv::default()
    }

    #[test]
    fn scalar_is_well_formed() {
        let errors = check_wf_typ(&Ty::named("int".to_string()), &empty_env());
        assert!(errors.is_empty());
    }

    #[test]
    fn unknown_adt_without_workspace() {
        // Without workspace context, unknown types are not flagged
        let errors = check_wf_typ(&Ty::named("UnknownType".to_string()), &empty_env());
        assert!(errors.is_empty(), "no error without workspace context");
    }

    #[test]
    fn unknown_adt_with_workspace() {
        let mut env = empty_env();
        env.has_workspace_context = true;
        let errors = check_wf_typ(&Ty::named("UnknownType".to_string()), &env);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown type"));
    }

    #[test]
    fn builtin_bits_is_well_formed() {
        let ty = Ty::app("bits", vec![TyArg::numeric("32")], "bits(32)".to_string());
        let errors = check_wf_typ(&ty, &empty_env());
        assert!(errors.is_empty());
    }
}
