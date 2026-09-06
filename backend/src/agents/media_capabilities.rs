//! What a provider says one media model can actually do.
//!
//! The launcher used to offer a hard-coded list — 3 s, 1080p, four ratios —
//! against a catalogue that answers something else entirely: `seedance-2.0-mini`
//! accepts 4..15 s, only 480p/720p, and seven ratios. Offering 3 s meant
//! offering a click that the provider rejects, and hiding six ratios it does
//! support. Nothing here is inferred: a field the provider does not advertise
//! stays empty, and an empty field means "do not offer it", never "offer
//! everything".
//!
//! The two OpenRouter catalogues have DIFFERENT shapes, measured rather than
//! assumed:
//!   * `/v1/videos/models` is flat — `supported_durations`, `supported_resolutions`,
//!     `supported_aspect_ratios`, `supported_frame_images`, `generate_audio`;
//!   * `/v1/images/models` nests typed entries under `supported_parameters`
//!     (`aspect_ratio` as an enum, `input_references` as a range).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::models::{MediaModality, MediaReferenceMode};

/// Where a still image can be pinned in a generated clip. Only what the model
/// advertises; nothing is offered by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MediaFramePosition {
    FirstFrame,
    LastFrame,
}

impl MediaFramePosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstFrame => "first_frame",
            Self::LastFrame => "last_frame",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "first_frame" => Some(Self::FirstFrame),
            "last_frame" => Some(Self::LastFrame),
            _ => None,
        }
    }
}

/// The advertised envelope of one model, as the launcher must present it.
///
/// Every list is "what the provider named". An empty list is a statement — the
/// capability is not offered — and the UI must render it as such rather than
/// falling back to a default that would be rejected at submission time.
// No `Default`: an envelope without a modality describes nothing, and the
// only honest way to build one is to read a catalogue entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaModelCapabilities {
    pub model: String,
    pub modality: MediaModality,
    /// Seconds, exactly as advertised. Video only.
    pub durations_secs: Vec<u32>,
    pub resolutions: Vec<String>,
    pub aspect_ratios: Vec<String>,
    /// Where a source image may be pinned. Empty means this model does not
    /// take an input frame at all.
    pub frame_positions: Vec<MediaFramePosition>,
    /// How many reference images the model accepts. `None` means the provider
    /// says nothing, which is not the same as zero — but the UI treats both as
    /// "do not offer it" rather than guessing.
    pub max_input_references: Option<u32>,
    /// Whether the model itself advertises a soundtrack switch. A model that
    /// does not name it must not be shown an audio checkbox (KT-553).
    pub generate_audio: Option<bool>,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the flat video-catalogue entry.
pub fn video_capabilities(entry: &Value) -> MediaModelCapabilities {
    MediaModelCapabilities {
        model: entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        modality: MediaModality::Video,
        durations_secs: entry
            .get("supported_durations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|value| value as u32)
                    .collect()
            })
            .unwrap_or_default(),
        resolutions: strings(entry.get("supported_resolutions")),
        aspect_ratios: strings(entry.get("supported_aspect_ratios")),
        frame_positions: strings(entry.get("supported_frame_images"))
            .iter()
            .filter_map(|value| MediaFramePosition::parse(value))
            .collect(),
        // Videos advertise no reference-image count anywhere in this
        // catalogue: the only structured image input they name is a frame.
        max_input_references: None,
        generate_audio: entry.get("generate_audio").and_then(Value::as_bool),
    }
}

/// Reads the nested image-catalogue entry, whose parameters are typed.
pub fn image_capabilities(entry: &Value) -> MediaModelCapabilities {
    let parameters = entry.get("supported_parameters");
    let aspect_ratios = parameters
        .and_then(|parameters| parameters.get("aspect_ratio"))
        .filter(|ratio| ratio.get("type").and_then(Value::as_str) == Some("enum"))
        .map(|ratio| strings(ratio.get("values")))
        .unwrap_or_default();
    let max_input_references = parameters
        .and_then(|parameters| parameters.get("input_references"))
        .filter(|references| references.get("type").and_then(Value::as_str) == Some("range"))
        .and_then(|references| references.get("max"))
        .and_then(Value::as_u64)
        .map(|max| max as u32);
    MediaModelCapabilities {
        model: entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        modality: MediaModality::Image,
        durations_secs: Vec::new(),
        resolutions: Vec::new(),
        aspect_ratios,
        frame_positions: Vec::new(),
        max_input_references,
        generate_audio: None,
    }
}

