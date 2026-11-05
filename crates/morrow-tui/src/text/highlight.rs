//! Lightweight syntax highlighting.
//!
//! A small hand-rolled tokenizer rather than a real grammar engine: the goal is
//! that code pasted into the transcript is *scannable*, not that it is
//! syntactically perfect. It covers the languages that actually appear in this
//! project (rust, bash, json, toml, python, js/ts, go) and degrades to dim
//! monochrome for anything else — it never guesses.
//!
//! The tokenizer is single-pass with a small "what kind of token is this"
//! classifier per language. It is not a full lexer: it will mis-color an
//! unterminated string, which is a cosmetic worst case and still readable.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::Theme;

/// A classified token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Plain,
    Comment,
    Str,
    Num,
    Kw,
    Type,
    Fn,
}

/// Which language a fence declares, normalized to a supported one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Bash,
    Json,
    Toml,
    Python,
    Js,
    Go,
    Unknown,
}

impl Lang {
    /// Map a fence info string to a supported language.
    ///
    /// Empty and unrecognised values become [`Lang::Unknown`], which renders as
    /// dim plain text — deliberately not a default guess, because mislabeling
    /// Python as Rust produces confidently wrong colors.
    pub fn detect(tag: &str) -> Lang {
        let t = tag.trim().to_ascii_lowercase();
        let t = t.split([' ', ',', ':']).next().unwrap_or("");
        match t {
            "rust" | "rs" => Lang::Rust,
            "bash" | "sh" | "shell" | "zsh" | "console" | "" => Lang::Bash,
            "json" | "jsonc" => Lang::Json,
            "toml" | "ini" => Lang::Toml,
            "python" | "py" => Lang::Python,
            "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => Lang::Js,
            "go" | "golang" => Lang::Go,
            _ => Lang::Unknown,
        }
    }

    /// Canonical display name for the fence label.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Bash => "bash",
            Lang::Json => "json",
            Lang::Toml => "toml",
            Lang::Python => "python",
            Lang::Js => "js",
            Lang::Go => "go",
            Lang::Unknown => "text",
        }
    }
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use",
    "where", "while",
];
const RUST_TYPE: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16",
    "u32", "u64", "u128", "usize", "String", "Vec", "Option", "Result", "Box",
];

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];
const PY_TYPE: &[&str] = &[
    "bool", "bytes", "dict", "float", "int", "list", "set", "str", "tuple",
];

const JS_KW: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "yield",
];

const GO_KW: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Classify a single bare word for the given language.
fn word_tok(lang: Lang, w: &str) -> Tok {
    match lang {
        Lang::Rust => {
            if RUST_KW.contains(&w) {
                Tok::Kw
            } else if RUST_TYPE.contains(&w) || w.chars().next().is_some_and(char::is_uppercase) {
                // Capitalized identifiers are conventionally types in Rust.
                Tok::Type
            } else {
                Tok::Plain
            }
        }
        Lang::Python => {
            if PY_KW.contains(&w) {
                Tok::Kw
            } else if PY_TYPE.contains(&w) {
                Tok::Type
            } else {
                Tok::Plain
            }
        }
        Lang::Js => {
            if JS_KW.contains(&w) {
                Tok::Kw
            } else if w == "true" || w == "false" || w == "null" || w == "undefined" {
                Tok::Num
            } else {
                Tok::Plain
            }
        }
        Lang::Go => {
            if GO_KW.contains(&w) {
                Tok::Kw
            } else {
                Tok::Plain
            }
        }
        _ => Tok::Plain,
    }
}

