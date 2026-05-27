//! Bitfield accessor materialization.
//!
//! Generates accessor functions for each bitfield definition:
//! - `Mk_T`: constructor `bits(N) -> T`
//! - `_get_T_field`: getter `T -> bits(width)`
//! - `_update_T_field`: updater `(T, bits(width)) -> T`
//! - `_set_T_field`: register setter `(register(T), bits(width)) -> unit`
//!
//! In the LSP, we "materialize" these as virtual `ItemTreeEntry` entries
//! in the ItemTree so they participate in name resolution, completion,
//! and hover without generating actual source code.

/// A parsed bitfield field: name + bit range.
#[derive(Debug, Clone)]
pub struct BitfieldField {
    pub name: String,
    pub hi: u32,
    pub lo: u32,
}

impl BitfieldField {
    /// Width of this field in bits.
    pub fn width(&self) -> u32 {
        self.hi.saturating_sub(self.lo) + 1
    }
}

/// Kind of generated bitfield accessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitfieldAccessorKind {
    /// `Mk_T : bits(N) -> T`
    Constructor,
    /// `_get_T_field : T -> bits(width)`
    Getter,
    /// `_update_T_field : (T, bits(width)) -> T`
    Updater,
    /// `_set_T_field : (register(T), bits(width)) -> unit`
    Setter,
}

/// Metadata for a generated bitfield accessor entry.
#[derive(Debug, Clone)]
pub struct BitfieldAccessor {
    pub bitfield_name: String,
    pub field_name: String,
    pub kind: BitfieldAccessorKind,
    pub total_bits: Option<u32>,
    pub field_width: Option<u32>,
}

/// Parse bitfield fields from signature text.
pub fn parse_bitfield_fields(signature: &str) -> Vec<BitfieldField> {
    let mut fields = Vec::new();
    // Find text between { and }
    let Some(open) = signature.find('{') else {
        return fields;
    };
    let Some(close) = signature.rfind('}') else {
        return fields;
    };
    if open >= close {
        return fields;
    }
    let body = &signature[open + 1..close];

    for part in body.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Format: "field_name : hi .. lo" or "field_name : idx"
        let Some((name, range)) = part.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let range = range.trim();
        if name.is_empty() {
            continue;
        }

        if let Some((hi_s, lo_s)) = range.split_once("..") {
            let hi = hi_s.trim().parse::<u32>().ok();
            let lo = lo_s.trim().parse::<u32>().ok();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                fields.push(BitfieldField { name: name.to_string(), hi, lo });
            }
        } else if let Ok(idx) = range.parse::<u32>() {
            fields.push(BitfieldField { name: name.to_string(), hi: idx, lo: idx });
        }
    }
    fields
}

/// Extract total bits from bitfield signature (e.g., `Some(32)`).
pub fn total_bits_from_signature(signature: &str) -> Option<u32> {
    let bits_pos = signature.find("bits(")?;
    let start = bits_pos + 5; // len("bits(")
    let end = signature[start..].find(')')? + start;
    signature[start..end].trim().parse::<u32>().ok()
}

/// Generate accessor names for a bitfield and field.
pub fn accessor_names(bitfield_name: &str, field_name: &str) -> (String, String, String, String) {
    (
        format!("Mk_{bitfield_name}"),
        format!("_get_{bitfield_name}_{field_name}"),
        format!("_update_{bitfield_name}_{field_name}"),
        format!("_set_{bitfield_name}_{field_name}"),
    )
}

/// Parse a name as a bitfield accessor (`Mk_T`, `_get_T_field`, etc.).
pub fn parse_accessor_name(name: &str) -> Option<BitfieldAccessor> {
    if let Some(rest) = name.strip_prefix("Mk_") {
        return Some(BitfieldAccessor {
            bitfield_name: rest.to_string(),
            field_name: String::new(),
            kind: BitfieldAccessorKind::Constructor,
            total_bits: None,
            field_width: None,
        });
    }
    if let Some(rest) = name.strip_prefix("_get_") {
        let (bf, field) = split_accessor_name(rest)?;
        return Some(BitfieldAccessor {
            bitfield_name: bf,
            field_name: field,
            kind: BitfieldAccessorKind::Getter,
            total_bits: None,
            field_width: None,
        });
    }
    if let Some(rest) = name.strip_prefix("_update_") {
        let (bf, field) = split_accessor_name(rest)?;
        return Some(BitfieldAccessor {
            bitfield_name: bf,
            field_name: field,
            kind: BitfieldAccessorKind::Updater,
            total_bits: None,
            field_width: None,
        });
    }
    if let Some(rest) = name.strip_prefix("_set_") {
        let (bf, field) = split_accessor_name(rest)?;
        return Some(BitfieldAccessor {
            bitfield_name: bf,
            field_name: field,
            kind: BitfieldAccessorKind::Setter,
            total_bits: None,
            field_width: None,
        });
    }
    None
}

/// Split "TypeName_fieldname" into ("TypeName", "fieldname").
/// Heuristic: first uppercase segment is the type name.
fn split_accessor_name(s: &str) -> Option<(String, String)> {
    let first_lower = s.find(|c: char| c == '_')?;
    let bf = &s[..first_lower];
    let field = &s[first_lower + 1..];
    if bf.is_empty() || field.is_empty() {
        return None;
    }
    Some((bf.to_string(), field.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fields_basic() {
        let sig = "bitfield Foo = bits(32) { opcode : 31 .. 26, rs1 : 19 .. 15 }";
        let fields = parse_bitfield_fields(sig);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "opcode");
        assert_eq!(fields[0].hi, 31);
        assert_eq!(fields[0].lo, 26);
        assert_eq!(fields[0].width(), 6);
        assert_eq!(fields[1].name, "rs1");
        assert_eq!(fields[1].hi, 19);
        assert_eq!(fields[1].lo, 15);
        assert_eq!(fields[1].width(), 5);
    }

    #[test]
    fn parse_fields_single_bit() {
        let sig = "bitfield Flags = bits(8) { carry : 0 }";
        let fields = parse_bitfield_fields(sig);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].hi, 0);
        assert_eq!(fields[0].lo, 0);
        assert_eq!(fields[0].width(), 1);
    }

    #[test]
    fn total_bits() {
        assert_eq!(total_bits_from_signature("bitfield X = bits(32) { }"), Some(32));
        assert_eq!(total_bits_from_signature("bitfield X = bits(64) { }"), Some(64));
        assert_eq!(total_bits_from_signature("val x : int"), None);
    }

    #[test]
    fn accessor_name_generation() {
        let (mk, get, upd, set) = accessor_names("Foo", "bar");
        assert_eq!(mk, "Mk_Foo");
        assert_eq!(get, "_get_Foo_bar");
        assert_eq!(upd, "_update_Foo_bar");
        assert_eq!(set, "_set_Foo_bar");
    }

    #[test]
    fn parse_accessor_name_constructor() {
        let acc = parse_accessor_name("Mk_MyType").unwrap();
        assert_eq!(acc.kind, BitfieldAccessorKind::Constructor);
        assert_eq!(acc.bitfield_name, "MyType");
    }

    #[test]
    fn parse_accessor_name_getter() {
        let acc = parse_accessor_name("_get_Foo_bar").unwrap();
        assert_eq!(acc.kind, BitfieldAccessorKind::Getter);
        assert_eq!(acc.bitfield_name, "Foo");
        assert_eq!(acc.field_name, "bar");
    }

    #[test]
    fn parse_accessor_name_non_accessor() {
        assert!(parse_accessor_name("regular_function").is_none());
        assert!(parse_accessor_name("get_something").is_none());
    }
}