/// Finds one model in a catalogue payload and reads what it advertises.
///
/// A model absent from the catalogue yields `None` rather than an empty
/// envelope: "this provider never mentioned that model" and "this model
/// supports nothing" are different answers, and only the first one should
/// make the caller fall back to what the operator configured by hand.
pub fn capabilities_for(
    body: &Value,
    model: &str,
    modality: MediaModality,
) -> Option<MediaModelCapabilities> {
    let entry = body
        .get("data")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(model))?;
    Some(match modality {
        MediaModality::Video => video_capabilities(entry),
        MediaModality::Image => image_capabilities(entry),
    })
}

impl MediaModelCapabilities {
    /// Whether this model advertises the requested use of a source image.
    ///
    /// A frame is checked against `supported_frame_images`; a visual reference
    /// against the reference count, because no video model lists "reference"
    /// as a frame — the two are different capabilities in both catalogues, and
    /// treating them as one would offer image-to-video on a model that only
    /// accepts reference pictures.
    pub fn supports_reference_mode(&self, mode: MediaReferenceMode) -> bool {
        match mode.frame_capability() {
            Some(frame) => self
                .frame_positions
                .iter()
                .any(|position| position.as_str() == frame),
            None => self.max_input_references.unwrap_or(0) > 0,
        }
    }

