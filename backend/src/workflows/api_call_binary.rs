//! Binary response mode for `ApiCall` / `BatchApiCall` (`api_response: Binary`).
//!
//! A 2xx body of a declared media type is returned as
//! `{content_type, size, base64, data_uri}` instead of being parsed as JSON.
//! The credentials still never leave the server: only the body comes back.
//! Two guards keep the broker from becoming a file proxy: the media type must
//! match the step's `accept` list, and a body over `max_bytes` is refused.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};

use crate::models::{ApiResponseMode, PaginationSpec, WorkflowStep};

/// Default body cap: Jira thumbnails weigh a few tens of KiB.
pub const DEFAULT_BINARY_MAX_BYTES: u64 = 256 * 1024;
/// Largest cap a step may declare; each byte is stored ~1.33x as base64.
pub const BINARY_MAX_BYTES_CEILING: u64 = 2 * 1024 * 1024;
const DEFAULT_ACCEPT: &str = "image/*";
const MAX_ACCEPT_ENTRIES: usize = 20;

/// Validated binary contract of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryPolicy {
    pub accept: Vec<String>,
    pub max_bytes: u64,
}

impl BinaryPolicy {
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn accepts(&self, media_type: &str) -> bool {
        let family = media_type.split('/').next().unwrap_or_default();
        self.accept
            .iter()
            .any(|entry| match entry.strip_suffix("/*") {
                Some(accepted_family) => accepted_family == family,
                None => entry == media_type,
            })
    }

    fn accept_label(&self) -> String {
        self.accept.join(", ")
    }
}

/// `Ok(None)` = JSON mode (field absent or `Json`). Used both when the
/// workflow is saved and before any request is sent.
pub fn resolve_binary_policy(step: &WorkflowStep) -> Result<Option<BinaryPolicy>, String> {
    let Some(ApiResponseMode::Binary { accept, max_bytes }) = step.api_response.as_ref() else {
        return Ok(None);
    };
    if matches!(
        step.api_pagination,
        Some(
            PaginationSpec::Offset { .. }
                | PaginationSpec::Cursor { .. }
                | PaginationSpec::Page { .. }
                | PaginationSpec::LinkHeader { .. }
        )
    ) {
        return Err("`api_response: Binary` returns one file; remove `api_pagination`.".into());
    }
    let max_bytes = max_bytes.unwrap_or(DEFAULT_BINARY_MAX_BYTES);
    if max_bytes == 0 || max_bytes > BINARY_MAX_BYTES_CEILING {
        return Err(format!(
            "`api_response.max_bytes` must be between 1 and {BINARY_MAX_BYTES_CEILING} bytes (got {max_bytes})."
        ));
    }
    if accept.len() > MAX_ACCEPT_ENTRIES {
        return Err(format!(
            "`api_response.accept` lists at most {MAX_ACCEPT_ENTRIES} media types."
        ));
    }
    let mut normalized = Vec::with_capacity(accept.len().max(1));
    for entry in accept {
        let entry = entry.trim().to_ascii_lowercase();
        if !is_accept_entry(&entry) {
            return Err(format!(
                "`api_response.accept` entry `{entry}` is not `type/subtype` or `type/*` \
                 (`*/*` is refused: declare the media types the step expects)."
            ));
        }
        if !normalized.contains(&entry) {
            normalized.push(entry);
        }
    }
    if normalized.is_empty() {
        normalized.push(DEFAULT_ACCEPT.to_string());
    }
    Ok(Some(BinaryPolicy {
        accept: normalized,
        max_bytes,
    }))
}

