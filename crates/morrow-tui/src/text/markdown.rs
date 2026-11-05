//! Markdown-aware rendering for assistant prose.
//!
//! Models emit markdown; the transcript used to print it verbatim, so `#`
//! markers, `-` bullets and ``` fences all showed up as literal characters.
//! This module turns that text into styled blocks, choosing a presentation per
//! construct and degrading to plain wrapped text for anything unrecognized.
//!
//! Deliberately not a CommonMark implementation. It handles the constructs a
//! chat model actually produces, and anything else renders as readable prose
//! rather than as an error.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::text::highlight::{self, Lang};
use crate::text::{display_width, truncate_cols};
use crate::theme::Theme;

/// Indent (in cells) applied to blockquote and code-fence bodies.
const INDENT: usize = 2;

/// Render `text` as styled lines fitted to `width` columns.
///
/// `width` is the full content width available, excluding any gutter the caller
/// draws. Lines are already wrapped to `width`; ratatui renders them without
/// re-wrapping, which keeps scroll math and pixel math in agreement.
pub fn render_markdown(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = raw.trim_end();

        // --- fenced code block -------------------------------------------
        if let Some(tag) = fence_open(line) {
            let lang = Lang::detect(tag);
            let mut body: Vec<&str> = Vec::new();
            for l in lines.by_ref() {
                if fence_close(l.trim()) {
                    break;
                }
                body.push(l);
            }
            render_code_block(&body, lang, theme, &mut out);
            continue;
        }

        // --- blank -------------------------------------------------------
        if line.trim().is_empty() {
            // Collapse runs of blank lines to a single spacer.
            if out.last().is_some_and(|l: &Line| !is_spacer(l)) {
                out.push(Line::from(""));
            }
            continue;
        }

        // --- ATX heading --------------------------------------------------
        if let Some((level, body)) = atx_heading(line) {
            render_heading(level, body, width, theme, &mut out);
            continue;
        }

        // --- horizontal rule ----------------------------------------------
        if is_hr(line) {
            out.push(Line::from(Span::styled(
                "─".repeat(width.min(72)),
                Style::new().fg(theme.border),
            )));
            continue;
        }

        // --- blockquote ---------------------------------------------------
        if let Some(body) = line.trim_start().strip_prefix('>') {
            let mut quote: Vec<String> = vec![body.trim_start().to_string()];
            while let Some(next) = lines.peek() {
                match next.trim_start().strip_prefix('>') {
                    Some(b) => {
                        quote.push(b.trim_start().to_string());
                        lines.next();
                    }
                    None => break,
                }
            }
            render_quote(&quote.join("\n"), width, theme, &mut out);
            continue;
        }

        // --- table ---------------------------------------------------------
        if is_table_row(line) && lines.peek().is_some_and(|n| is_table_divider(n.trim())) {
            let header = line.to_string();
            lines.next(); // consume the divider row
            let mut rows: Vec<String> = Vec::new();
            while let Some(n) = lines.peek() {
                if is_table_row(n.trim()) {
                    rows.push(n.to_string());
                    lines.next();
                } else {
                    break;
                }
            }
            render_table(&header, &rows, width, theme, &mut out);
            continue;
        }

        // --- list item ------------------------------------------------------
        if let Some((marker, body, ordered)) = list_item(line) {
            render_list_item(marker, &body, ordered, width, theme, &mut out, &mut lines);
            continue;
        }

        // --- plain paragraph (may run over several physical lines) ---------
        let mut para: Vec<&str> = vec![line];
        while let Some(next) = lines.peek() {
            let n = next.trim_end();
            // A blank, or any new block construct, ends the paragraph.
            let starts_block = n.trim().is_empty()
                || fence_open(n).is_some()
                || atx_heading(n).is_some()
                || is_hr(n)
                || list_item(n).is_some()
                || n.trim_start().starts_with('>');
            if starts_block {
                break;
            }
            para.push(n);
            lines.next();
        }
        let joined = para.join(" ");
        for w in crate::text::wrap(&joined, width) {
            out.push(Line::from(inline_spans(&w, theme)));
        }
    }

    out
}

/// ` ```lang ` opens a fence; returns the language tag.
fn fence_open(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix("```")
        .map(|rest| rest.trim().split(' ').next().unwrap_or("").trim())
}

/// A line of only backticks closes a fence.
fn fence_close(t: &str) -> bool {
    t.starts_with("```")
}

