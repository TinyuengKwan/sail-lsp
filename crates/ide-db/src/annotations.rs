//! Inline `//^^^ expected` annotations in test fixtures.
//!
//! Ported from rust-analyzer's `test-utils` crate (`extract_annotations`).
//! Sail uses the same `//` line comment, so the syntax carries over
//! unchanged; only the imports differ.

use std::collections::BTreeMap;

use base_db::{TextRange, TextSize};

/// Extracts `//^^^ some text` annotations.
///
/// A run of `^^^` can be arbitrary long and points to the corresponding range
/// in the line above.
///
/// The `// ^file text` syntax can be used to attach `text` to the entirety of
/// the file.
///
/// Multiline string values are supported:
///
/// // ^^^ first line
/// //   | second line
///
/// Trailing whitespace is sometimes desired but usually stripped by the editor
/// if at the end of a line, or incorrectly sized if followed by another
/// annotation. In those cases the annotation can be explicitly ended with the
/// `$` character.
///
/// // ^^^ trailing-ws-wanted  $
///
/// Annotations point to the last line that actually was long enough for the
/// range, not counting annotations themselves. So overlapping annotations are
/// possible:
/// ```text
/// // stuff        other stuff
/// // ^^ 'st'
/// // ^^^^^ 'stuff'
/// //              ^^^^^^^^^^^ 'other stuff'
/// ```
pub fn extract_annotations(text: &str) -> Vec<(TextRange, String)> {
    let mut res = Vec::new();
    // map from line length to beginning of last line that had that length
    let mut line_start_map = BTreeMap::new();
    let mut line_start: TextSize = 0.into();
    let mut prev_line_annotations: Vec<(TextSize, usize)> = Vec::new();
    for line in text.split_inclusive('\n') {
        let mut this_line_annotations = Vec::new();
        let line_length = if let Some((prefix, suffix)) = line.split_once("//") {
            let ss_len = TextSize::of("//");
            let annotation_offset = TextSize::of(prefix) + ss_len;
            for annotation in extract_line_annotations(suffix.trim_end_matches('\n')) {
                match annotation {
                    LineAnnotation::Annotation { mut range, content, file } => {
                        range += annotation_offset;
                        this_line_annotations.push((range.end(), res.len()));
                        let range = if file {
                            TextRange::up_to(TextSize::of(text))
                        } else {
                            let line_start = line_start_map.range(range.end()..).next().unwrap();

                            range + line_start.1
                        };
                        res.push((range, content));
                    }
                    LineAnnotation::Continuation { mut offset, content } => {
                        offset += annotation_offset;
                        let &(_, idx) = prev_line_annotations
                            .iter()
                            .find(|&&(off, _idx)| off == offset)
                            .unwrap();
                        res[idx].1.push('\n');
                        res[idx].1.push_str(&content);
                        res[idx].1.push('\n');
                    }
                }
            }
            annotation_offset
        } else {
            TextSize::of(line)
        };

        line_start_map = line_start_map.split_off(&line_length);
        line_start_map.insert(line_length, line_start);

        line_start += TextSize::of(line);

        prev_line_annotations = this_line_annotations;
    }

    res
}

enum LineAnnotation {
    Annotation { range: TextRange, content: String, file: bool },
    Continuation { offset: TextSize, content: String },
}

fn extract_line_annotations(mut line: &str) -> Vec<LineAnnotation> {
    let mut res = Vec::new();
    let mut offset: TextSize = 0.into();
    let marker: fn(char) -> bool = if line.contains('^') { |c| c == '^' } else { |c| c == '|' };
    while let Some(idx) = line.find(marker) {
        offset += TextSize::try_from(idx).unwrap();
        line = &line[idx..];

        let mut len = line.chars().take_while(|&it| it == '^').count();
        let mut continuation = false;
        if len == 0 {
            assert!(line.starts_with('|'));
            continuation = true;
            len = 1;
        }
        let range = TextRange::at(offset, len.try_into().unwrap());
        let line_no_caret = &line[len..];
        let end_marker = line_no_caret.find('$');
        let next = line_no_caret.find(marker).map_or(line.len(), |it| it + len);

        let cond = |end_marker| {
            end_marker < next
                && (line_no_caret[end_marker + 1..].is_empty()
                    || line_no_caret[end_marker + 1..]
                        .strip_prefix(|c: char| c.is_whitespace() || c == '^')
                        .is_some())
        };
        let mut content = match end_marker {
            Some(end_marker) if cond(end_marker) => &line_no_caret[..end_marker],
            _ => line_no_caret[..next - len].trim_end(),
        };

        let mut file = false;
        if !continuation && content.starts_with("file") {
            file = true;
            content = &content["file".len()..];
        }

        let content = content.trim_start().to_owned();

        let annotation = if continuation {
            LineAnnotation::Continuation { offset: range.end(), content }
        } else {
            LineAnnotation::Annotation { range, content, file }
        };
        res.push(annotation);

        line = &line[next..];
        offset += TextSize::try_from(next).unwrap();
    }

    res
}

#[cfg(test)]
mod tests {
    use super::extract_annotations;

    #[test]
    fn annotation_points_at_the_line_above() {
        let text = "\
val foo : int -> int
//  ^^^ error: nope
";
        let res = extract_annotations(text);
        let spans: Vec<_> = res
            .iter()
            .map(|(r, s)| (&text[usize::from(r.start())..usize::from(r.end())], s.as_str()))
            .collect();
        assert_eq!(spans, [("foo", "error: nope")]);
    }

    #[test]
    fn several_annotations_on_one_line() {
        let text = "\
function f(x) = x
//         ^ a
//              ^ b
";
        let res = extract_annotations(text);
        let spans: Vec<_> = res
            .iter()
            .map(|(r, s)| (&text[usize::from(r.start())..usize::from(r.end())], s.as_str()))
            .collect();
        assert_eq!(spans, [("x", "a"), ("x", "b")]);
    }

    #[test]
    fn whole_file_annotation() {
        let text = "val foo : int
// ^file error: bad file
";
        let res = extract_annotations(text);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, super::TextRange::up_to(super::TextSize::of(text)));
        assert_eq!(res[0].1, "error: bad file");
    }

    #[test]
    fn continuation_lines_join_with_newlines() {
        let text = "\
val foo : int
//  ^^^ error: first
//    | second
";
        let res = extract_annotations(text);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].1, "error: first\nsecond\n");
    }
}
