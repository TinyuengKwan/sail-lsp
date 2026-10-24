//! `HirDisplay` implementations for hir-level public types.
//! Each public HIR type (Function, TypeDef, Register, Mapping,
//! etc.) implements `HirDisplay` so IDE features can render them
//! without reaching into lower layers.
//!
//! RA's `hir/src/display.rs` is ~960 lines with impls for Function,
//! Struct, Enum, Union, Field, EnumVariant, Trait, TypeAlias, etc.
//! Sail's version covers the types that exist in our hir.

use std::fmt::Write as _;

pub use hir_ty::display::{HirDisplay, HirDisplayError, HirFormatter};

use crate::{
    Def, Enum, File, Function, Local, Mapping, PathResolution, Register, Struct, TypeDef,
    TypeDefKind, TypeVar, Union,
};

/// RA renders: `pub fn name(params) -> ret_ty`.
/// ItemTree signatures already include the declaration keyword and name.
impl HirDisplay for Function {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{sig}")?;
        } else {
            write!(f, "function {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for TypeDef {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let keyword = match self.kind {
            TypeDefKind::Struct => "struct",
            TypeDefKind::Union => "union",
            TypeDefKind::Enum => "enum",
            TypeDefKind::Bitfield => "bitfield",
            TypeDefKind::Newtype => "newtype",
            TypeDefKind::TypeAlias => "type",
        };
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            // A type definition's signature is its whole declaration —
            // `enum Color = { Red, .. }`. Prepending the keyword and name gave
            // `enum Color = enum Color = { .. }`.
            write!(f, "{}", with_head(&sig, keyword, &name.to_string()))?;
        } else {
            write!(f, "{keyword} {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Struct {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{}", with_head(&sig, "struct", &name.to_string()))?;
        } else {
            write!(f, "struct {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Union {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{}", with_head(&sig, "union", &name.to_string()))?;
        } else {
            write!(f, "union {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Enum {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{}", with_head(&sig, "enum", &name.to_string()))?;
        } else {
            write!(f, "enum {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Register {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{sig}")?;
        } else {
            write!(f, "register {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Mapping {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let db = f.db;
        let name = self.name(db);
        if let Some(sig) = self.id.lookup_signature(db) {
            write!(f, "{sig}")?;
        } else {
            write!(f, "mapping {name}")?;
        }
        Ok(())
    }
}

impl HirDisplay for Def {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        match self {
            Def::Function(it) => return it.hir_fmt(f),
            Def::TypeDef(it) => return it.hir_fmt(f),
            Def::Register(it) => return it.hir_fmt(f),
            Def::Mapping(it) => return it.hir_fmt(f),
            _ => {}
        }
        // Every other form is written as `<keyword> <name>`, so that is
        // how it reads back.
        let name = self.location().lookup_name(f.db);
        match self.kind().keyword() {
            Some(kw) => write!(f, "{kw} {name}")?,
            None => write!(f, "{name}")?,
        }
        Ok(())
    }
}

/// `parameter x` or `let x`, as the thing reads in Sail.
///
/// This rendered the bare name, which is less than the per-file scrape's
/// label said — and that is what stopped `ide/src/hover` from building
/// its header through `HirDisplay`.
/// A signature that already opens with its own keyword is the whole
/// declaration; anything else needs the head put back in front of it.
fn with_head(sig: &str, keyword: &str, name: &str) -> String {
    if sig.trim_start().starts_with(keyword) {
        sig.trim().to_owned()
    } else {
        format!("{keyword} {name} = {sig}")
    }
}

impl HirDisplay for Local {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        let keyword = if self.is_param { "parameter" } else { "let" };
        write!(f, "{keyword} {}", self.name)?;
        Ok(())
    }
}

impl HirDisplay for TypeVar {
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
            PathResolution::TypeVar(tp) => tp.hir_fmt(f),
        }
    }
}

/// In Sail, each file is one module. Render as `module`.
impl HirDisplay for File {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "module")?;
        Ok(())
    }
}

// Re-export HirDisplay from hir-ty so consumers can use `hir::HirDisplay`.
pub use hir_ty::display::HirDisplayWrapper;
