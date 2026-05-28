//! Multi-file test fixture using `//- /path.sail` syntax.
//!
//! Supports `$0` cursor marker for position-dependent tests
//! (completions, goto-definition, hover, etc.).
//!
//! ```ignore
//! let fixture = MultiFileFixture::parse("
//!   //- /prelude.sail
//!   val print : string -> unit
//!
//!   //- /main.sail
//!   $include \"prelude.sail\"
//!   function main() = print($0\"hello\")
//! ");
//! let (pos, file) = fixture.cursor_position().unwrap();
//! // pos.file_id = main.sail's FileId, pos.offset = offset of $0
//! ```

use base_db::FileId;
use hir_def::include_graph::IncludeGraph;
use hir_expand::in_file::{FilePosition, FileRange};
use std::collections::HashMap;

/// Cursor marker used in test fixtures.
/// Place `$0` in source text to mark the cursor position.
pub const CURSOR_MARKER: &str = "$0";

/// Extract the offset of the first `$0` marker and return the cleaned text.
/// Returns `None` if no marker is found.
pub fn extract_offset(text: &str) -> Option<(base_db::TextSize, String)> {
    let cursor_pos = text.find(CURSOR_MARKER)?;
    let mut clean = String::with_capacity(text.len());
    clean.push_str(&text[..cursor_pos]);
    clean.push_str(&text[cursor_pos + CURSOR_MARKER.len()..]);
    Some((base_db::TextSize::from(cursor_pos as u32), clean))
}

/// Extract a range marked by two `$0` markers, or a single offset.
pub enum RangeOrOffset {
    Range(base_db::TextRange),
    Offset(base_db::TextSize),
}

pub fn extract_range_or_offset(text: &str) -> Option<(RangeOrOffset, String)> {
    let first = text.find(CURSOR_MARKER)?;
    let after_first = first + CURSOR_MARKER.len();
    let rest = &text[after_first..];

    if let Some(second_rel) = rest.find(CURSOR_MARKER) {
        // Two markers → range.
        let second = after_first + second_rel;
        let mut clean = String::with_capacity(text.len());
        clean.push_str(&text[..first]);
        clean.push_str(&text[after_first..second]);
        clean.push_str(&text[second + CURSOR_MARKER.len()..]);
        let range = base_db::TextRange::new(
            base_db::TextSize::from(first as u32),
            base_db::TextSize::from((first + second_rel) as u32),
        );
        Some((RangeOrOffset::Range(range), clean))
    } else {
        // One marker → offset.
        let (offset, clean) = extract_offset(text)?;
        Some((RangeOrOffset::Offset(offset), clean))
    }
}

/// A single file within a multi-file fixture.
#[derive(Debug, Clone)]
pub struct FixtureFile {
    /// Virtual path (e.g., "/prelude.sail").
    pub path: String,
    /// Source text.
    pub text: String,
    /// FileId assigned during parsing.
    pub file_id: FileId,
}

/// Parsed multi-file test fixture.
#[derive(Debug, Clone)]
pub struct MultiFileFixture {
    pub files: Vec<FixtureFile>,
    pub include_graph: IncludeGraph,
    path_to_id: HashMap<String, FileId>,
    /// Cursor position extracted from `$0` marker (if any).
    cursor: Option<FilePosition>,
}

impl MultiFileFixture {
    /// Parse a fixture string using `//- /path.sail` delimiters.
    ///
    /// Each `//- /path.sail` line starts a new file. Lines before
    /// the first delimiter are ignored. `$include` directives in
    /// file text automatically build the include graph.
    pub fn parse(input: &str) -> Self {
        let mut files = Vec::new();
        let mut current_path: Option<String> = None;
        let mut current_lines: Vec<String> = Vec::new();
        let mut next_id = 0u32;
        let mut path_to_id: HashMap<String, FileId> = HashMap::new();
        let mut cursor: Option<FilePosition> = None;

        for line in input.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("//-") {
                // Flush previous file.
                if let Some(path) = current_path.take() {
                    let raw_text = current_lines.join("\n") + "\n";
                    let fid = FileId::from_raw(next_id);
                    next_id += 1;
                    // Extract $0 cursor marker if present.
                    let text = if let Some((offset, clean)) = extract_offset(&raw_text) {
                        if cursor.is_none() {
                            cursor = Some(FilePosition { file_id: fid, offset });
                        }
                        clean
                    } else {
                        raw_text
                    };
                    path_to_id.insert(path.clone(), fid);
                    files.push(FixtureFile { path, text, file_id: fid });
                    current_lines.clear();
                }
                let path = rest.trim().to_string();
                current_path = Some(path);
            } else if current_path.is_some() {
                current_lines.push(line.to_string());
            }
        }
        // Flush last file.
        if let Some(path) = current_path {
            let raw_text = current_lines.join("\n") + "\n";
            let fid = FileId::from_raw(next_id);
            let text = if let Some((offset, clean)) = extract_offset(&raw_text) {
                if cursor.is_none() {
                    cursor = Some(FilePosition { file_id: fid, offset });
                }
                clean
            } else {
                raw_text
            };
            path_to_id.insert(path.clone(), fid);
            files.push(FixtureFile { path, text, file_id: fid });
        }