/// `# Heading` through `###### Heading`. Returns (level, text).
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    // A heading needs a space after the hashes, else it is not a heading.
    if !rest.starts_with(' ') {
        return None;
    }
    Some((hashes, rest.trim()))
}

/// `---`, `***`, `___` with at least three repeats.
fn is_hr(line: &str) -> bool {
    let t: String = line.trim().chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// Detect a markdown table row: `| a | b |` with a leading and trailing pipe,
/// or any row containing a pipe with surrounding cell whitespace.
fn is_table_row(t: &str) -> bool {
    t.contains('|') && t.trim_matches('|').contains('|')
}

/// `|---|---|` — the divider under a table header.
fn is_table_divider(t: &str) -> bool {
    if !t.contains('-') || !t.contains('|') {
        return false;
    }
    t.chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c.is_whitespace())
}

/// `- item`, `* item`, `+ item`, `1. item`, `1) item`.
///
/// Returns (marker, content, is_ordered).
fn list_item(line: &str) -> Option<(String, String, bool)> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if indent > 8 {
        return None;
    }
    // Unordered.
    for m in ['-', '*', '+'] {
        if let Some(rest) = t.strip_prefix(m) {
            if rest.starts_with(' ') {
                return Some((m.to_string(), rest.trim().to_string(), false));
            }
        }
    }
    // Ordered: digits then `.` or `)` then a space.
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = &t[digits.len()..];
        for sep in ['.', ')'] {
            if let Some(r) = rest.strip_prefix(sep) {
                if r.starts_with(' ') {
                    return Some((format!("{digits}{sep}"), r.trim().to_string(), true));
                }
            }
        }
    }
    None
}

/// How far a list item is indented, in levels. Two levels are rendered; deeper
/// nesting flattens to the second level rather than indenting into the margin.
fn list_depth(line: &str) -> usize {
    let spaces = line.len() - line.trim_start().len();
    (spaces / 2).min(2)
}

fn render_heading(
    level: usize,
    text: &str,
    width: usize,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    let style = match level {
        1 => Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        2 => Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        3..=4 => Style::new()
            .fg(theme.text)
            .add_modifier(Modifier::BOLD | Modifier::ITALIC),
        _ => Style::new().fg(theme.text_dim),
    };
    let prefix = "#".repeat(level.min(3));
    let marker = format!("{prefix} ");
    let wrapped = crate::text::wrap(text, width.saturating_sub(marker.len().max(1)));
    for (i, w) in wrapped.iter().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(Span::styled(
                marker.clone(),
                Style::new().fg(theme.accent).add_modifier(Modifier::DIM),
            ));
        }
        spans.extend(inline_spans(w, theme));
        out.push(Line::from(spans).style(style));
    }
    // A hairline under h1/h2 gives the section a visible edge.
    if level <= 2 {
        out.push(Line::from(Span::styled(
            "─".repeat(width.min(72)),
            Style::new().fg(theme.border),
        )));
    }
}

fn render_quote(text: &str, width: usize, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let inner = width.saturating_sub(INDENT + 1);
    for w in crate::text::wrap(text, inner) {
        let mut spans = vec![Span::styled(
            format!("{} ", crate::theme::glyph::QUOTE),
            Style::new().fg(theme.border_focus),
        )];
        spans.extend(
            inline_spans(&w, theme)
                .into_iter()
                .map(|s| Span::styled(s.content, Style::new().fg(theme.text_dim))),
        );
        out.push(Line::from(spans));
    }
}

fn render_code_block(body: &[&str], lang: Lang, theme: &Theme, out: &mut Vec<Line<'static>>) {
    // Cap very large blocks: highlighting is per-character work and a 5k-line
    // dump would stall the frame. Past the cap, fall back to dim plain text.
    const MAX_HIGHLIGHT_LINES: usize = 400;
    let highlight = lang != Lang::Unknown && body.len() <= MAX_HIGHLIGHT_LINES;

    out.push(Line::from(vec![
        Span::styled(
            format!("{} CODE ", crate::theme::glyph::FENCE),
            Style::new().fg(theme.border_focus),
        ),
        Span::styled(lang.label(), Style::new().fg(theme.accent)),
    ]));

    if body.is_empty() {
        out.push(Line::from(""));
        return;
    }
    for l in body {
        if highlight {
            let spans = highlight::line_spans(l, lang, theme);
            let mut v = vec![Span::raw(" ".repeat(INDENT))];
            v.extend(spans);
            out.push(Line::from(v));
        } else {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(INDENT)),
                Span::styled(l.to_string(), highlight::unknown_style(theme)),
            ]));
        }
    }
    out.push(Line::from(""));
}

