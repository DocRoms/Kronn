//! Origins a Live Page may embed third-party content from.
//!
//! A Page marks a placeholder with the full URL of what it wants to show
//! (`<div data-kronn-embed="https://player.example.com/embed/abc">`). The host
//! draws that content only when its origin (scheme, host and port, compared
//! exactly) is in this Kronn's `embed_allowed_origins`. An Artifact bundle
//! carries the origins its HTML declares, as information for the importer,
//! never as an authorization.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::LazyLock;

use html5ever::buffer_queue::BufferQueue;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

use reqwest::Url;

/// Upper bound on a stored or declared list; a Page needs a handful at most.
pub const MAX_EMBED_ORIGINS: usize = 256;
const MAX_EMBED_URL_CHARS: usize = 2048;

static EMBED_ATTRIBUTE: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r#"(?i)\bdata-kronn-embed\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .expect("valid embed attribute regex")
});

fn parse_web_url(raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().count() > MAX_EMBED_URL_CHARS {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url)
}

/// The origin (`scheme://host[:port]`) an embed URL loads from, or `None` when
/// it is not an http(s) URL Kronn would ever draw.
pub fn embed_url_origin(raw: &str) -> Option<String> {
    parse_web_url(raw).map(|url| url.origin().ascii_serialization())
}

/// Normalize an origin typed by the user. A trailing `/` is accepted; a path,
/// query, fragment or credentials are refused so the stored value is exactly
/// what gets compared.
pub fn normalize_origin(raw: &str) -> Result<String, String> {
    let url = parse_web_url(raw).ok_or_else(|| format!("Not an http(s) origin: {}", raw.trim()))?;
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "An origin has no path, query or fragment: {}",
            raw.trim()
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// Collects `data-kronn-embed` values, and the text of `script` elements,
/// from html5ever's tokenizer: the WHATWG tokenizer a browser runs, so values
/// come out exactly as the DOM sees them (quoted or not, every character
/// reference decoded, the first of duplicate attributes kept, comments and
/// raw-text elements not read as markup).
#[derive(Default)]
struct EmbedSink {
    values: RefCell<Vec<String>>,
    scripts: RefCell<Vec<String>>,
    in_script: Cell<bool>,
}

/// Enough for any real Page; bounds the work on a hostile one.
const MAX_EMBED_VALUES: usize = 4096;

impl TokenSink for EmbedSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        match token {
            Token::TagToken(tag) if tag.kind == TagKind::StartTag => {
                if let Some(attr) = tag
                    .attrs
                    .iter()
                    .find(|attr| &*attr.name.local == "data-kronn-embed")
                {
                    let mut values = self.values.borrow_mut();
                    if values.len() < MAX_EMBED_VALUES {
                        values.push(attr.value.to_string());
                    }
                }
                // What the tree builder tells the tokenizer for these elements:
                // their content is text, not tags.
                match &*tag.name {
                    "script" => {
                        self.in_script.set(true);
                        self.scripts.borrow_mut().push(String::new());
                        TokenSinkResult::RawData(RawKind::ScriptData)
                    }
                    "style" | "xmp" | "iframe" | "noembed" | "noframes" => {
                        TokenSinkResult::RawData(RawKind::Rawtext)
                    }
                    "textarea" | "title" => TokenSinkResult::RawData(RawKind::Rcdata),
                    "plaintext" => TokenSinkResult::Plaintext,
                    _ => TokenSinkResult::Continue,
                }
            }
            Token::TagToken(tag) => {
                if &*tag.name == "script" {
                    self.in_script.set(false);
                }
                TokenSinkResult::Continue
            }
            Token::CharacterTokens(text) if self.in_script.get() => {
                if let Some(script) = self.scripts.borrow_mut().last_mut() {
                    script.push_str(&text);
                }
                TokenSinkResult::Continue
            }
            _ => TokenSinkResult::Continue,
        }
    }
}

