//! `HirDisplay` implementations for hir-level public types.
//! Each public HIR type (Function, Adt, Register, Mapping,
//! etc.) implements `HirDisplay` so IDE features can render them
//! without reaching into lower layers.
//!
//! RA's `hir/src/display.rs` is ~960 lines with impls for Function,
//! Struct, Enum, Union, Field, EnumVariant, Trait, TypeAlias, etc.
//! Sail's version covers the types that exist in our hir.

use std::fmt::Write as _;

use hir_ty::display::{HirDisplay, HirDisplayError, HirFormatter};

use crate::{
    Adt, AdtId, Enum, Function, Local, Mapping, Module, ModuleDef, PathResolution, Register,
    Struct, TypeDefKind, GenericParam, Union,
};

/// RA renders: `pub fn name(params) -> ret_ty`.
/// Sail renders: `val name : signature` (or `function name` as fallback).
impl HirDisplay for Function {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "val {name} : {sig}")?;
            } else {
                write!(f, "function {name}")?;
            }
        } else {
            write!(f, "function")?;
        }
        Ok(())
    }
}

impl HirDisplay for Adt {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let keyword = match self.kind {
            TypeDefKind::Struct => "struct",
            TypeDefKind::Union => "union",
            TypeDefKind::Enum => "enum",
            TypeDefKind::Bitfield => "bitfield",
            TypeDefKind::Newtype => "newtype",
            TypeDefKind::TypeAlias => "type",
        };
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "{keyword} {name} = {sig}")?;
            } else {
                write!(f, "{keyword} {name}")?;
            }
        } else {
            write!(f, "{keyword}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Struct {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "struct {name} = {sig}")?;
            } else {
                write!(f, "struct {name}")?;
            }
        } else {
            write!(f, "struct")?;
        }
        Ok(())
    }
}

impl HirDisplay for Union {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "union {name} = {sig}")?;
            } else {
                write!(f, "union {name}")?;
            }
        } else {
            write!(f, "union")?;
        }
        Ok(())
    }
}

impl HirDisplay for Enum {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "enum {name} = {sig}")?;
            } else {
                write!(f, "enum {name}")?;
            }
        } else {
            write!(f, "enum")?;
        }
        Ok(())
    }
}

impl HirDisplay for AdtId {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        match self {
            AdtId::Struct(s) => s.hir_fmt(f),
            AdtId::Union(u) => u.hir_fmt(f),
            AdtId::Enum(e) => e.hir_fmt(f),
        }
    }
}

impl HirDisplay for Register {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "register {name} : {sig}")?;
            } else {
                write!(f, "register {name}")?;
            }
        } else {
            write!(f, "register")?;
        }
        Ok(())
    }
}

impl HirDisplay for Mapping {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        if let Some(db) = f.db {
            let name = self.name(db);
            if let Some(sig) = self.id.lookup_signature(db) {
                write!(f, "mapping {name} : {sig}")?;
            } else {
                write!(f, "mapping {name}")?;
            }
        } else {
            write!(f, "mapping")?;
        }
        Ok(())
    }
}

impl HirDisplay for ModuleDef {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        match self {
            ModuleDef::Function(it) => it.hir_fmt(f),
            ModuleDef::TypeDef(it) => it.hir_fmt(f),
            ModuleDef::Register(it) => it.hir_fmt(f),
            ModuleDef::Mapping(it) => it.hir_fmt(f),
            ModuleDef::Let(loc) => {
                if let Some(db) = f.db {
                    let name = loc.lookup_name(db);
                    write!(f, "let {name}")?;
                } else {
                    write!(f, "let")?;
                }
                Ok(())
            }
            ModuleDef::Overload(loc) => {
                if let Some(db) = f.db {
                    let name = loc.lookup_name(db);
                    write!(f, "overload {name}")?;
                } else {
                    write!(f, "overload")?;
                }
                Ok(())
            }
        }
    }
}

impl HirDisplay for Local {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "{}", self.name)?;
        Ok(())
    }
}

impl HirDisplay for GenericParam {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "'{}", self.name)?;
        Ok(())
    }
}

impl HirDisplay for PathResolution {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        match self {
            PathResolution::Def(d) => d.hir_fmt(f),
            PathResolution::Local(l) => l.hir_fmt(f),
            PathResolution::TypeParam(tp) => tp.hir_fmt(f),
        }
    }
}

/// In Sail, each file is one module. Render as `module`.
impl HirDisplay for Module {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "module")?;
        Ok(())
    }
}

// Re-export HirDisplay from hir-ty so consumers can use `hir::HirDisplay`.
pub use hir_ty::display::HirDisplayWrapper;