fn render_table(
    header: &str,
    rows: &[String],
    width: usize,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    let mut header_cells = split_row(header);
    let mut body: Vec<Vec<String>> = rows.iter().map(|r| split_row(r)).collect();

    // Column widths from content, then clamp to fit the available space.
    let ncols = header_cells
        .len()
        .max(body.iter().map(|r| r.len()).max().unwrap_or(0));
    let mut widths = vec![0usize; ncols];
    for (i, c) in header_cells.iter().enumerate() {
        widths[i] = widths[i].max(display_width(c));
    }
    for r in &body {
        for (i, c) in r.iter().enumerate() {
            if i < ncols {
                widths[i] = widths[i].max(display_width(c));
            }
        }
    }
    // If the natural table is wider than the viewport, shrink the widest columns.
    let gap = 2;
    let total: usize = widths.iter().sum::<usize>() + gap * ncols.saturating_sub(1);
    if total > width {
        let mut excess = total - width;
        // Take from the widest column first, repeatedly.
        while excess > 0 {
            let Some(widest) = widths
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| **w)
                .map(|(i, _)| i)
            else {
                break;
            };
            if widths[widest] <= 3 {
                break;
            }
            widths[widest] -= 1;
            excess -= 1;
        }
    }

    let render_row = |cells: &[String], style: Style| -> Line<'static> {
        let mut spans = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            spans.push(Span::styled(pad(cell, *w), style));
            if i + 1 < ncols {
                spans.push(Span::raw(" ".repeat(gap)));
            }
        }
        Line::from(spans)
    };

    header_cells.resize(ncols, String::new());
    out.push(render_row(
        &header_cells,
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
    ));
    out.push(Line::from(Span::styled(
        widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join(&" ".repeat(gap)),
        Style::new().fg(theme.border),
    )));
    for r in &mut body {
        r.resize(ncols, String::new());
        out.push(render_row(r, Style::new().fg(theme.text_dim)));
    }
}

fn render_list_item(
    marker: String,
    body: &str,
    ordered: bool,
    width: usize,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
    lines: &mut std::iter::Peekable<std::str::Lines<'_>>,
) {
    let marker_style = if ordered {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text_dim)
    };
    let lead = format!("  {marker} ");
    let hang = lead.len();
    let inner = width.saturating_sub(hang).max(8);
    let wrapped = crate::text::wrap(body, inner);
    for (i, w) in wrapped.iter().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(Span::styled(lead.clone(), marker_style));
        } else {
            spans.push(Span::raw(" ".repeat(hang)));
        }
        spans.extend(inline_spans(w, theme));
        out.push(Line::from(spans));
    }
    // A nested bullet directly under this item belongs to it.
    while let Some(next) = lines.peek() {
        let n = next.trim_end();
        if list_item(n).is_some() && list_depth(n) > 0 {
            let (m, b, o) = list_item(n).unwrap();
            lines.next();
            let sub_marker = if o { m.clone() } else { "·".to_string() };
            let sub_lead = format!("    {sub_marker} ");
            let sub_hang = sub_lead.len();
            let sub_inner = width.saturating_sub(sub_hang).max(8);
            for (i, w) in crate::text::wrap(&b, sub_inner).iter().enumerate() {
                let mut spans = Vec::new();
                if i == 0 {
                    spans.push(Span::styled(
                        sub_lead.clone(),
                        Style::new().fg(theme.text_faint),
                    ));
                } else {
                    spans.push(Span::raw(" ".repeat(sub_hang)));
                }
                spans.extend(inline_spans(w, theme));
                out.push(Line::from(spans));
            }
        } else {
            break;
        }
    }
}

