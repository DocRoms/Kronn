//! KT-1990 — the HTML document an execution of a step wrote, read from its
//! stored output so that Kronn, not the execution, makes the Artifact: the same
//! way for the main execution and every shadow, which has no Kronn tool.
//!
//! The document is the last HTML block of the output: an `html` or
//! `kronn-doc-preview` fence, or, when the output holds no such fence, a bare
//! document (`<!DOCTYPE html>` or `<html>` up to `</html>`).
//! - In a closed fence, the fence is the unit: the document runs from the first
//!   `<!doctype html>` or `<html>` of its body found outside a script, a style
//!   or a comment, to its last `</html>` ([`fenced_document`]); a fence with no
//!   such opening holds a fragment, whole.
//! - A bare document is read as a browser reads it: a doctype or an `<html>`
//!   tag opens one only when markup (`<`) follows it, so prose naming the tag
//!   counts for nothing; a `</html>` or an `<html>` inside a `<script>`, a
//!   `<style>` or a comment does not end or start one; and the last document
//!   closed wins ([`bare_document`]).
//!
//! A document that is cut is never used: a fence that never closes, an
//! `<html>` with no `</html>` after it, or an output the shadow stored cut
//! ([`super::shadow::CUT_MARKER`]).

use serde::Serialize;

/// What the output of an execution holds, as the run view tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionHtml {
    /// No HTML document.
    None,
    /// A complete document an Artifact can be made from.
    Complete,
    /// The last HTML fence never closes.
    CutUnclosedFence,
    /// The document opens `<html>` and never closes it.
    CutNoClosingTag,
    /// The output was stored cut, so the document may be incomplete.
    CutOutput,
}

impl ExecutionHtml {
    pub fn of(output: &str) -> Self {
        match find(output) {
            Found::None => ExecutionHtml::None,
            Found::Complete(_) => ExecutionHtml::Complete,
            Found::Cut(why) => why,
        }
    }

    /// Why an Artifact is refused, in the words of the API.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            ExecutionHtml::Complete => None,
            ExecutionHtml::None => Some("This execution's output holds no HTML document"),
            ExecutionHtml::CutUnclosedFence => Some(
                "The HTML document of this execution is cut: its block never closes; \
                 no Artifact was made",
            ),
            ExecutionHtml::CutNoClosingTag => Some(
                "The HTML document of this execution is cut: it has no closing </html>; \
                 no Artifact was made",
            ),
            ExecutionHtml::CutOutput => Some(
                "The output of this execution was stored cut, so its HTML document may be \
                 incomplete; no Artifact was made",
            ),
        }
    }
}

/// The document found in an output.
#[derive(Debug, PartialEq, Eq)]
pub enum Found<'a> {
    None,
    Complete(&'a str),
    Cut(ExecutionHtml),
}

struct Fence<'a> {
    info: String,
    body: &'a str,
    closed: bool,
}

/// The fenced blocks of a Markdown text, in order.
fn fences(text: &str) -> Vec<Fence<'_>> {
    let mut out = Vec::new();
    // (marker char, marker length, info, body start)
    let mut open: Option<(char, usize, String, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let indent = content.len() - content.trim_start_matches(' ').len();
        let trimmed = content.trim_start_matches(' ');
        if indent > 3 {
            continue;
        }
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let Some(marker) = marker else { continue };
        let run = trimmed.chars().take_while(|c| *c == marker).count();
        if run < 3 {
            continue;
        }
        match &open {
            Some((ch, len, info, body_start)) => {
                if *ch == marker && run >= *len && trimmed[run..].trim().is_empty() {
                    out.push(Fence {
                        info: info.clone(),
                        body: &text[*body_start..start],
                        closed: true,
                    });
                    open = None;
                }
            }
            None => {
                let info = trimmed[run..].trim();
                // A backtick fence's info string never holds a backtick.
                if marker == '`' && info.contains('`') {
                    continue;
                }
                let word = info.split_whitespace().next().unwrap_or("");
                open = Some((marker, run, word.to_ascii_lowercase(), offset));
            }
        }
    }
    if let Some((_, _, info, body_start)) = open {
        out.push(Fence {
            info,
            body: &text[body_start..],
            closed: false,
        });
    }
    out
}

