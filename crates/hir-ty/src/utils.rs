//! Type system utility functions.

use crate::ty::{Scalar, Ty, TyKind};

/// Check if a type is a numeric type (int, nat, or numeric application).
pub fn is_numeric(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Scalar(Scalar::Int | Scalar::Nat))
}

/// Check if a type is a boolean.
pub fn is_bool(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Scalar(Scalar::Bool))
}

/// Check if a type is a bitvector (bits(n)).
pub fn is_bitvector(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::App { name, .. } if name == "bits")
}

/// Check if a type is a function type.
pub fn is_function(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::FnPtr(_))
}

/// Extract the return type from a function type.
pub fn fn_return_type(ty: &Ty) -> Option<&Ty> {
    match ty.kind() {
        TyKind::FnPtr(sig) => Some(&sig.ret),
        _ => None,
    }
}

/// Extract parameter types from a function type.
pub fn fn_param_types(ty: &Ty) -> Option<&[Ty]> {
    match ty.kind() {
        TyKind::FnPtr(sig) => Some(&sig.params),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::TyArg;

    #[test]
    fn numeric_checks() {
        assert!(is_numeric(&Ty::scalar(Scalar::Int)));
        assert!(is_numeric(&Ty::scalar(Scalar::Nat)));
        assert!(!is_numeric(&Ty::scalar(Scalar::Bool)));
    }

    #[test]
    fn bool_check() {
        assert!(is_bool(&Ty::scalar(Scalar::Bool)));
        assert!(!is_bool(&Ty::scalar(Scalar::Int)));
    }

    #[test]
    fn bitvector_check() {
        let bv = Ty::app("bits", vec![TyArg::numeric("32")], "bits(32)");
        assert!(is_bitvector(&bv));
        assert!(!is_bitvector(&Ty::scalar(Scalar::Int)));
    }

    #[test]
    fn function_type_checks() {
        let fn_ty = Ty::function(vec![Ty::scalar(Scalar::Int)], Ty::scalar(Scalar::Bool));
        assert!(is_function(&fn_ty));
        assert!(!is_function(&Ty::scalar(Scalar::Int)));

        let ret = fn_return_type(&fn_ty);
        assert_eq!(ret, Some(&Ty::scalar(Scalar::Bool)));

        let params = fn_param_types(&fn_ty);
        assert_eq!(params, Some([Ty::scalar(Scalar::Int)].as_slice()));

        assert_eq!(fn_return_type(&Ty::scalar(Scalar::Int)), None);
        assert_eq!(fn_param_types(&Ty::scalar(Scalar::Int)), None);
    }
}