/// Split a table row into trimmed cells.
fn split_row(row: &str) -> Vec<String> {
    row.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Pad a cell to `width` display columns, truncating when too long.
fn pad(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w == width {
        s.to_string()
    } else if w < width {
        format!("{s}{}", " ".repeat(width - w))
    } else {
        truncate_cols(s, width)
    }
}

/// Style inline `` `code` ``, `**bold**`, `*italic*` and bare URLs.
///
/// Inline formatting is applied with a simple left-to-right scan rather than a
/// parser: it handles the constructs models emit, and anything unmatched simply
/// renders as body text.
fn inline_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // `code`
        if c == '`' {
            if let Some(end) = find_char(&chars, i + 1, '`') {
                let inner: String = chars[i + 1..end].iter().collect();
                if !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        Style::new().fg(theme.text),
                    ));
                }
                spans.push(Span::styled(
                    inner,
                    Style::new().fg(theme.code_str).bg(theme.surface),
                ));
                i = end + 1;
                continue;
            }
        }

        // **bold**
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = find_seq(&chars, i + 2, "**") {
                let inner: String = chars[i + 2..end].iter().collect();
                if !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        Style::new().fg(theme.text),
                    ));
                }
                spans.push(Span::styled(
                    inner,
                    Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
                ));
                i = end + 2;
                continue;
            }
        }

        // *italic*
        if c == '*' {
            if let Some(end) = find_char(&chars, i + 1, '*') {
                let inner: String = chars[i + 1..end].iter().collect();
                if !inner.trim().is_empty() {
                    if !buf.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut buf),
                            Style::new().fg(theme.text),
                        ));
                    }
                    spans.push(Span::styled(
                        inner,
                        Style::new()
                            .fg(theme.text_dim)
                            .add_modifier(Modifier::ITALIC),
                    ));
                    i = end + 1;
                    continue;
                }
            }
        }

        buf.push(c);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, Style::new().fg(theme.text)));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), Style::new().fg(theme.text)));
    }
    spans
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

fn find_seq(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    (from..chars.len().saturating_sub(n.len() - 1)).find(|&i| chars[i..i + n.len()] == n[..])
}

fn is_spacer(l: &Line) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

