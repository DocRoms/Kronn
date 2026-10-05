//! GET/PUT /api/ui-preferences — the interface preferences that must survive
//! a change of browser origin (desktop port fallback, first launch, restart).
//!
//! The frontend decides which keys are synced (an allowlist without drafts,
//! queues or secrets); the server only bounds what it stores.

use axum::{extract::State, Json};

use crate::db::ui_preferences::{self, UiPreferences};
use crate::models::ApiResponse;
use crate::AppState;

/// Serialized size cap of the whole map.
pub const MAX_UI_PREFERENCES_BYTES: usize = 64 * 1024;
pub const MAX_UI_PREFERENCE_KEYS: usize = 200;
const MAX_KEY_CHARS: usize = 128;
const KEY_PREFIX: &str = "kronn:";

pub fn validate(values: &UiPreferences) -> Result<(), String> {
    if values.len() > MAX_UI_PREFERENCE_KEYS {
        return Err(format!(
            "Too many interface preferences ({} > {MAX_UI_PREFERENCE_KEYS})",
            values.len()
        ));
    }
    if let Some(key) = values
        .keys()
        .find(|key| !key.starts_with(KEY_PREFIX) || key.chars().count() > MAX_KEY_CHARS)
    {
        let shown: String = key.chars().take(40).collect();
        return Err(format!(
            "Invalid interface preference key '{shown}': expected '{KEY_PREFIX}…' of at most {MAX_KEY_CHARS} characters"
        ));
    }
    let size = serde_json::to_string(values).map_or(usize::MAX, |json| json.len());
    if size > MAX_UI_PREFERENCES_BYTES {
        return Err(format!(
            "Interface preferences too large ({size} bytes > {MAX_UI_PREFERENCES_BYTES})"
        ));
    }
    Ok(())
}

pub async fn get(State(state): State<AppState>) -> Json<ApiResponse<UiPreferences>> {
    match state.db.with_conn(ui_preferences::get).await {
        Ok(values) => Json(ApiResponse::ok(values)),
        Err(error) => Json(ApiResponse::err(format!(
            "Failed to read interface preferences: {error}"
        ))),
    }
}

/// Replaces the whole map: the client always sends its full synced snapshot.
pub async fn put(
    State(state): State<AppState>,
    Json(values): Json<UiPreferences>,
) -> Json<ApiResponse<()>> {
    if let Err(message) = validate(&values) {
        return Json(ApiResponse::err(message));
    }
    match state
        .db
        .with_conn(move |conn| ui_preferences::put(conn, &values))
        .await
    {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(error) => Json(ApiResponse::err(format!(
            "Failed to save interface preferences: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> UiPreferences {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn accepts_kronn_keys_with_unicode_values() {
        assert!(validate(&map(&[("kronn:theme", "nuit 🌙 é")])).is_ok());
        assert!(validate(&UiPreferences::new()).is_ok());
    }

    #[test]
    fn rejects_foreign_or_oversized_keys() {
        assert!(validate(&map(&[("theme", "dark")])).is_err());
        let long = format!("kronn:{}", "é".repeat(MAX_KEY_CHARS));
        assert!(validate(&map(&[(long.as_str(), "x")])).is_err());
    }

    #[test]
    fn rejects_too_many_keys_and_too_many_bytes() {
        let many: UiPreferences = (0..=MAX_UI_PREFERENCE_KEYS)
            .map(|i| (format!("kronn:k{i}"), "1".to_string()))
            .collect();
        assert!(validate(&many).is_err());
        let big = "x".repeat(MAX_UI_PREFERENCES_BYTES);
        assert!(validate(&map(&[("kronn:theme", big.as_str())])).is_err());
    }
}