/// Read a 2xx body under `policy`. `Ok(None)` = empty body, handled by the
/// caller like an empty JSON success. The type is checked before download.
pub async fn read_binary_body(
    mut response: reqwest::Response,
    policy: &BinaryPolicy,
) -> Result<Option<Value>, String> {
    if response.content_length() == Some(0) {
        return Ok(None);
    }
    let declared = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let media_type = match declared.as_deref().and_then(media_type_of) {
        Some(media_type) if policy.accepts(&media_type) => media_type,
        Some(media_type) => {
            return Err(format!(
                "Binary response refused: Content-Type `{media_type}` is not accepted by this step \
                 (accept: {}).",
                policy.accept_label()
            ))
        }
        None => {
            return Err(format!(
                "Binary response refused: missing or invalid Content-Type (accept: {}).",
                policy.accept_label()
            ))
        }
    };
    if let Some(length) = response.content_length() {
        if length > policy.max_bytes {
            return Err(too_large(length, policy.max_bytes));
        }
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Binary response body read failed: {error}"))?
    {
        let total = (body.len() + chunk.len()) as u64;
        if total > policy.max_bytes {
            return Err(too_large(total, policy.max_bytes));
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Ok(None);
    }
    Ok(Some(binary_envelope(&media_type, &body)))
}

/// One-line summary that never embeds the payload itself.
pub fn binary_summary(value: &Value) -> Option<String> {
    let content_type = value.get("content_type")?.as_str()?;
    let size = value.get("size")?.as_u64()?;
    Some(format!("{content_type}, {size} bytes"))
}

fn binary_envelope(media_type: &str, body: &[u8]) -> Value {
    let base64 = STANDARD.encode(body);
    let data_uri = format!("data:{media_type};base64,{base64}");
    json!({
        "content_type": media_type,
        "size": body.len(),
        "base64": base64,
        "data_uri": data_uri,
    })
}

fn too_large(size: u64, max_bytes: u64) -> String {
    format!(
        "Binary response refused: body exceeds max_bytes ({size} > {max_bytes} bytes). \
         Raise `api_response.max_bytes` (at most {BINARY_MAX_BYTES_CEILING}) or request a smaller \
         rendition such as a thumbnail."
    )
}

/// `image/PNG; charset=x` → `image/png`, or `None` when it is not a concrete
/// `type/subtype` pair, so nothing unexpected reaches the data URI.
fn media_type_of(header: &str) -> Option<String> {
    let media_type = header.split(';').next()?.trim().to_ascii_lowercase();
    let (family, subtype) = media_type.split_once('/')?;
    (is_token(family) && is_token(subtype)).then_some(media_type)
}

fn is_accept_entry(entry: &str) -> bool {
    let Some((family, subtype)) = entry.split_once('/') else {
        return false;
    };
    is_token(family) && (subtype == "*" || is_token(subtype))
}

/// RFC 6838 restricted name.
fn is_token(part: &str) -> bool {
    let mut chars = part.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && part.len() <= 127
        && chars.all(|c| c.is_ascii_alphanumeric() || "!#$&^_.+-".contains(c))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn binary_step(accept: &[&str], max_bytes: Option<u64>) -> WorkflowStep {
        WorkflowStep {
            name: "thumb".into(),
            api_response: Some(ApiResponseMode::Binary {
                accept: accept.iter().map(|value| value.to_string()).collect(),
                max_bytes,
            }),
            ..WorkflowStep::default()
        }
    }

    #[test]
    fn absent_or_json_mode_keeps_the_json_path() {
        assert_eq!(resolve_binary_policy(&WorkflowStep::default()), Ok(None));
        let json = WorkflowStep {
            api_response: Some(ApiResponseMode::Json),
            ..WorkflowStep::default()
        };
        assert_eq!(resolve_binary_policy(&json), Ok(None));
    }

    #[test]
    fn binary_defaults_to_images_under_256_kib() {
        let policy = resolve_binary_policy(&binary_step(&[], None))
            .unwrap()
            .unwrap();
        assert_eq!(policy.max_bytes(), 256 * 1024);
        assert!(policy.accepts("image/png"));
        assert!(policy.accepts("image/jpeg"));
        assert!(!policy.accepts("application/pdf"));
        assert!(!policy.accepts("text/html"));
    }

    #[test]
    fn explicit_list_accepts_only_what_it_declares() {
        let policy = resolve_binary_policy(&binary_step(&[" Image/PNG ", "application/pdf"], None))
            .unwrap()
            .unwrap();
        assert!(policy.accepts("image/png"));
        assert!(policy.accepts("application/pdf"));
        assert!(!policy.accepts("image/jpeg"));
    }

    #[test]
    fn wildcard_and_malformed_types_are_refused() {
        for bad in ["*/*", "*", "image", "image/png; q=1", "/png", "image/", ""] {
            let error = resolve_binary_policy(&binary_step(&[bad], None)).unwrap_err();
            assert!(error.contains("accept"), "{bad}: {error}");
        }
    }

    #[test]
    fn cap_must_stay_between_one_byte_and_the_ceiling() {
        assert!(resolve_binary_policy(&binary_step(&[], Some(0))).is_err());
        let error = resolve_binary_policy(&binary_step(&[], Some(BINARY_MAX_BYTES_CEILING + 1)))
            .unwrap_err();
        assert!(error.contains("max_bytes"), "{error}");
        let policy = resolve_binary_policy(&binary_step(&[], Some(BINARY_MAX_BYTES_CEILING)))
            .unwrap()
            .unwrap();
        assert_eq!(policy.max_bytes(), BINARY_MAX_BYTES_CEILING);
    }

    #[test]
    fn multi_page_pagination_is_refused_for_a_single_file() {
        let mut step = binary_step(&[], None);
        step.api_pagination = Some(PaginationSpec::LinkHeader {
            page_size_param: None,
            page_size: None,
            max_pages: None,
        });
        assert!(resolve_binary_policy(&step)
            .unwrap_err()
            .contains("api_pagination"));
        step.api_pagination = Some(PaginationSpec::None);
        assert!(resolve_binary_policy(&step).unwrap().is_some());
    }

    #[test]
    fn response_media_type_is_normalized_before_reaching_the_data_uri() {
        assert_eq!(
            media_type_of("image/PNG; charset=binary").as_deref(),
            Some("image/png")
        );
        assert_eq!(media_type_of("image/png,text/html"), None);
        assert_eq!(media_type_of("image/*"), None);
        assert_eq!(media_type_of("garbage"), None);
    }

    #[test]
    fn envelope_carries_a_ready_data_uri_and_a_payload_free_summary() {
        let envelope = binary_envelope("image/png", &[0x89, b'P', b'N', b'G']);
        assert_eq!(envelope["size"], 4);
        assert_eq!(envelope["base64"], "iVBORw==");
        assert_eq!(envelope["data_uri"], "data:image/png;base64,iVBORw==");
        assert_eq!(
            binary_summary(&envelope).as_deref(),
            Some("image/png, 4 bytes")
        );
    }
}
