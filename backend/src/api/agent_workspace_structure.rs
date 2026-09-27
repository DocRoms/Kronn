//! Structural guard for the non-Rust sources a shell-less worker edits.
//!
//! Rust gets a real parser (`syn`) in `agent_workspace_tools`. Other languages
//! get two cheap, language-aware checks that catch what local models actually
//! break on bounded edits: an orphan delimiter, and a replaced range whose
//! boundary lines drift from the file's indentation. Neither is a compiler:
//! both only refuse a change that makes a readable file worse.

/// Stable marker consumed by the bounded worker loop, like the Rust one.
pub(crate) const STRUCTURE_REFUSAL_PREFIX: &str = "Structure validation refused";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Language {
    CLike,
    Php,
    Css,
    Scss,
    Json,
    Twig,
}

fn language(path: &std::path::Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Language::CLike,
        "php" => Language::Php,
        "css" => Language::Css,
        "scss" | "less" => Language::Scss,
        "json" => Language::Json,
        "twig" => Language::Twig,
        _ => return None,
    })
}

/// Whether this file gets the non-Rust structural guard at all.
pub(crate) fn is_guarded(path: &std::path::Path) -> bool {
    language(path).is_some()
}

fn refusal(requested: &str, detail: String) -> String {
    format!(
        "{STRUCTURE_REFUSAL_PREFIX} `{requested}`: {detail}. Nothing was written; the previous \
         `content_sha256` remains authoritative. Make one bounded repair from this exact \
         diagnostic, then hand off to a stronger worker instead of exploring again."
    )
}