        // Build include graph from $include directives
        let mut graph = IncludeGraph::new();
        for file in &files {
            for line in file.text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("$include") {
                    let rest = rest.trim();
                    let target = rest
                        .strip_prefix('"')
                        .and_then(|r| r.strip_suffix('"'))
                        .or_else(|| rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')));
                    if let Some(target_name) = target {
                        // Match by filename suffix
                        if let Some(&target_id) = path_to_id
                            .iter()
                            .find(|(p, _)| p.ends_with(target_name))
                            .map(|(_, id)| id)
                        {
                            graph.add_edge(file.file_id, target_id);
                        }
                    }
                }
            }
        }

        Self { files, include_graph: graph, path_to_id, cursor }
    }

    /// Get a file by path.
    pub fn file(&self, path: &str) -> Option<&FixtureFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Get FileId for a path.
    pub fn file_id(&self, path: &str) -> Option<FileId> {
        self.path_to_id.get(path).copied()
    }

    /// Number of files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Is empty?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the cursor position extracted from `$0` marker.
    /// Panics if no `$0` was present in any file.
    pub fn cursor_position(&self) -> FilePosition {
        self.cursor.expect("fixture has no $0 cursor marker")
    }

    /// Get the cursor position if a `$0` marker was present.
    pub fn try_cursor_position(&self) -> Option<FilePosition> {
        self.cursor
    }

    /// Get the cursor position as a `FileRange` (zero-width range at offset).
    pub fn cursor_range(&self) -> FileRange {
        let pos = self.cursor_position();
        FileRange { file_id: pos.file_id, range: base_db::TextRange::empty(pos.offset) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_files() {
        let fixture = MultiFileFixture::parse(
            "
            //- /a.sail
            val foo : int
            //- /b.sail
            val bar : bool
        ",
        );
        assert_eq!(fixture.len(), 2);
        assert!(fixture.file("/a.sail").is_some());
        assert!(fixture.file("/b.sail").is_some());
        assert!(fixture.file("/a.sail").unwrap().text.contains("val foo"));
    }

    #[test]
    fn include_graph_built() {
        let fixture = MultiFileFixture::parse(
            "
            //- /prelude.sail
            val helper : int -> int
            //- /main.sail
            $include \"prelude.sail\"
            function main() = helper(1)
        ",
        );
        assert_eq!(fixture.len(), 2);
        let main_id = fixture.file_id("/main.sail").unwrap();
        let prelude_id = fixture.file_id("/prelude.sail").unwrap();
        assert_eq!(
            fixture.include_graph.includes_of(main_id),
            &[prelude_id],
            "main.sail should include prelude.sail"
        );
    }

    #[test]
    fn empty_fixture() {
        let fixture = MultiFileFixture::parse("");
        assert_eq!(fixture.len(), 0);
    }

    #[test]
    fn extract_offset_basic() {
        let (offset, clean) = super::extract_offset("val foo$0 : int").unwrap();
        assert_eq!(offset, base_db::TextSize::from(7));
        assert_eq!(clean, "val foo : int");
    }

    #[test]
    fn extract_offset_none() {
        assert!(super::extract_offset("val foo : int").is_none());
    }

    #[test]
    fn extract_range_two_markers() {
        let (result, clean) = super::extract_range_or_offset("val $0foo$0 : int").unwrap();
        match result {
            super::RangeOrOffset::Range(r) => {
                assert_eq!(r.start(), base_db::TextSize::from(4));
                assert_eq!(r.end(), base_db::TextSize::from(7));
            }
            _ => panic!("expected range"),
        }
        assert_eq!(clean, "val foo : int");
    }

    #[test]
    fn cursor_in_fixture() {
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            val foo : int
            function bar($0x : int) = x + 1
        ",
        );
        let pos = fixture.cursor_position();
        assert_eq!(pos.file_id, fixture.file_id("/main.sail").unwrap());
        // Text should have $0 stripped.
        let main = fixture.file("/main.sail").unwrap();
        assert!(!main.text.contains("$0"), "text: {}", main.text);
        assert!(main.text.contains("bar(x : int)"), "text: {}", main.text);
        // Offset should point to where $0 was (after "function bar(").
        let expected_offset = main.text.find("x : int").unwrap();
        assert_eq!(pos.offset, base_db::TextSize::from(expected_offset as u32));
    }

    #[test]
    fn cursor_in_second_file() {
        let fixture = MultiFileFixture::parse(
            "
            //- /a.sail
            val a : int
            //- /b.sail
            val b$0 : bool
        ",
        );
        let pos = fixture.cursor_position();
        assert_eq!(pos.file_id, fixture.file_id("/b.sail").unwrap());
    }

    #[test]
    fn no_cursor_try() {
        let fixture = MultiFileFixture::parse(
            "
            //- /a.sail
            val a : int
        ",
        );
        assert!(fixture.try_cursor_position().is_none());
    }
}