    /// What to name in a refusal, so the caller knows what to pick instead.
    pub fn advertised_modes(&self) -> Vec<&'static str> {
        let mut modes: Vec<&'static str> = self
            .frame_positions
            .iter()
            .map(|position| position.as_str())
            .collect();
        if self.max_input_references.unwrap_or(0) > 0 {
            modes.push("reference");
        }
        modes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Captured verbatim from `GET /api/v1/videos/models` on 2026-09-02.
    fn seedance() -> Value {
        json!({"data": [{
            "id": "bytedance/seedance-2.0-mini",
            "name": "ByteDance: Seedance 2.0 Mini",
            "supported_resolutions": ["480p", "720p"],
            "supported_aspect_ratios": ["1:1", "3:4", "9:16", "4:3", "16:9", "21:9", "9:21"],
            "supported_durations": [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            "supported_frame_images": ["first_frame", "last_frame"],
            "generate_audio": true,
            "seed": true
        }]})
    }

    /// Captured verbatim from `GET /api/v1/images/models` on 2026-09-02.
    fn nano_banana() -> Value {
        json!({"data": [{
            "id": "google/gemini-2.5-flash-image",
            "architecture": {"input_modalities": ["image", "text"], "output_modalities": ["image", "text"]},
            "supported_parameters": {
                "aspect_ratio": {"type": "enum", "values": ["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"]},
                "n": {"type": "range", "min": 1, "max": 1},
                "input_references": {"type": "range", "min": 0, "max": 3}
            }
        }]})
    }

    #[test]
    fn a_video_model_is_read_from_the_flat_catalogue() {
        let caps = capabilities_for(
            &seedance(),
            "bytedance/seedance-2.0-mini",
            MediaModality::Video,
        )
        .expect("the model is in the catalogue");
        // The launcher offered 3 s and 1080p, which this model rejects, and
        // hid six of the seven ratios it accepts.
        assert_eq!(caps.durations_secs.first(), Some(&4));
        assert_eq!(caps.durations_secs.last(), Some(&15));
        assert!(!caps.durations_secs.contains(&3));
        assert_eq!(caps.resolutions, vec!["480p", "720p"]);
        assert!(!caps.resolutions.iter().any(|value| value == "1080p"));
        assert_eq!(caps.aspect_ratios.len(), 7);
        assert_eq!(
            caps.frame_positions,
            vec![
                MediaFramePosition::FirstFrame,
                MediaFramePosition::LastFrame
            ]
        );
        assert_eq!(caps.generate_audio, Some(true));
    }

    #[test]
    fn an_image_model_is_read_from_the_nested_catalogue() {
        let caps = capabilities_for(
            &nano_banana(),
            "google/gemini-2.5-flash-image",
            MediaModality::Image,
        )
        .expect("the model is in the catalogue");
        assert_eq!(caps.aspect_ratios.len(), 10);
        assert_eq!(caps.max_input_references, Some(3));
        // Durations and frame positions belong to video; reading them here
        // would offer a clip's controls on a picture.
        assert!(caps.durations_secs.is_empty());
        assert!(caps.frame_positions.is_empty());
    }

    #[test]
    fn a_model_that_advertises_nothing_offers_nothing() {
        // Four of the 28 video models name no frame image at all. Treating
        // absence as "everything" would offer a mode the provider refuses.
        let body = json!({"data": [{"id": "runway/aleph-2", "supported_frame_images": null}]});
        let caps = capabilities_for(&body, "runway/aleph-2", MediaModality::Video).unwrap();
        assert!(caps.frame_positions.is_empty());
        assert!(caps.durations_secs.is_empty());
        assert!(caps.resolutions.is_empty());
        assert_eq!(caps.generate_audio, None);
    }

    #[test]
    fn an_unknown_model_is_absent_not_empty() {
        // "The provider never mentioned it" must stay distinguishable from
        // "it supports nothing": only the first lets the caller keep the
        // operator's own configuration.
        assert!(capabilities_for(&seedance(), "some/other-model", MediaModality::Video).is_none());
    }

    #[test]
    fn a_frame_mode_is_checked_against_frames_and_a_reference_against_references() {
        let video = capabilities_for(
            &seedance(),
            "bytedance/seedance-2.0-mini",
            MediaModality::Video,
        )
        .unwrap();
        assert!(video.supports_reference_mode(MediaReferenceMode::FirstFrame));
        assert!(video.supports_reference_mode(MediaReferenceMode::LastFrame));
        // No video model lists "reference" as a frame, and this one advertises
        // no reference count either: offering it would be image-to-video on a
        // capability the provider never named.
        assert!(!video.supports_reference_mode(MediaReferenceMode::Reference));
        assert_eq!(video.advertised_modes(), vec!["first_frame", "last_frame"]);

        let image = capabilities_for(
            &nano_banana(),
            "google/gemini-2.5-flash-image",
            MediaModality::Image,
        )
        .unwrap();
        assert!(image.supports_reference_mode(MediaReferenceMode::Reference));
        assert!(!image.supports_reference_mode(MediaReferenceMode::FirstFrame));
        assert_eq!(image.advertised_modes(), vec!["reference"]);
    }

    #[test]
    fn a_model_advertising_only_a_first_frame_refuses_the_last_one() {
        // 9 of the 28 video models are in exactly this case.
        let body =
            json!({"data": [{"id": "alibaba/wan-3.0", "supported_frame_images": ["first_frame"]}]});
        let caps = capabilities_for(&body, "alibaba/wan-3.0", MediaModality::Video).unwrap();
        assert!(caps.supports_reference_mode(MediaReferenceMode::FirstFrame));
        assert!(!caps.supports_reference_mode(MediaReferenceMode::LastFrame));
    }

    #[test]
    fn an_unparseable_frame_position_is_dropped_rather_than_guessed() {
        let body = json!({"data": [{
            "id": "m", "supported_frame_images": ["first_frame", "middle_frame"]
        }]});
        let caps = capabilities_for(&body, "m", MediaModality::Video).unwrap();
        assert_eq!(caps.frame_positions, vec![MediaFramePosition::FirstFrame]);
    }
}