const CLOSE_HTML: &str = "</html>";

/// Whether `lower[at..]` opens the tag `name` (`<html`, `<script`…): the name
/// is followed by `>`, `/` or a space, so `<htmlish>` is not `<html`.
fn tag_at(lower: &str, at: usize, name: &str) -> bool {
    lower[at..].starts_with(name)
        && matches!(
            lower.as_bytes().get(at + name.len()),
            Some(c) if *c == b'>' || *c == b'/' || c.is_ascii_whitespace()
        )
}

/// Whether `lower[at..]` opens a document: `<!doctype html` or an `<html` tag.
fn opening_at(lower: &str, at: usize) -> Option<Opening> {
    if lower[at..].starts_with("<!doctype html") {
        Some(Opening::Doctype)
    } else if tag_at(lower, at, "<html") {
        Some(Opening::Tag)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Opening {
    Doctype,
    Tag,
}

/// Whether markup follows the opening at `lower[at..]`: after its own `>` and
/// any whitespace, the next character is `<`. An opening the text ends in or
/// right after (no `>`, or nothing but whitespace after it) counts too, so a
/// cut output stays cut. An opening followed by anything else (a backtick, a
/// comma, a word) is prose naming the tag.
fn markup_follows(lower: &str, at: usize) -> bool {
    let Some(gt) = lower[at..].find('>') else {
        return true;
    };
    match lower[at + gt + 1..].trim_start().chars().next() {
        None => true,
        Some(next) => next == '<',
    }
}

/// When `lower[at..]` starts raw text an opening or a `</html>` inside does
/// not count in (a comment up to `-->`, a `<script>` up to `</script`, a
/// `<style>` up to `</style`): `Some(end)` with the offset after its end, or
/// `Some(None)` when it never ends. `None` when `at` starts no raw text.
fn raw_text_end(lower: &str, at: usize) -> Option<Option<usize>> {
    let (from, end) = if lower[at..].starts_with("<!--") {
        (at + 4, "-->")
    } else if tag_at(lower, at, "<script") {
        (at + 7, "</script")
    } else if tag_at(lower, at, "<style") {
        (at + 6, "</style")
    } else {
        return None;
    };
    Some(
        lower[from..]
            .find(end)
            .map(|found| from + found + end.len()),
    )
}

/// The document of a closed fence: the fence is the unit, so the document runs
/// from the first `<!doctype html>` or `<html>` of `body` found outside a
/// script, a style or a comment, to the fence's last `</html>`. Whatever a
/// script, a style or a comment holds stays in it: a document template in a
/// fragment's script is not the document. `None` when `body` holds no opening
/// outside them (a script, style or comment that never closes before any
/// opening means no opening); cut when no `</html>` follows the opening.
fn fenced_document(body: &str) -> Option<Result<&str, ExecutionHtml>> {
    let lower = body.to_ascii_lowercase();
    let mut at = 0;
    let start = loop {
        let here = at + lower[at..].find('<')?;
        if opening_at(&lower, here).is_some() {
            break here;
        }
        at = match raw_text_end(&lower, here) {
            Some(Some(end)) => end,
            Some(None) => return None,
            None => here + 1,
        };
    };
    Some(match lower[start..].rfind(CLOSE_HTML) {
        Some(close) => Ok(&body[start..start + close + CLOSE_HTML.len()]),
        None => Err(ExecutionHtml::CutNoClosingTag),
    })
}

/// The last document of a bare output (no HTML fence), read in order as a
/// browser reads it:
/// - a `<!doctype html>` or an `<html>` tag is an opening only when markup
///   follows it ([`markup_follows`]); one followed by text is prose naming the
///   tag and counts for nothing, so `<html>Hello</html>` with no doctype before
///   it is not a document;
/// - outside a document, an opening opens one;
/// - inside, the first `</html>` closes it, except one inside a `<script>` or
///   a `<style>` (up to its own `</script>` / `</style>`) or a comment (up to
///   `-->`), so a script string holding `<html>` or `</html>` is part of the
///   page;
/// - inside, another doctype, or a second `<html>` tag outside a script, style
///   or comment, starts the document again from there: the one before never
///   closed;
/// - the last document closed wins; when the output ends inside a document
///   (no `</html>`, or a script or comment that never closes), it is cut.
fn bare_document(text: &str) -> Option<Result<&str, ExecutionHtml>> {
    let lower = text.to_ascii_lowercase();
    let mut last: Option<&str> = None;
    // (start of the document, whether its `<html>` tag was seen)
    let mut open: Option<(usize, bool)> = None;
    let mut at = 0;
    while let Some(found) = lower[at..].find('<') {
        let here = at + found;
        at = here + 1;
        let opening = opening_at(&lower, here).filter(|_| markup_follows(&lower, here));
        let Some((start, has_tag)) = open else {
            if let Some(opening) = opening {
                open = Some((here, opening == Opening::Tag));
            }
            continue;
        };
        match opening {
            Some(Opening::Doctype) => open = Some((here, false)),
            Some(Opening::Tag) if has_tag => open = Some((here, true)),
            Some(Opening::Tag) => open = Some((start, true)),
            None => {}
        }
        if opening.is_some() {
            continue;
        }
        if lower[here..].starts_with(CLOSE_HTML) {
            last = Some(&text[start..here + CLOSE_HTML.len()]);
            open = None;
            at = here + CLOSE_HTML.len();
            continue;
        }
        match raw_text_end(&lower, here) {
            Some(Some(end)) => at = end,
            Some(None) => return Some(Err(ExecutionHtml::CutNoClosingTag)),
            None => {}
        }
    }
    match (open, last) {
        (Some(_), _) => Some(Err(ExecutionHtml::CutNoClosingTag)),
        (None, Some(doc)) => Some(Ok(doc)),
        (None, None) => None,
    }
}

fn is_html_fence(info: &str) -> bool {
    info == "html" || info == "kronn-doc-preview"
}

/// The last HTML document of `output`.
pub fn find(output: &str) -> Found<'_> {
    let fenced: Vec<Fence<'_>> = fences(output)
        .into_iter()
        .filter(|fence| is_html_fence(&fence.info))
        .collect();
    let found = match fenced.last() {
        Some(fence) if !fence.closed => Found::Cut(ExecutionHtml::CutUnclosedFence),
        Some(fence) => match fenced_document(fence.body) {
            Some(Ok(doc)) => Found::Complete(doc),
            Some(Err(why)) => Found::Cut(why),
            // A fragment in a closed fence is whole: the fence closing says so.
            None if fence.body.trim().is_empty() => Found::None,
            None => Found::Complete(fence.body.trim()),
        },
        None => match bare_document(output) {
            Some(Ok(doc)) => Found::Complete(doc),
            Some(Err(why)) => Found::Cut(why),
            None => Found::None,
        },
    };
    let stored_cut = output
        .trim_end()
        .rsplit('\n')
        .next()
        .is_some_and(|last| last.starts_with(super::shadow::CUT_MARKER));
    match found {
        Found::Complete(_) if stored_cut => Found::Cut(ExecutionHtml::CutOutput),
        other => other,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    const PAGE: &str = "<!DOCTYPE html>\n<html lang=\"fr\"><body><canvas></canvas>é</body></html>";

    #[test]
    fn a_fenced_document_is_found() {
        let output = format!("Voici le jeu :\n\n```html\n{PAGE}\n```\n\nBon jeu !");
        assert_eq!(find(&output), Found::Complete(PAGE));
        let preview = format!("```kronn-doc-preview\n{PAGE}\n```");
        assert_eq!(find(&preview), Found::Complete(PAGE));
        let tilde = format!("~~~HTML\n{PAGE}\n~~~");
        assert_eq!(find(&tilde), Found::Complete(PAGE));
    }

    #[test]
    fn the_last_block_wins() {
        let first = "<html><body>v1</body></html>";
        let output = format!("```html\n{first}\n```\nthen\n```html\n{PAGE}\n```\n");
        assert_eq!(find(&output), Found::Complete(PAGE));
    }

    #[test]
    fn a_bare_document_is_found() {
        let output = format!("Here it is.\n{PAGE}\nEnjoy.");
        assert_eq!(find(&output), Found::Complete(PAGE));
        let two = format!("{PAGE}\nand a second one: <html><body>2</body></html>");
        assert_eq!(find(&two), Found::Complete("<html><body>2</body></html>"));
        let no_doctype = "x <HTML><body>1</body></HTML> y";
        assert_eq!(
            find(no_doctype),
            Found::Complete("<HTML><body>1</body></HTML>")
        );
    }

    #[test]
    fn a_closed_fence_with_a_fragment_is_whole() {
        let output = "```html\n<div id=\"game\"></div>\n<script>go()</script>\n```";
        assert_eq!(
            find(output),
            Found::Complete("<div id=\"game\"></div>\n<script>go()</script>")
        );
    }

    #[test]
    fn no_html_says_so() {
        assert_eq!(find(""), Found::None);
        assert_eq!(find("A plain review: 3 findings."), Found::None);
        assert_eq!(find("```json\n{\"a\": 1}\n```"), Found::None);
        assert_eq!(find("```html\n\n```"), Found::None);
        assert_eq!(find("an <htmlish> word"), Found::None);
        assert_eq!(
            ExecutionHtml::of("[Output removed by run retention]"),
            ExecutionHtml::None
        );
    }

    #[test]
    fn a_cut_document_is_refused() {
        let unclosed = format!("```html\n{}", &PAGE[..40]);
        assert_eq!(find(&unclosed), Found::Cut(ExecutionHtml::CutUnclosedFence));
        let no_close = "```html\n<!doctype html><html><body>half\n```";
        assert_eq!(find(no_close), Found::Cut(ExecutionHtml::CutNoClosingTag));
        let bare = "<html><body>half";
        assert_eq!(find(bare), Found::Cut(ExecutionHtml::CutNoClosingTag));
        let stored = format!(
            "```html\n{PAGE}\n```\ntrailing\n{}12 bytes]",
            super::super::shadow::CUT_MARKER
        );
        assert_eq!(find(&stored), Found::Cut(ExecutionHtml::CutOutput));
        for cut in [
            ExecutionHtml::CutUnclosedFence,
            ExecutionHtml::CutNoClosingTag,
            ExecutionHtml::CutOutput,
            ExecutionHtml::None,
        ] {
            assert!(cut.refusal().is_some());
        }
        assert_eq!(ExecutionHtml::Complete.refusal(), None);
    }

    /// KT-1999 — a script string holding an `<html>` document.
    const TEMPLATE_GAME: &str = "<!DOCTYPE html>\n<html><head><title>Game</title></head><body>\n\
        <canvas id=\"c\"></canvas>\n<script>\nconst tpl = \"<html><body>hi</body></html>\";\n\
        </script>\n</body></html>";
    /// KT-1999 — a script string holding `</html>` before the real end.
    const CLOSING_GAME: &str = "<!doctype html>\n<html><body><script>\n\
        const end = \"</html>\";\n<!-- not a comment in a script -->\n</script>\n\
        <style>p::after { content: \"</html>\"; }</style>\n\
        <!-- an old </html> -->\n<p>still the page</p>\n</body></html>";

    #[test]
    fn a_tag_in_a_script_string_does_not_cut_a_fenced_page() {
        for page in [TEMPLATE_GAME, CLOSING_GAME] {
            let output = format!("Le jeu :\n\n```html\n{page}\n```\n\nBon jeu !");
            assert_eq!(find(&output), Found::Complete(page), "{page}");
        }
        // The fence is the unit: what it holds before its first opening is
        // not the document, and an opening with no `</html>` after it is cut.
        let led = format!("```html\n<!-- page -->\n{TEMPLATE_GAME}\n```");
        assert_eq!(find(&led), Found::Complete(TEMPLATE_GAME));
        let no_close = "```html\n</html> stray\n<html><body>half\n```";
        assert_eq!(find(no_close), Found::Cut(ExecutionHtml::CutNoClosingTag));
    }

    #[test]
    fn a_tag_in_a_script_string_does_not_cut_a_bare_page() {
        for page in [TEMPLATE_GAME, CLOSING_GAME] {
            let output = format!("Here is the game.\n{page}\nEnjoy.");
            assert_eq!(find(&output), Found::Complete(page), "{page}");
            assert_eq!(find(page), Found::Complete(page), "{page}");
        }
        // Still the last document of the output, even when the first one
        // holds tags in its scripts.
        let two = format!("{CLOSING_GAME}\nand then {TEMPLATE_GAME}\n");
        assert_eq!(find(&two), Found::Complete(TEMPLATE_GAME));
        let two = format!("{TEMPLATE_GAME}\nand then {CLOSING_GAME}\n");
        assert_eq!(find(&two), Found::Complete(CLOSING_GAME));
        // A script that never closes leaves the document cut.
        let cut = "<html><body><script>const s = \"</html>\";";
        assert_eq!(find(cut), Found::Cut(ExecutionHtml::CutNoClosingTag));
        // A doctype named in the prose before the page is not its start.
        let prose = format!("Start with <!DOCTYPE html>, then:\n{TEMPLATE_GAME}");
        assert_eq!(find(&prose), Found::Complete(TEMPLATE_GAME));
        // A `<script>` named in the prose does not hide the page after it.
        let prose = format!("It uses one <script> tag:\n{CLOSING_GAME}\n");
        assert_eq!(find(&prose), Found::Complete(CLOSING_GAME));
    }

    /// KT-1999 (review) — prose after a complete bare page that names an
    /// opening does not reopen a document.
    #[test]
    fn an_opening_named_in_prose_after_a_bare_page_is_ignored() {
        let page = "<!DOCTYPE html>\n<html><body><h1>Game</h1></body></html>";
        let both = format!(
            "{page}\n\nNote: `<!DOCTYPE html>` and `<html lang=\"en\">` are used here for \
             standards mode."
        );
        assert_eq!(find(&both), Found::Complete(page));
        let doctype = format!("{page}\n\nNote: `<!DOCTYPE html>` is used here for standards mode.");
        assert_eq!(find(&doctype), Found::Complete(page));
        // A real second document, cut, stays cut.
        let cut = format!("{PAGE}\n<!DOCTYPE html>\n<html><body>half");
        assert_eq!(find(&cut), Found::Cut(ExecutionHtml::CutNoClosingTag));
    }

    /// KT-1999 (review) — an `<html>` named in prose before a bare page, or in
    /// the page's own text, neither opens nor restarts a document.
    #[test]
    fn an_html_tag_named_in_prose_does_not_open_or_restart_a_bare_page() {
        let before = format!(
            "Everything lives in one `<html>` document with an inline `<style>` and a \
             `<script>`.\n{CLOSING_GAME}"
        );
        assert_eq!(find(&before), Found::Complete(CLOSING_GAME));
        let page = "<!doctype html>\n<html><body><p>The <html> tag wraps it</p></body></html>";
        assert_eq!(find(page), Found::Complete(page));
        let output = format!("Here:\n{page}\nDone.");
        assert_eq!(find(&output), Found::Complete(page));
    }

    /// KT-1999 (review) — a closed fence holding a fragment whose script holds
    /// a document template is the whole fragment, not the template.
    #[test]
    fn a_template_in_a_fenced_fragment_is_not_the_document() {
        let body = "<div id=\"app\"></div>\n<script>\n  const iframeDoc = \
                    \"<html><body>inner</body></html>\";\n</script>\n\
                    <style>#app { color: red; }</style>";
        let output = format!("```html\n{body}\n```");
        assert_eq!(find(&output), Found::Complete(body));
        // A script that never closes before any opening: no opening, so the
        // closed fence is a fragment, whole.
        let unclosed = "<p>x</p>\n<script>const s = \"<html></html>\";";
        assert_eq!(
            find(&format!("```html\n{unclosed}\n```")),
            Found::Complete(unclosed)
        );
    }

    #[test]
    fn a_fence_inside_another_does_not_close_it() {
        let output = format!("````html\n{PAGE}\n```\nstill inside\n````");
        assert_eq!(find(&output), Found::Complete(PAGE));
    }
}