/// Every `data-kronn-embed` value of `html`'s start tags, as a browser reads
/// them, and the text of its `script` elements.
fn embed_attribute_values(html: &str) -> (Vec<String>, Vec<String>) {
    let tokenizer = Tokenizer::new(EmbedSink::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let sink = tokenizer.sink;
    (sink.values.into_inner(), sink.scripts.into_inner())
}

/// A quoted value spelled in a script, decoded as the markup the script
/// builds will be, once the browser parses it.
fn decode_scripted_value(raw: &str) -> Option<String> {
    let (values, _) = embed_attribute_values(&format!("<a data-kronn-embed=\"{raw}\">"));
    values.into_iter().next()
}

/// The distinct origins of the `data-kronn-embed` URLs in `html`, sorted: the
/// attributes of its markup, read as a browser reads them, plus quoted ones
/// spelled inside scripts that build markup at runtime (a hint; anything a
/// script assembles otherwise is only known if the Artifact declares it). The
/// host still checks every URL when it renders.
pub fn declared_origins(html: &str) -> Vec<String> {
    let (values, scripts) = embed_attribute_values(html);
    let scripted = scripts
        .iter()
        .flat_map(|script| EMBED_ATTRIBUTE.captures_iter(script))
        .filter_map(|captures| captures.get(1).or_else(|| captures.get(2)))
        .take(MAX_EMBED_VALUES)
        .filter_map(|value| decode_scripted_value(value.as_str()));
    let mut origins = BTreeSet::new();
    for value in values.into_iter().chain(scripted) {
        if let Some(origin) = embed_url_origin(&value) {
            origins.insert(origin);
            if origins.len() >= MAX_EMBED_ORIGINS {
                break;
            }
        }
    }
    origins.into_iter().collect()
}

/// Origins from a declaration (an imported Artifact's `embed_origins`, a
/// stored list): the valid ones, normalized, sorted, bounded. Anything else is
/// dropped: a declaration is information, never a reason to fail.
pub fn normalize_declared(declared: &[String]) -> Vec<String> {
    declared
        .iter()
        .filter_map(|origin| normalize_origin(origin).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_EMBED_ORIGINS)
        .collect()
}

/// `current` with `add` merged in and `remove` taken out, normalized, without
/// duplicates and in insertion order. Fails on the first invalid origin so a
/// typo is reported instead of silently dropped.
pub fn apply_changes(
    current: &[String],
    add: &[String],
    remove: &[String],
) -> Result<Vec<String>, String> {
    let removed = remove
        .iter()
        .map(|origin| normalize_origin(origin))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut seen = BTreeSet::new();
    let mut next = Vec::new();
    for origin in current {
        // A hand-edited config may hold a malformed entry: it can never match,
        // so it is dropped rather than blocking every later change.
        let Ok(origin) = normalize_origin(origin) else {
            continue;
        };
        if !removed.contains(&origin) && seen.insert(origin.clone()) {
            next.push(origin);
        }
    }
    for origin in add {
        let origin = normalize_origin(origin)?;
        if !removed.contains(&origin) && seen.insert(origin.clone()) {
            next.push(origin);
        }
    }
    if next.len() > MAX_EMBED_ORIGINS {
        return Err(format!("At most {MAX_EMBED_ORIGINS} sites can be allowed"));
    }
    // Only an addition is refused, so a read or a removal never loses sites.
    let frame_src_bytes = "'self'".len() + next.iter().map(|o| 1 + o.len()).sum::<usize>();
    if !add.is_empty() && frame_src_bytes > MAX_FRAME_SRC_BYTES {
        return Err(format!(
            "The allowed sites exceed {MAX_FRAME_SRC_BYTES} bytes once listed; remove some first"
        ));
    }
    Ok(next)
}

/// Bound on the `frame-src` value the Docker gateway copies into a header;
/// additions beyond it are refused, so an accepted list always fits.
pub const MAX_FRAME_SRC_BYTES: usize = 16 * 1024;

/// A normalized origin that is also a plain CSP host source: nothing that could
/// end the directive (`;`, `,`, whitespace, quotes) or that CSP cannot express
/// (`_`, IPv6 brackets).
fn is_csp_host_source(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.:".contains(&b))
    })
}

