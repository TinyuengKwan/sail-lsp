use std::collections::{HashMap, HashSet};
use std::path::{PathBuf, Path};
use std::sync::{OnceLock, RwLock, Mutex};
use lsp_types::{Url, SymbolKind, Location, Range, Position};
use regex::Regex;
use crossbeam_channel::Sender;

use crate::repl::SailRepl;
use crate::utils::byte_to_utf16_offset;

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub location: Location,
    pub kind: SymbolKind,
}

pub struct SailState {
    pub repl: Mutex<SailRepl>,
    pub files: RwLock<HashMap<Url, String>>,
    pub project_root: Option<PathBuf>,
    pub symbols: RwLock<HashMap<String, Vec<SymbolInfo>>>,
    pub project_files: RwLock<HashSet<PathBuf>>,
    pub diag_tx: Sender<(Url, bool)>,
}

pub fn get_ident_patterns() -> &'static Vec<(Regex, SymbolKind)> {
    static PATTERNS: OnceLock<Vec<(Regex, SymbolKind)>> = OnceLock::new();
    PATTERNS.get_or_init(|| vec![
        (Regex::new(r"(?m)^(?:val|function|overload|outcome)\s+([a-zA-Z0-9_#]+)").unwrap(), SymbolKind::FUNCTION),
        (Regex::new(r"(?m)^(?:type|union|struct|enum|mapping)\s+([a-zA-Z0-9_#]+)").unwrap(), SymbolKind::CLASS),
        (Regex::new(r"(?m)^(?:union|function|mapping|enum)\s+clause\s+([a-zA-Z0-9_#]+)").unwrap(), SymbolKind::METHOD),
        (Regex::new(r"(?m)^let\s+([a-zA-Z0-9_#]+)").unwrap(), SymbolKind::VARIABLE),
        (Regex::new(r"(?m)^register\s+([a-zA-Z0-9_#]+)").unwrap(), SymbolKind::FIELD),
    ])
}

pub fn get_diag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^STDERR:(.*?):(\d+)\.(\d+)-(\d+)\.(\d+): (.*)").unwrap())
}

impl SailState {
    pub fn new(diag_tx: Sender<(Url, bool)>) -> Self {
         SailState {
            repl: Mutex::new(SailRepl::new()),
            files: RwLock::new(HashMap::new()),
            project_root: None,
            symbols: RwLock::new(HashMap::new()),
            project_files: RwLock::new(HashSet::new()),
            diag_tx,
        }
    }

    pub fn find_sail_root(&self, file_path: &Path) -> Option<PathBuf> {
        if let Some(root) = &self.project_root {
            if root.join("ROOT").exists() { return Some(root.join("ROOT")); }
            if root.join(".sail_project").exists() { return Some(root.join(".sail_project")); }
        }
        let mut curr = file_path;
        while let Some(parent) = curr.parent() {
            if parent.join("ROOT").exists() { return Some(parent.join("ROOT")); }
            if parent.join(".sail_project").exists() { return Some(parent.join(".sail_project")); }
            curr = parent;
        }
        None
    }

    pub fn index_project(&self) {
        if let Some(root) = &self.project_root {
            let mut symbols: HashMap<String, Vec<SymbolInfo>> = HashMap::new();
            let mut project_files = HashSet::new();
            let patterns = get_ident_patterns();
            let glob_pattern = format!("{}/**/*.sail", root.to_string_lossy());
            
            if let Ok(entries) = glob::glob(&glob_pattern) {
                for entry in entries.flatten() {
                    project_files.insert(entry.clone());
                    if let Ok(content) = std::fs::read_to_string(&entry) {
                        for (i, line) in content.lines().enumerate() {
                            for (re, kind) in patterns {
                                for caps in re.captures_iter(line) {
                                    if let Some(m) = caps.get(1) {
                                        let sym = m.as_str().to_string();
                                        if let Ok(uri) = Url::from_file_path(&entry) {
                                            symbols.entry(sym).or_default().push(SymbolInfo {
                                                location: Location {
                                                    uri,
                                                    range: Range {
                                                        start: Position { line: i as u32, character: byte_to_utf16_offset(line, m.start()) },
                                                        end: Position { line: i as u32, character: byte_to_utf16_offset(line, m.end()) },
                                                    },
                                                },
                                                kind: *kind,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let mut guard = self.symbols.write().unwrap();
            *guard = symbols;
            let mut files_guard = self.project_files.write().unwrap();
            *files_guard = project_files;
        }
    }
}
