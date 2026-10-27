use axum::{extract::State, Json};
use serde::Deserialize;

use crate::agents::media_capabilities::MediaModelCapabilities;
use crate::models::{
    ApiResponse, ExternalApiConnection, MediaModality, MediaReferenceLimit,
    MediaReferenceLimitSource, MediaReferenceSupport,
};
use crate::AppState;

pub fn endpoint(connection: &ExternalApiConnection) -> String {
    connection
        .media_endpoint
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .or(connection.endpoint.as_ref())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .unwrap_or_default()
}

pub fn resolve(
    connection: &ExternalApiConnection,
    model: &str,
    modality: MediaModality,
    advertised: Option<MediaModelCapabilities>,
    limits: &[MediaReferenceLimit],
) -> MediaModelCapabilities {
    let local = crate::api::comfyui::is_local(connection);
    let mut caps = advertised.unwrap_or_else(|| MediaModelCapabilities {
        model: model.into(),
        modality,
        durations_secs: Vec::new(),
        resolutions: Vec::new(),
        aspect_ratios: Vec::new(),
        frame_positions: Vec::new(),
        max_input_references: None,
        reference_support: None,
        generate_audio: None,
    });
    let mut support = caps
        .reference_support
        .take()
        .unwrap_or(MediaReferenceSupport {
            workflow: None,
            source: if caps.max_input_references.is_some() {
                MediaReferenceLimitSource::Provider
            } else {
                MediaReferenceLimitSource::Unknown
            },
            advertised_max: caps.max_input_references,
            configured_max: None,
            blocker: None,
        });
    support.configured_max = limits
        .iter()
        .find(|limit| {
            limit.connection_id == connection.id
                && limit.endpoint == endpoint(connection)
                && limit.workflow == support.workflow
                && limit.model == model
                && limit.modality == modality
        })
        .map(|limit| limit.max_input_references);
    if let Some(configured) = support.configured_max {
        caps.max_input_references = Some(
            caps.max_input_references
                .map_or(configured, |known| known.min(configured)),
        );
        support.source = MediaReferenceLimitSource::Configured;
    }
    if !local {
        let codec = crate::agents::media_codec::codec_for(connection.origin_preset);
        if !codec.accepts_reference_images() {
            caps.max_input_references = Some(0);
            caps.frame_positions.clear();
            support.blocker = Some("Kronn's adapter for this connection does not implement source-image transport yet. A configured limit cannot enable it.".into());
        } else if modality == MediaModality::Video {
            caps.max_input_references = Some(0);
            support.blocker = Some("Kronn's video adapter for this connection currently supports input frames only, not multiple visual references. Use a compatible ComfyUI Wan VACE workflow for multiple references.".into());
        }
    }
    caps.reference_support = Some(support);
    caps
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SetMediaReferenceLimitRequest {
    pub connection_id: String,
    pub model: String,
    pub modality: MediaModality,
    pub max_input_references: Option<u32>,
}

pub async fn set_limit(
    State(state): State<AppState>,
    Json(req): Json<SetMediaReferenceLimitRequest>,
) -> Json<ApiResponse<super::media::MediaModelCapabilitiesResponse>> {
    if req.max_input_references.is_some_and(|maximum| maximum > 64) {
        return Json(ApiResponse::err("reference limit must be between 0 and 64"));
    }
    let connection = match super::media::connection_by_id(&state, &req.connection_id).await {
        Ok(Some(connection)) => connection,
        Ok(None) => return Json(ApiResponse::err("unknown media connection")),
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    if super::media::configured_media_model(&connection, req.modality).as_deref()
        != Some(req.model.as_str())
    {
        return Json(ApiResponse::err(
            "the configured model changed; reload its reference settings",
        ));
    }
    let current =
        super::media::advertised_capabilities(&state, &connection, &req.model, req.modality).await;
    let known_max = current
        .as_ref()
        .and_then(|caps| caps.reference_support.as_ref())
        .and_then(|support| support.advertised_max);
    if let (Some(configured), Some(known)) = (req.max_input_references, known_max) {
        if configured > known {
            return Json(ApiResponse::err(format!(
                "this provider or workflow advertises at most {known} reference image(s)"
            )));
        }
    }
    let latest = match super::media::connection_by_id(&state, &req.connection_id).await {
        Ok(Some(latest)) => latest,
        _ => {
            return Json(ApiResponse::err(
                "the media connection changed; reload its reference settings",
            ))
        }
    };
    if endpoint(&latest) != endpoint(&connection)
        || super::media::configured_media_model(&latest, req.modality).as_deref()
            != Some(req.model.as_str())
    {
        return Json(ApiResponse::err(
            "the configured model or endpoint changed; reload its reference settings",
        ));
    }
    let workflow = current
        .as_ref()
        .and_then(|caps| caps.reference_support.as_ref())
        .and_then(|support| support.workflow.clone());
    let mut config = state.config.write().await;
    if crate::api::comfyui::is_local(&connection) {
        let family =
            crate::api::comfyui::reference_family(&config.comfyui, &req.model, req.modality);
        if crate::core::comfyui_references::workflow_id(family) != workflow.as_deref() {
            return Json(ApiResponse::err(
                "the configured workflow changed; reload its reference settings",
            ));
        }
    }
    let mut next = config.clone();
    let base = endpoint(&connection);
    next.media_reference_limits.retain(|limit| {
        !(limit.connection_id == connection.id
            && limit.endpoint == base
            && limit.workflow == workflow
            && limit.model == req.model
            && limit.modality == req.modality)
    });
    if let Some(maximum) = req.max_input_references {
        next.media_reference_limits.push(MediaReferenceLimit {
            connection_id: connection.id.clone(),
            endpoint: base,
            workflow,
            model: req.model.clone(),
            modality: req.modality,
            max_input_references: maximum,
        });
    }
    if let Err(error) = crate::core::config::save(&next).await {
        return Json(ApiResponse::err(format!(
            "reference settings were not saved: {error}"
        )));
    }
    *config = next;
    drop(config);
    Json(ApiResponse::ok(
        super::media::MediaModelCapabilitiesResponse {
            model: req.model.clone(),
            capabilities: super::media::advertised_capabilities(
                &state,
                &connection,
                &req.model,
                req.modality,
            )
            .await,
        },
    ))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::ExternalApiConnectionPreset;

    fn connection() -> ExternalApiConnection {
        serde_json::from_value(serde_json::json!({
            "id":"conn", "display_name":"Provider", "mention_alias":"provider",
            "endpoint":"https://provider.invalid/v1/", "credential_slug":"conn",
            "origin_preset":"open_router", "created_at":"2026-10-20T00:00:00Z",
            "updated_at":"2026-10-20T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn declarations_are_scoped_and_never_raise_known_limits() {
        let conn = connection();
        let limit = MediaReferenceLimit {
            connection_id: conn.id.clone(),
            endpoint: endpoint(&conn),
            workflow: None,
            model: "model".into(),
            modality: MediaModality::Image,
            max_input_references: 3,
        };
        let automatic = resolve(&conn, "model", MediaModality::Image, None, &[]);
        assert_eq!(automatic.max_input_references, None);
        assert_eq!(
            automatic.reference_support.as_ref().unwrap().source,
            MediaReferenceLimitSource::Unknown
        );
        let declared = resolve(
            &conn,
            "model",
            MediaModality::Image,
            None,
            std::slice::from_ref(&limit),
        );
        assert_eq!(declared.max_input_references, Some(3));
        let mut advertised = automatic.clone();
        advertised.reference_support = None;
        advertised.max_input_references = Some(2);
        assert_eq!(
            resolve(
                &conn,
                "model",
                MediaModality::Image,
                Some(advertised),
                std::slice::from_ref(&limit)
            )
            .max_input_references,
            Some(2)
        );
        for property in ["connection", "endpoint", "model", "modality", "workflow"] {
            let mut other = limit.clone();
            match property {
                "connection" => other.connection_id = "other".into(),
                "endpoint" => other.endpoint = "https://another.invalid".into(),
                "model" => other.model = "other".into(),
                "modality" => other.modality = MediaModality::Video,
                _ => other.workflow = Some("other-workflow".into()),
            }
            assert_eq!(
                resolve(&conn, "model", MediaModality::Image, None, &[other]).max_input_references,
                None,
                "{property}"
            );
        }
        let mut disabled = limit;
        disabled.max_input_references = 0;
        assert_eq!(
            resolve(&conn, "model", MediaModality::Image, None, &[disabled]).max_input_references,
            Some(0)
        );
    }

    #[test]
    fn declared_limits_cannot_invent_an_adapter_or_remove_a_workflow_blocker() {
        let mut conn = connection();
        conn.origin_preset = ExternalApiConnectionPreset::Nvidia;
        let limit = MediaReferenceLimit {
            connection_id: conn.id.clone(),
            endpoint: endpoint(&conn),
            workflow: None,
            model: "m".into(),
            modality: MediaModality::Image,
            max_input_references: 3,
        };
        let caps = resolve(&conn, "m", MediaModality::Image, None, &[limit]);
        assert_eq!(caps.max_input_references, Some(0));
        assert!(caps
            .reference_support
            .unwrap()
            .blocker
            .unwrap()
            .contains("adapter"));
        conn.id = "local:comfyui".into();
        let mut unavailable = resolve(&conn, "m", MediaModality::Image, None, &[]);
        unavailable.max_input_references = Some(0);
        unavailable.reference_support = Some(MediaReferenceSupport {
            workflow: Some("flux-redux-v1".into()),
            source: MediaReferenceLimitSource::Workflow,
            advertised_max: Some(3),
            configured_max: None,
            blocker: Some("Redux is missing".into()),
        });
        let limit = MediaReferenceLimit {
            connection_id: conn.id.clone(),
            endpoint: endpoint(&conn),
            workflow: Some("flux-redux-v1".into()),
            model: "m".into(),
            modality: MediaModality::Image,
            max_input_references: 3,
        };
        let caps = resolve(
            &conn,
            "m",
            MediaModality::Image,
            Some(unavailable),
            &[limit],
        );
        assert_eq!(caps.max_input_references, Some(0));
        assert_eq!(
            caps.reference_support.unwrap().blocker.as_deref(),
            Some("Redux is missing")
        );
    }
}
