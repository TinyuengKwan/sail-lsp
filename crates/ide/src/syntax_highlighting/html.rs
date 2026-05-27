//! HTML output for syntax highlighting (testing/documentation).
//! Two entry points:
//! - `highlight_as_html()`: high-level, takes Analysis + FileId (RA-compatible)
//! - `render_html()`: low-level, takes pre-computed highlights

use base_db::TextRange;

/// High-level: highlight source text and render as HTML.
/// Takes source text and a highlighting function that computes
/// semantic token ranges. Renders as HTML with `<span class="...">`.
pub fn highlight_as_html(
    source: &str,
    compute_highlights: impl FnOnce(&str) -> Vec<(TextRange, String)>,
) -> String {
    let highlights = compute_highlights(source);
    let hl_refs: Vec<(TextRange, &str)> =
        highlights.iter().map(|(r, s)| (*r, s.as_str())).collect();
    render_html(source, &hl_refs)
}

/// Low-level: render pre-computed highlights as HTML.
///
/// Each highlighted range becomes a `<span class="...">` element.
/// Unhighlighted text is HTML-escaped and included verbatim.
pub fn render_html(source: &str, highlights: &[(TextRange, &str)]) -> String {
    let mut result = String::from("<pre><code>");
    let mut last_end = 0u32;

    for (range, class) in highlights {
        let start: usize = u32::from(range.start()) as usize;
        let end: usize = u32::from(range.end()) as usize;

        // Text before this highlight
        if start > last_end as usize {
            result.push_str(&html_escape(&source[last_end as usize..start]));
        }
        // Highlighted span
        result.push_str(&format!("<span class=\"{}\">", class));
        result.push_str(&html_escape(&source[start..end]));
        result.push_str("</span>");
        last_end = end as u32;
    }
    // Remaining text
    if (last_end as usize) < source.len() {
        result.push_str(&html_escape(&source[last_end as usize..]));
    }
    result.push_str("</code></pre>");
    result
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_html_rendering() {
        let html = render_html("val x : int", &[]);
        assert!(html.contains("val x : int"));
        assert!(html.starts_with("<pre><code>"));
    }

    #[test]
    fn html_with_highlights() {
        let html = render_html("val x : int", &[(TextRange::new(0.into(), 3.into()), "keyword")]);
        assert!(html.contains("<span class=\"keyword\">val</span>"));
        assert!(html.contains("x : int"));
    }
}
