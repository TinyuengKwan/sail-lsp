//! One optional previous physical parse per file, used only as a reparse seed.
//! This is a performance cache, not a semantic input: users must validate both
//! source text and fixity context, and may always discard it. The weak tree
//! reference lets Salsa's normal query eviction release syntax trees.
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug)]
pub struct ParseSeed {
    pub text: Arc<str>,
    pub green: Weak<rowan::GreenNode>,
    pub parser_errors: Vec<(String, rowan::TextRange)>,
    pub fixities: Arc<parser::FixityContext>,
}

#[derive(Clone, Default)]
pub struct ParseCache(Arc<Mutex<Option<Arc<ParseSeed>>>>);

impl ParseCache {
    pub fn get(&self) -> Option<Arc<ParseSeed>> {
        self.0.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
    }

    pub fn store(&self, seed: ParseSeed) {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::new(seed));
    }

    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
    }
}

impl std::fmt::Debug for ParseCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseCache").finish_non_exhaustive()
    }
}
impl PartialEq for ParseCache {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ParseCache {}