/// Tokenize one line of `lang` code into styled spans.
///
/// Single pass over the line, tracked with a byte cursor rather than a peekable
/// char iterator: a peekable iterator cannot skip ahead by a token of unknown
/// length without losing its position, which silently dropped characters.
///
/// Returns a single span for empty lines so the line still occupies height.
pub fn line_spans(line: &str, lang: Lang, theme: &Theme) -> Vec<Span<'static>> {
    if line.is_empty() {
        return vec![Span::styled(String::new(), Style::new().fg(theme.text))];
    }

    // Whole-line comment fast paths.
    let trimmed = line.trim_start();
    let whole_comment = match lang {
        Lang::Rust | Lang::Js | Lang::Go => trimmed.starts_with("//"),
        Lang::Python => trimmed.starts_with('#'),
        Lang::Bash | Lang::Toml => trimmed.starts_with('#'),
        Lang::Json | Lang::Unknown => false,
    };
    if whole_comment {
        return vec![Span::styled(
            line.to_string(),
            style_for(theme, Tok::Comment),
        )];
    }

    let body = Style::new().fg(theme.text);
    let comment_style = style_for(theme, Tok::Comment);
    let mut spans: Vec<Span<'static>> = Vec::new();
    // Pending plain text, flushed whenever a styled token is emitted.
    let mut buf = String::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut in_str: Option<u8> = None;
    let mut escaped = false;

    while i < line.len() {
        let c = bytes[i];
        let ch = line[i..].chars().next().unwrap_or('\0');

        // Inside a string: consume verbatim until the closing quote.
        if let Some(q) = in_str {
            buf.push(ch);
            i += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if c == q {
                in_str = None;
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    style_for(theme, Tok::Str),
                ));
            }
            continue;
        }

        // Open a string. A quote directly after a word character is an
        // apostrophe or a Rust lifetime, not a string opener.
        if (ch == '"' || ch == '\'') && lang != Lang::Unknown {
            let prev_is_word = i > 0 && is_word_byte(bytes[i - 1]);
            let opens = match ch {
                // Single quotes are strings in bash/python/json, lifetimes in rust.
                '\'' => matches!(lang, Lang::Bash | Lang::Python | Lang::Json),
                _ => true,
            };
            if opens && !prev_is_word {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), body));
                }
                buf.push(ch);
                i += ch.len_utf8();
                in_str = Some(c);
                continue;
            }
        }

        let rest = &line[i..];

        // Comment start.
        let is_comment = match lang {
            Lang::Rust | Lang::Js | Lang::Go => rest.starts_with("//"),
            Lang::Python => ch == '#',
            Lang::Bash | Lang::Toml => ch == '#' && i == 0,
            Lang::Json | Lang::Unknown => false,
        };
        if is_comment {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), body));
            }
            spans.push(Span::styled(rest.to_string(), comment_style));
            return spans;
        }

        // Number.
        if ch.is_ascii_digit() && !(i > 0 && is_word_byte(bytes[i - 1])) {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), body));
            }
            let start = i;
            i += 1;
            while i < line.len() {
                let n = line[i..].chars().next().unwrap_or('\0');
                if n.is_alphanumeric() || n == '_' || n == '.' {
                    i += n.len_utf8();
                } else {
                    break;
                }
            }
            buf.push_str(&line[start..i]);
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style_for(theme, Tok::Num),
            ));
            continue;
        }

        // Identifier, keyword, or call.
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            i += ch.len_utf8();
            while i < line.len() {
                let n = line[i..].chars().next().unwrap_or('\0');
                if n.is_alphanumeric() || n == '_' {
                    i += n.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[start..i];
            // A `(` or `!` after the word means it was called.
            let next = line[i..].trim_start().chars().next();
            let tok = if matches!(next, Some('(')) && lang != Lang::Unknown {
                Tok::Fn
            } else {
                word_tok(lang, word)
            };
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), body));
            }
            buf.push_str(word);
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style_for(theme, tok),
            ));
            continue;
        }

        // Whitespace and punctuation stay in the body color.
        buf.push(ch);
        i += ch.len_utf8();
    }

    if !buf.is_empty() {
        spans.push(Span::styled(buf, body));
    }
    spans
}

/// True when this ASCII byte can appear inside a word or number.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Map a token class to its theme style.
pub fn style_for(theme: &Theme, tok: Tok) -> Style {
    let color = match tok {
        Tok::Plain => theme.text,
        Tok::Comment => theme.code_comment,
        Tok::Str => theme.code_str,
        Tok::Num => theme.code_num,
        Tok::Kw => theme.code_kw,
        Tok::Type => theme.code_type,
        Tok::Fn => theme.code_fn,
    };
    Style::new().fg(color)
}