/// Markdown source and the plain text it should read as, for tests.
#[cfg(test)]
pub(crate) fn plain(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(src: &str, w: usize) -> String {
        plain(&render_markdown(src, w, &Theme::DARK))
    }

    #[test]
    fn plain_text_is_wrapped_not_marker_prefixed() {
        let out = r("hello world", 40);
        assert!(out.contains("hello world"));
        assert!(!out.contains('#'));
    }

    #[test]
    fn heading_keeps_a_depth_marker_and_gains_a_rule() {
        let out = r("## Findings", 40);
        assert!(out.contains("Findings"), "heading text missing: {out}");
        // The marker is retained, but dimmed, as a depth cue rather than as
        // literal markdown syntax.
        assert!(out.contains('#'), "depth cue missing: {out}");
        assert!(out.contains('─'), "h2 should draw a rule");
    }

    #[test]
    fn heading_marker_is_styled_dim_not_as_body_text() {
        let theme = Theme::DARK;
        let lines = render_markdown("# Title", 40, &theme);
        let marker = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains('#'))
            .expect("marker span");
        assert!(marker.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn deeper_headings_use_shorter_markers() {
        let h1 = r("# A", 40);
        let h4 = r("#### D", 40);
        let n = |s: &str| s.chars().filter(|c| *c == '#').count();
        assert!(n(&h1) < n(&h4), "deeper heading should show more markers");
    }

    #[test]
    fn hashes_without_a_space_are_not_headings() {
        // `#hashtag` is not a heading; it must stay literal.
        let out = r("#hashtag", 40);
        assert!(out.contains("#hashtag"));
    }

    #[test]
    fn unordered_and_ordered_lists_render_with_markers() {
        let out = r("- alpha\n- beta", 40);
        assert!(out.contains('-'), "bullet missing");
        assert!(out.contains("alpha"));
        let out = r("1. first\n2. second", 40);
        assert!(out.contains("1."));
        assert!(out.contains("second"));
    }

    #[test]
    fn nested_list_items_indent() {
        let out = r("- outer\n  - inner", 40);
        let lines: Vec<&str> = out.lines().collect();
        let inner_line = lines.iter().find(|l| l.contains("inner")).unwrap();
        assert!(
            inner_line.starts_with("    "),
            "nested item should indent, got {inner_line:?}"
        );
    }

    #[test]
    fn fenced_code_gets_a_label_and_no_fence_markers() {
        let out = r("```rust\nlet x = 1;\n```", 60);
        assert!(out.contains("CODE"));
        assert!(out.contains("rust"));
        assert!(!out.contains("```"), "fence markers leaked into output");
        assert!(out.contains("let x = 1;"));
    }

    #[test]
    fn unknown_fence_language_is_labeled_text_and_not_colored() {
        let theme = Theme::DARK;
        let lines = render_markdown("```brainfuck\n+++++\n```", 40, &theme);
        let body = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("+++++")))
            .unwrap();
        let colored = body.spans.iter().filter(|s| s.style.fg.is_some()).count();
        assert!(
            colored <= 1,
            "unknown language should not be highlighted: {body:?}"
        );
    }

    #[test]
    fn unterminated_fence_still_renders() {
        let out = r("```rust\nlet x = 1;", 60);
        assert!(out.contains("CODE"));
        assert!(out.contains("let x = 1;"));
    }

    #[test]
    fn blockquote_gets_a_bar() {
        let out = r("> quoted text", 40);
        assert!(out.contains(crate::theme::glyph::QUOTE));
        assert!(out.contains("quoted text"));
    }

    #[test]
    fn table_aligns_columns() {
        let src = "| name | value |\n|------|-------|\n| a | 1 |\n| bb | 22 |";
        let out = r(src, 60);
        assert!(out.contains("name"), "header missing: {out}");
        assert!(out.contains('─'), "table rule missing: {out}");
        // Header cells and body cells must start at the same column.
        let lines: Vec<&str> = out.lines().collect();
        // Match on a distinctive token so `find` cannot hit a substring of
        // another cell.
        let col = |needle: &str| -> usize {
            lines
                .iter()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} missing from:\n{out}"))
                .find(needle)
                .expect("needle is present in the line it was found in")
        };
        assert_eq!(col("name"), col("bb"), "columns not aligned:\n{out}");
    }

    #[test]
    fn wide_table_is_clamped_to_the_viewport() {
        let src = "| aaaaaaaaaa | bbbbbbbbbb | cccccccccc |\n|---|---|---|\n| 1 | 2 | 3 |";
        let lines = render_markdown(src, 24, &Theme::DARK);
        for l in &lines {
            assert!(
                display_width(&plain(std::slice::from_ref(l))) <= 24,
                "row overflowed: {:?}",
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            );
        }
    }

    #[test]
    fn inline_code_and_bold_are_styled() {
        let theme = Theme::DARK;
        let lines = render_markdown("use `let` and **bold** here", 60, &theme);
        let inline: Vec<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.content == "let" || s.content == "bold")
            .collect();
        assert_eq!(inline.len(), 2, "inline spans not isolated");
        assert_ne!(inline[0].style, inline[1].style, "styles should differ");
    }

    #[test]
    fn unbalanced_markers_do_not_lose_text() {
        for src in ["a ` b", "a ** b", "a * b", "`", "**", "*"] {
            let out = r(src, 40);
            let stripped: String = src.chars().filter(|c| !c.is_whitespace()).collect();
            let got: String = out.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(got, stripped, "text lost for {src:?}");
        }
    }

    #[test]
    fn horizontal_rule_is_rendered_as_a_rule() {
        let out = r("---", 40);
        assert!(out.trim().chars().all(|c| c == '─'), "got {out:?}");
    }

    #[test]
    fn long_paragraph_wraps_within_width() {
        let para = "word ".repeat(60);
        for l in render_markdown(&para, 30, &Theme::DARK) {
            let w: usize = l.spans.iter().map(|s| display_width(&s.content)).sum();
            assert!(w <= 30, "line overflowed to {w}: {l:?}");
        }
    }

    #[test]
    fn pathological_input_does_not_panic() {
        let cases = [
            "x".repeat(10_000),
            format!("{}`", "a ".repeat(5_000)),
            "- ".repeat(2_000),
            "| a | b |\n|---|---|\n".repeat(500),
            "```".repeat(500),
            "> ".repeat(1_000),
        ];
        for src in cases {
            let lines = render_markdown(&src, 40, &Theme::DARK);
            // Must terminate and produce something.
            assert!(!lines.is_empty() || src.is_empty());
        }
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert!(render_markdown("", 40, &Theme::DARK).is_empty());
    }

    #[test]
    fn light_theme_renders_without_panic() {
        let lines = render_markdown("## H\n\n- a\n\n```rust\nlet x=1;\n```", 60, &Theme::LIGHT);
        assert!(!lines.is_empty());
    }

    #[test]
    fn mono_theme_renders_without_panic() {
        let lines = render_markdown("**bold** and `code`", 60, &Theme::MONO);
        assert!(!lines.is_empty());
    }
}