/// The first delimiter problem of `text`, as `(line, message)`.
fn first_imbalance(language: Language, text: &str) -> Option<(usize, String)> {
    if language == Language::Twig {
        return twig_imbalance(text);
    }
    let bytes = text.as_bytes();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut line = 1usize;
    let mut index = 0usize;
    let line_comments = match language {
        Language::CLike | Language::Scss => &["//"][..],
        Language::Php => &["//", "#"][..],
        Language::Css | Language::Json | Language::Twig => &[][..],
    };
    let block_comments = language != Language::Json;
    let quotes: &[u8] = match language {
        Language::Json => b"\"",
        Language::CLike => b"\"'`",
        _ => b"\"'",
    };
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            line += 1;
            index += 1;
            continue;
        }
        let rest = &text[index..];
        if block_comments && rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(rest.len(), |end| end + 4);
            line += rest[..end].matches('\n').count();
            index += end;
            continue;
        }
        // `#{` is SCSS interpolation and `#[` a PHP attribute, not comments.
        if let Some(marker) = line_comments
            .iter()
            .find(|marker| rest.starts_with(**marker))
        {
            let is_code = *marker == "#" && matches!(bytes.get(index + 1), Some(b'{' | b'['));
            // `url(http://…)` in a stylesheet is not a comment.
            let is_url = *marker == "//" && index > 0 && bytes[index - 1] == b':';
            if !is_code && !is_url {
                let end = rest.find('\n').unwrap_or(rest.len());
                index += end;
                continue;
            }
        }
        if quotes.contains(&byte) {
            let start_line = line;
            index += 1;
            let mut closed = false;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 2,
                    b'\n' if byte != b'`' && language != Language::Php => break,
                    quote if quote == byte => {
                        index += 1;
                        closed = true;
                        break;
                    }
                    b'\n' => {
                        line += 1;
                        index += 1;
                    }
                    _ => index += 1,
                }
            }
            if !closed && byte != b'\'' {
                return Some((
                    start_line,
                    format!("unterminated `{}` string", byte as char),
                ));
            }
            continue;
        }
        match byte {
            b'{' | b'(' | b'[' => stack.push((byte, line)),
            b'}' | b')' | b']' => {
                let expected = match byte {
                    b'}' => b'{',
                    b')' => b'(',
                    _ => b'[',
                };
                match stack.pop() {
                    Some((open, _)) if open == expected => {}
                    Some((open, open_line)) => {
                        return Some((
                            line,
                            format!(
                                "`{}` closes the `{}` opened at line {open_line}",
                                byte as char, open as char
                            ),
                        ));
                    }
                    None => {
                        return Some((line, format!("orphan closing `{}`", byte as char)));
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    stack
        .last()
        .map(|(open, open_line)| (*open_line, format!("`{}` is never closed", *open as char)))
}

/// Twig tag delimiters and the block tags that need an `end…` partner.
fn twig_imbalance(text: &str) -> Option<(usize, String)> {
    const BLOCKS: &[&str] = &[
        "if",
        "for",
        "block",
        "macro",
        "embed",
        "apply",
        "with",
        "spaceless",
        "verbatim",
        "autoescape",
        "filter",
        "sandbox",
        "set",
    ];
    let mut blocks: Vec<(String, usize)> = Vec::new();
    let mut index = 0usize;
    while let Some(offset) = text[index..].find('{') {
        let start = index + offset;
        let line = text[..start].matches('\n').count() + 1;
        let close = match text.as_bytes().get(start + 1) {
            Some(b'{') => "}}",
            Some(b'%') => "%}",
            Some(b'#') => "#}",
            _ => {
                index = start + 1;
                continue;
            }
        };
        let body_start = start + 2;
        let Some(length) = text[body_start..].find(close) else {
            return Some((
                line,
                format!(
                    "`{}` is never closed by `{close}`",
                    &text[start..body_start]
                ),
            ));
        };
        let body = text[body_start..body_start + length]
            .trim_matches(|c: char| c == '-' || c == '~' || c.is_whitespace());
        index = body_start + length + close.len();
        if close != "%}" {
            continue;
        }
        let keyword = body.split_whitespace().next().unwrap_or_default();
        if let Some(name) = keyword.strip_prefix("end") {
            match blocks.pop() {
                Some((open, _)) if open == name => {}
                Some((open, open_line)) => {
                    return Some((
                        line,
                        format!(
                            "`{{% {keyword} %}}` closes the `{open}` opened at line {open_line}"
                        ),
                    ));
                }
                None => return Some((line, format!("orphan `{{% {keyword} %}}`"))),
            }
        } else if BLOCKS.contains(&keyword)
            // `{% set x = 1 %}` and `{% block title page.title %}` have no end tag.
            && !(keyword == "set" && body.contains('='))
            && !(keyword == "block" && body.split_whitespace().count() > 2)
        {
            blocks.push((keyword.to_string(), line));
        }
    }
    blocks.last().map(|(open, line)| {
        (
            *line,
            format!("`{{% {open} %}}` is never closed by `{{% end{open} %}}`"),
        )
    })
}

/// Refuse a change that unbalances a file whose delimiters were balanced. A
/// file the lexer cannot read as balanced to begin with is not judged.
pub(crate) fn validate_delimiters(
    path: &std::path::Path,
    requested: &str,
    original: &str,
    proposed: &str,
) -> Result<(), String> {
    let Some(language) = language(path) else {
        return Ok(());
    };
    if first_imbalance(language, original).is_some() {
        return Ok(());
    }
    match first_imbalance(language, proposed) {
        None => Ok(()),
        Some((line, message)) => Err(refusal(requested, format!("line {line}: {message}"))),
    }
}

fn indentation(line: &str) -> &str {
    let text = line.trim_end_matches(['\n', '\r']);
    &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
}

fn describe(indent: &str) -> String {
    let spaces = indent.bytes().filter(|byte| *byte == b' ').count();
    let tabs = indent.len() - spaces;
    match (spaces, tabs) {
        (0, 0) => "no indentation".into(),
        (spaces, 0) => format!("{spaces} space(s)"),
        (0, tabs) => format!("{tabs} tab(s)"),
        (spaces, tabs) => format!("{spaces} space(s) and {tabs} tab(s)"),
    }
}

/// A bounded replacement keeps the indentation of the lines it replaces at
/// its edges: the first non-blank line, and the last one when the range spans
/// several. Inner lines may move freely.
pub(crate) fn validate_range_indentation(
    path: &std::path::Path,
    requested: &str,
    start_line: usize,
    replaced_lines: &[&str],
    new_string: &str,
) -> Result<(), String> {
    if !is_guarded(path) {
        return Ok(());
    }
    let non_blank = |line: &&str| !line.trim().is_empty();
    let original: Vec<&str> = replaced_lines.iter().copied().filter(non_blank).collect();
    let proposed: Vec<&str> = new_string.lines().filter(non_blank).collect();
    let (Some(first), Some(new_first)) = (original.first(), proposed.first()) else {
        return Ok(());
    };
    let check = |which: &str, line: usize, before: &str, after: &str| {
        let (expected, actual) = (indentation(before), indentation(after));
        if expected == actual {
            return Ok(());
        }
        Err(refusal(
            requested,
            format!(
                "the {which} line of `new_string` must keep the indentation of line {line} \
                 ({}), not {}",
                describe(expected),
                describe(actual)
            ),
        ))
    };
    let first_line = start_line + replaced_lines.iter().position(non_blank).unwrap_or(0);
    check("first", first_line, first, new_first)?;
    if original.len() > 1 {
        let (last, new_last) = (original[original.len() - 1], proposed[proposed.len() - 1]);
        let last_line = start_line + replaced_lines.iter().rposition(non_blank).unwrap_or(0);
        check("last", last_line, last, new_last)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn delimiters(path: &str, original: &str, proposed: &str) -> Result<(), String> {
        validate_delimiters(Path::new(path), path, original, proposed)
    }

    #[test]
    fn an_orphan_brace_in_scss_or_php_is_refused() {
        let scss = ".a {\n  color: red;\n}\n";
        let error = delimiters("a.scss", scss, ".a {\n  color: red;\n}\n}\n").unwrap_err();
        assert!(error.starts_with(STRUCTURE_REFUSAL_PREFIX), "{error}");
        assert!(error.contains("line 4: orphan closing `}`"), "{error}");

        let php = "<?php\nfunction a() {\n    return 1;\n}\n";
        let error = delimiters("a.php", php, "<?php\nfunction a() {\n    return 1;\n").unwrap_err();
        assert!(error.contains("`{` is never closed"), "{error}");
    }

    #[test]
    fn delimiters_inside_strings_comments_and_interpolation_do_not_count() {
        let scss = ".a { content: \"}\"; /* { */ width: #{$w}; } // }\n";
        assert!(delimiters("a.scss", ".a {}\n", scss).is_ok());
        let php = "<?php\n$a = '{'; // }\n# {\n#[Attr]\nfunction b() { return \"(\"; }\n";
        assert!(delimiters("a.php", "<?php\n", php).is_ok());
        let ts = "const a = `${b} {`;\nconst url = 'http://x';\n";
        assert!(delimiters("a.ts", "", ts).is_ok());
        let css = ".a { background: url(http://x/y.png); }\n";
        assert!(delimiters("a.css", "", css).is_ok());
    }

    #[test]
    fn a_file_the_lexer_cannot_balance_is_not_judged() {
        // A regex literal holding a brace reads as unbalanced before and after.
        let before = "const re = /{/;\n";
        assert!(delimiters("a.ts", before, "const re = /{/;\nconst b = 1;\n").is_ok());
    }

    #[test]
    fn twig_tags_and_blocks_must_pair() {
        let before = "{% if a %}\n  {{ a }}\n{% endif %}\n";
        assert!(delimiters("a.twig", before, "{% if a %}\n  {{ b }}\n{% endif %}\n").is_ok());
        let error = delimiters("a.twig", before, "{% if a %}\n  {{ a }}\n").unwrap_err();
        assert!(error.contains("`{% if %}` is never closed"), "{error}");
        let error =
            delimiters("a.twig", before, "{% if a %}\n  {{ a }\n{% endif %}\n").unwrap_err();
        assert!(error.contains("never closed by `}}`"), "{error}");
        // An inline `set` or a shorthand `block` has no end tag.
        assert!(delimiters("a.twig", before, "{% set x = 1 %}\n{% if a %}{% endif %}\n").is_ok());
        assert!(delimiters("a.twig", before, "{% block title page.title %}\n").is_ok());
    }

    #[test]
    fn other_languages_are_left_alone() {
        assert!(delimiters("a.md", "", "}").is_ok());
        assert!(delimiters("a.py", "", ")").is_ok());
    }

    fn indent(path: &str, start: usize, replaced: &[&str], new_string: &str) -> Result<(), String> {
        validate_range_indentation(Path::new(path), path, start, replaced, new_string)
    }

    #[test]
    fn a_shifted_first_or_last_line_is_refused_for_php_twig_and_scss() {
        // The measured failure: two spaces became three on a one-line range.
        let error = indent("a.twig", 9, &["  {{ a }}\n"], "   {{ b }}").unwrap_err();
        assert!(error.starts_with(STRUCTURE_REFUSAL_PREFIX), "{error}");
        assert!(
            error.contains("line 9 (2 space(s)), not 3 space(s)"),
            "{error}"
        );

        let range = ["    <li>\n", "      {{ a }}\n", "    </li>\n"];
        let error =
            indent("a.twig", 38, &range, "     <li>\n      {{ b }}\n     </li>").unwrap_err();
        assert!(error.contains("first line"), "{error}");
        let error = indent("a.scss", 34, &["  .a {\n", "  }\n"], "  .a {\n   }").unwrap_err();
        assert!(
            error.contains("last line") && error.contains("line 35"),
            "{error}"
        );
        let error = indent("a.php", 2, &["\treturn 1;\n"], "    return 2;").unwrap_err();
        assert!(error.contains("1 tab(s)"), "{error}");
    }

    #[test]
    fn inner_lines_blank_edges_and_deletions_may_move_freely() {
        let range = ["    <li>\n", "      {{ a }}\n", "    </li>\n"];
        assert!(indent(
            "a.twig",
            38,
            &range,
            "    <li>\n          {{ b }}\n    </li>"
        )
        .is_ok());
        assert!(indent("a.twig", 38, &["\n", "  a\n"], "\n  b").is_ok());
        assert!(indent("a.twig", 38, &range, "").is_ok());
        // One line becoming a block keeps the first line's indentation only.
        assert!(indent(
            "a.scss",
            4,
            &["  color: red;\n"],
            "  color: red;\n    & b { x: y; }"
        )
        .is_ok());
        assert!(indent("a.md", 1, &["  - a\n"], "- a").is_ok());
    }
}