/// Style used for a language we do not recognize: dim, no guessing.
pub fn unknown_style(theme: &Theme) -> Style {
    Style::new().fg(theme.text_dim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// Colors of a rendered line, for asserting token classification.
    fn colors(line: &str, lang: Lang) -> Vec<Color> {
        line_spans(line, lang, &Theme::DARK)
            .into_iter()
            .map(|s| s.style.fg.unwrap_or(Color::Reset))
            .collect()
    }

    #[test]
    fn rust_line_assigns_distinct_colors_to_keyword_and_number() {
        let theme = Theme::DARK;
        let out = line_spans("let n = 42;", Lang::Rust, &theme);
        let seen: Vec<Color> = out.iter().filter_map(|s| s.style.fg).collect();
        assert!(
            seen.contains(&theme.code_kw),
            "keyword not colored: {seen:?}"
        );
        assert!(
            seen.contains(&theme.code_num),
            "number not colored: {seen:?}"
        );
        assert!(!seen.contains(&theme.code_comment), "code is not a comment");
    }

    #[test]
    fn function_call_is_colored_as_a_function() {
        let theme = Theme::DARK;
        assert!(colors("doThing()", Lang::Rust).contains(&theme.code_fn));
    }

    #[test]
    fn lang_detection_is_conservative() {
        assert_eq!(Lang::detect("rust"), Lang::Rust);
        assert_eq!(Lang::detect("RS"), Lang::Rust);
        assert_eq!(Lang::detect("bash"), Lang::Bash);
        assert_eq!(Lang::detect("sh"), Lang::Bash);
        assert_eq!(Lang::detect("typescript"), Lang::Js);
        assert_eq!(Lang::detect("brainfuck"), Lang::Unknown);
        // An empty fence tag is a shell session by convention, not a guess at
        // a "default language".
        assert_eq!(Lang::detect(""), Lang::Bash);
    }

    #[test]
    fn lang_label_is_canonical() {
        assert_eq!(Lang::detect("rs").label(), "rust");
        assert_eq!(Lang::detect("py").label(), "python");
        assert_eq!(Lang::detect("xyz").label(), "text");
    }

    #[test]
    fn rust_keywords_and_types_classify() {
        assert_eq!(word_tok(Lang::Rust, "let"), Tok::Kw);
        assert_eq!(word_tok(Lang::Rust, "String"), Tok::Type);
        assert_eq!(word_tok(Lang::Rust, "foo"), Tok::Plain);
    }

    #[test]
    fn highlighting_covers_the_whole_line() {
        let theme = Theme::DARK;
        let out = line_spans("let x = 1;", Lang::Rust, &theme);
        let joined: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            joined, "let x = 1;",
            "spans must not drop or duplicate text"
        );
    }

    #[test]
    fn highlighting_is_lossless_on_code_with_strings() {
        let theme = Theme::DARK;
        let cases = [
            ("fn main() { println!(\"hi\"); }", Lang::Rust),
            ("echo \"hello # world\"", Lang::Bash),
            ("{\"key\": \"value\"}", Lang::Json),
            ("x = 1  # comment", Lang::Python),
        ];
        for (src, lang) in cases {
            let out = line_spans(src, lang, &theme);
            let joined: String = out.iter().map(|s| s.content.to_string()).collect();
            assert_eq!(joined, src, "lossless for {src:?} as {lang:?}");
        }
    }

    #[test]
    fn slashes_inside_json_strings_are_not_comments() {
        let theme = Theme::DARK;
        let src = "{\"url\": \"http://example.com\"}";
        let out = line_spans(src, Lang::Json, &theme);
        let joined: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, src);
        // The `//` inside the string must not have turned the rest into a comment.
        assert!(
            out.iter().all(|s| s.style.fg != Some(theme.code_comment)),
            "URL was mistaken for a comment"
        );
    }

    #[test]
    fn whole_line_comment_short_circuits() {
        let theme = Theme::DARK;
        let out = line_spans("// just a comment", Lang::Rust, &theme);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].style.fg, Some(theme.code_comment));
    }

    #[test]
    fn empty_line_still_produces_a_span() {
        let out = line_spans("", Lang::Rust, &Theme::DARK);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn multiline_safe_no_panic_on_unicode() {
        let theme = Theme::DARK;
        for src in ["let s = \"日本語\";", "// ünïcode ✓", "x = \"🎉\""] {
            let out = line_spans(src, Lang::Rust, &theme);
            let joined: String = out.iter().map(|s| s.content.to_string()).collect();
            assert_eq!(joined, src);
        }
    }

    #[test]
    fn unknown_language_renders_dim_not_guessed() {
        let theme = Theme::DARK;
        let out = line_spans("some random text", Lang::Unknown, &theme);
        let joined: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "some random text");
    }
}