/// The `frame-src` sources of the app document: `'self'` (what `default-src`
/// already allowed) and exactly the allowed origins, each re-validated. The
/// Docker gateway reads it per document request, so a list change applies at
/// the next page load. CSP lets an `http://` source match its `https://`
/// upgrade too, so one is listed only when that upgrade is allowed as well.
pub fn frame_src_sources(allowed: &[String]) -> String {
    let mut sources = String::from("'self'");
    let origins = apply_changes(allowed, &[], &[]).unwrap_or_default();
    let upgrade_allowed = |origin: &String| {
        origin.strip_prefix("http://").is_none_or(|rest| {
            normalize_origin(&format!("https://{rest}"))
                .is_ok_and(|secure| origins.contains(&secure))
        })
    };
    for origin in origins
        .iter()
        .filter(|origin| is_csp_host_source(origin) && upgrade_allowed(origin))
    {
        if sources.len() + 1 + origin.len() > MAX_FRAME_SRC_BYTES {
            break;
        }
        sources.push(' ');
        sources.push_str(origin);
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_src_lists_exactly_the_allowed_origins() {
        assert_eq!(frame_src_sources(&[]), "'self'");
        let allowed = vec![
            "https://Suno.com/".to_string(),
            "http://localhost:3000".to_string(),
            "https://suno.com".to_string(),
            "https://player.example.com:8443".to_string(),
        ];
        assert_eq!(
            frame_src_sources(&allowed),
            "'self' https://suno.com https://player.example.com:8443"
        );
        assert!(!frame_src_sources(&allowed).contains('*'));
    }

    #[test]
    fn an_http_source_needs_its_https_upgrade_allowed_too() {
        let http_only = vec![
            "http://localhost:3000".to_string(),
            "http://a.example".to_string(),
        ];
        assert_eq!(frame_src_sources(&http_only), "'self'");
        let both = vec![
            "http://localhost:3000".to_string(),
            "https://localhost:3000".to_string(),
            "http://a.example".to_string(),
            "https://a.example:443".to_string(),
            "http://b.example:8080".to_string(),
            "https://b.example".to_string(),
        ];
        assert_eq!(
            frame_src_sources(&both),
            "'self' http://localhost:3000 https://localhost:3000 http://a.example https://a.example https://b.example"
        );
    }

    #[test]
    fn frame_src_never_carries_a_wildcard_or_a_directive_break() {
        let hostile = [
            "*",
            "https://*",
            "https://*.example.com",
            "https:",
            "'unsafe-inline'",
            "https://a.example; script-src *",
            "https://a.example;report-uri",
            "https://a.example,b.example",
            "https://a.example 'unsafe-eval'",
            "https://a\".example",
            "https://a'.example",
            "https://a_b.example",
            "http://[::1]:8080",
            "https://a.example\r\nX-Injected: 1",
            "data:",
            "blob:",
        ];
        for value in hostile {
            let sources = frame_src_sources(&[value.to_string()]);
            assert_eq!(sources, "'self'", "{value:?} reached frame-src");
        }
    }

    #[test]
    fn csp_host_source_check_holds_without_normalization() {
        for value in [
            "https://a.example;report-uri",
            "https://a.example b",
            "https://a.example'",
            "https://a.example\"",
            "https://a.example,b",
            "https://A.example",
            "https://*.example",
            "https://",
            "ftp://a.example",
        ] {
            assert!(!is_csp_host_source(value), "{value:?}");
        }
        assert!(is_csp_host_source("https://xn--bcher-kva.example:8443"));
    }

    #[test]
    fn every_accepted_origin_fits_in_frame_src() {
        let long: Vec<String> = (0..MAX_EMBED_ORIGINS)
            .map(|i| format!("https://{}{i}.example", "a".repeat(60)))
            .collect();
        assert!(apply_changes(&[], &long, &[]).is_err());
        let mut accepted = Vec::new();
        for origin in &long {
            match apply_changes(&accepted, std::slice::from_ref(origin), &[]) {
                Ok(next) => accepted = next,
                Err(_) => break,
            }
        }
        let sources = frame_src_sources(&accepted);
        assert!(sources.len() <= MAX_FRAME_SRC_BYTES);
        assert_eq!(sources.split(' ').count(), accepted.len() + 1);
        // A hand-edited list over the budget is still read and still shrinks.
        assert_eq!(apply_changes(&long, &[], &[]).unwrap().len(), long.len());
        assert!(apply_changes(&long, &[], &[long[0].clone()]).is_ok());
        assert!(frame_src_sources(&long).len() <= MAX_FRAME_SRC_BYTES);
        let typical: Vec<String> = (0..MAX_EMBED_ORIGINS)
            .map(|i| format!("https://site{i}.example"))
            .collect();
        assert_eq!(
            frame_src_sources(&typical).split(' ').count(),
            MAX_EMBED_ORIGINS + 1
        );
    }

    #[test]
    fn origin_compares_scheme_host_and_port_exactly() {
        assert_eq!(
            embed_url_origin("https://Suno.com/embed/abc?x=1#t").as_deref(),
            Some("https://suno.com")
        );
        assert_eq!(
            embed_url_origin("https://player.example.com:443/x").as_deref(),
            Some("https://player.example.com")
        );
        assert_eq!(
            embed_url_origin("https://player.example.com:8443/x").as_deref(),
            Some("https://player.example.com:8443")
        );
        assert_eq!(
            embed_url_origin("http://localhost:3000/embed").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            embed_url_origin("https://bücher.example/x").as_deref(),
            Some("https://xn--bcher-kva.example")
        );
    }

    #[test]
    fn non_web_or_credentialed_urls_have_no_origin() {
        for raw in [
            "",
            "   ",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "https://user:pass@example.com/x",
            "https://user@example.com/x",
            "//example.com/x",
            "/embed/abc",
            "https://",
        ] {
            assert_eq!(embed_url_origin(raw), None, "{raw}");
        }
        assert_eq!(
            embed_url_origin(&format!("https://example.com/{}", "a".repeat(3000))),
            None
        );
    }

    #[test]
    fn typed_origins_are_normalized_or_refused() {
        assert_eq!(
            normalize_origin(" https://Player.Example.com/ ").unwrap(),
            "https://player.example.com"
        );
        assert_eq!(
            normalize_origin("https://player.example.com").unwrap(),
            "https://player.example.com"
        );
        for raw in [
            "player.example.com",
            "https://player.example.com/embed",
            "https://player.example.com/?a=1",
            "https://player.example.com/#x",
            "https://u:p@player.example.com",
        ] {
            assert!(normalize_origin(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn declared_origins_reads_both_quote_styles_and_dedupes() {
        let html = r#"
            <div data-kronn-embed="https://suno.com/embed/a"></div>
            <div DATA-KRONN-EMBED = 'https://suno.com/embed/b'></div>
            <div data-kronn-embed="https://www.youtube-nocookie.com/embed/x?a=1&amp;b=2"></div>
            <div data-kronn-embed="javascript:alert(1)"></div>
            <div data-kronn-embed-id="ignored"></div>
            <div data-kronn-embed="https://u:p@evil.example/x"></div>
        "#;
        assert_eq!(
            declared_origins(html),
            vec![
                "https://suno.com".to_string(),
                "https://www.youtube-nocookie.com".to_string(),
            ]
        );
        assert!(declared_origins("<p>nothing</p>").is_empty());
    }

    #[test]
    fn declared_origins_read_attributes_as_a_browser_does() {
        // Unquoted (minified HTML), character references, any case, the first
        // occurrence on a tag winning: the same values the bridge sees in the DOM.
        let html = concat!(
            "<div data-kronn-embed=https://player.example/embed/123></div>",
            "<div DATA-KRONN-EMBED=https&#58;//entity.example/v?a=1&amp;b=2 class=x></div>",
            "<div data-kronn-embed=\"https&colon;&sol;&sol;named.example/x\"></div>",
            "<div data-kronn-embed=\"https://first.example/x\" data-kronn-embed=\"https://second.example/x\"></div>",
            "<p title='>' data-kronn-embed='https://after-gt.example/x'></p>",
            "<div\ndata-kronn-embed\n=\nhttps://spaced.example/x/></div>",
        );
        assert_eq!(
            declared_origins(html),
            vec![
                "https://after-gt.example",
                "https://entity.example",
                "https://first.example",
                "https://named.example",
                "https://player.example",
                "https://spaced.example",
            ]
        );
    }

    #[test]
    fn declared_origins_skip_what_is_not_markup_but_read_scripts_that_build_it() {
        let html = concat!(
            "<!-- <div data-kronn-embed=\"https://commented.example/x\"></div> -->",
            "<textarea><div data-kronn-embed=\"https://text.example/x\"></div></textarea>",
            "<style>/* data-kronn-embed=\"https://style.example/x\" */</style>",
            "<script>el.innerHTML = `<div data-kronn-embed=\"https://built.example/${id}\"></div>`;</script>",
            "<div data-kronn-embed=\"https://real.example/x\"></div>",
        );
        assert_eq!(
            declared_origins(html),
            vec!["https://built.example", "https://real.example"]
        );
    }

    #[test]
    fn references_decode_as_chromium_reads_them() {
        // The two forms from the review: a named reference outside the old
        // short list, and a numeric one without `;`. Chromium reads
        // `https://café.example/embed/123` and `https://player.example/embed/123`.
        assert_eq!(
            declared_origins(
                r#"<div data-kronn-embed="https://caf&eacute;.example/embed/123"></div>"#
            ),
            vec!["https://xn--caf-dma.example"]
        );
        assert_eq!(
            declared_origins("<div data-kronn-embed=https&#58//player.example/embed/123></div>"),
            vec!["https://player.example"]
        );
        assert_eq!(
            decode_scripted_value("x?a=1&ampb=2&amp;c=&#x41;&notit;").as_deref(),
            Some("x?a=1&ampb=2&c=A&notit;")
        );
        assert_eq!(
            decode_scripted_value("Été 🦀 &amp; ok").as_deref(),
            Some("Été 🦀 & ok")
        );
    }

    #[test]
    fn extraction_cost_stays_linear_on_hostile_values() {
        // The review's case: a 512 KB value of `&` with no `;` (and a 1 MiB
        // Page of them) must not take seconds.
        let ampersands = "&".repeat(512 * 1024);
        let started = std::time::Instant::now();
        assert!(declared_origins(&format!(
            "<div data-kronn-embed=\"https://x.example/{ampersands}\"></div>"
        ))
        .is_empty());
        let page = format!(
            "<script>el.innerHTML='<a data-kronn-embed=\"{ampersands}\">'</script>{}",
            "<p data-kronn-embed=\"&&&&\">".repeat(30_000)
        );
        assert!(declared_origins(&page).is_empty());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn declared_lists_are_normalized_and_never_fail() {
        assert_eq!(
            normalize_declared(&[
                "https://Runtime.example/".into(),
                "not an origin".into(),
                "https://runtime.example".into(),
                "https://a.example/path".into(),
            ]),
            vec!["https://runtime.example"]
        );
    }

    #[test]
    fn changes_merge_normalize_and_remove() {
        let current = vec!["https://suno.com".to_string(), "not an origin".to_string()];
        let next = apply_changes(
            &current,
            &[
                "https://Player.example.com/".to_string(),
                "https://suno.com".to_string(),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(next, vec!["https://suno.com", "https://player.example.com"]);
        let next = apply_changes(&next, &[], &["https://SUNO.com".to_string()]).unwrap();
        assert_eq!(next, vec!["https://player.example.com"]);
        assert!(apply_changes(&next, &["https://x.example/path".to_string()], &[]).is_err());
    }

    #[test]
    fn changes_are_bounded() {
        let many: Vec<String> = (0..=MAX_EMBED_ORIGINS)
            .map(|i| format!("https://site{i}.example"))
            .collect();
        assert!(apply_changes(&[], &many, &[]).is_err());
    }
}
