//! Global `Arc`-based object interning infrastructure.
//!
//! Eventually this should probably be replaced with salsa-based interning.

mod gc;
mod intern;
mod intern_slice;
mod symbol;

pub use self::gc::{GarbageCollector, GcInternedSliceVisit, GcInternedVisit};
pub use self::intern::{impl_internable, InternStorage, Internable, Interned, InternedRef};
pub use self::intern_slice::{
    impl_slice_internable, InternSliceStorage, InternedSlice, InternedSliceRef, SliceInternable,
};
pub use self::symbol::{symbols as sym, Symbol};
