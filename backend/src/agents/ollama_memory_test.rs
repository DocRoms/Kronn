use super::*;
use crate::agents::runner::{
    ollama_machine_ceiling, parse_ollama_model_profile, ram_derived_ceiling,
    resolve_ctx_cap_for_model, resolve_ctx_cap_within, CtxCapOrigin, OLLAMA_NUM_CTX_FLOOR,
};
use serde_json::json;

const GIB: u64 = 1024 * 1024 * 1024;
/// What `qwen3.8:27b-mlx` takes on disk (nvfp4), as `/api/tags` reports it.
const QWEN_MLX_WEIGHTS: u64 = 18_200_000_000;
/// The model's trained window, as `/api/show` reports it.
const QWEN_MLX_WINDOW: u64 = 262_144;
/// 16 full-attention layers × 2 × 4 KV heads × 256 × 2 bytes.
const QWEN_MLX_KV_PER_TOKEN: u64 = 65_536;

/// The `config.json` of `qwen3.8:27b-mlx`, as read from its Ollama manifest:
/// 64 layers, every fourth one with full attention, the others linear.
fn qwen_hybrid_config() -> Value {
    let layer_types: Vec<&str> = (1..=64)
        .map(|n| {
            if n % 4 == 0 {
                "full_attention"
            } else {
                "linear_attention"
            }
        })
        .collect();
    json!({
        "model_type": "qwen3_5",
        "text_config": {
            "num_hidden_layers": 64,
            "full_attention_interval": 4,
            "num_key_value_heads": 4,
            "num_attention_heads": 24,
            "hidden_size": 5120,
            "head_dim": 256,
            "layer_types": layer_types,
        }
    })
}

/// A dense 70B: every layer caches, 8 KV heads of 128 (`hidden_size / heads`).
fn dense_70b_config() -> Value {
    json!({
        "num_hidden_layers": 80,
        "num_key_value_heads": 8,
        "num_attention_heads": 64,
        "hidden_size": 8192,
    })
}

fn apple(total: u64, cache: Option<KvCacheType>) -> MachineFacts {
    MachineFacts {
        total_ram_bytes: Some(total),
        gpu_budget_bytes: Some(gpu_budget_bytes(total, None)),
        kv_cache_type: cache,
    }
}

fn ceiling(
    machine: &MachineFacts,
    weights: Option<u64>,
    shape: Option<KvShape>,
    measured: Option<u64>,
) -> ModelCeiling {
    ceiling_for_model(&CeilingInputs {
        total_ram_bytes: machine.total_ram_bytes,
        gpu_budget_bytes: machine.gpu_budget_bytes,
        weights_bytes: weights,
        measured_kv_bytes_per_token: measured,
        shape,
        kv_cache_type: machine.kv_cache_type,
    })
}

fn hybrid_shape() -> KvShape {
    kv_shape_from_config(&qwen_hybrid_config()).expect("the hybrid fixture is complete")
}

// ── What one token of context costs ─────────────────────────────────────────

#[test]
fn a_hybrid_model_caches_only_its_full_attention_layers() {
    // 16 layers × 2 × 4 heads × 256 × 2 bytes = 64 KiB: where counting all 64
    // layers would say 256 KiB and make the model look four times heavier.
    let shape = hybrid_shape();
    assert_eq!(
        shape.bytes_per_token(KvCacheType::F16),
        QWEN_MLX_KV_PER_TOKEN
    );
    assert_eq!(
        shape.bytes_per_token(KvCacheType::F16) * 65_536,
        4 * GIB,
        "the cache of the old 65 536-token band, as measured"
    );
    assert_eq!(shape.bytes_per_token(KvCacheType::F16) * 262_144, 16 * GIB);
}

#[test]
fn the_interval_alone_gives_the_same_answer_as_the_layer_types() {
    let mut config = qwen_hybrid_config();
    config["text_config"]
        .as_object_mut()
        .unwrap()
        .remove("layer_types");
    assert_eq!(kv_shape_from_config(&config), Some(hybrid_shape()));
}

#[test]
fn layer_types_say_more_than_the_interval() {
    let mut config = qwen_hybrid_config();
    // Eight full-attention layers, wherever the interval claims sixteen.
    let kinds: Vec<&str> = (0..64)
        .map(|n| {
            if n < 8 {
                "full_attention"
            } else {
                "linear_attention"
            }
        })
        .collect();
    config["text_config"]["layer_types"] = json!(kinds);
    let shape = kv_shape_from_config(&config).unwrap();
    assert_eq!(shape.bytes_per_token(KvCacheType::F16), 32_768);
}

#[test]
fn a_layer_list_that_does_not_add_up_is_not_priced() {
    let mut short = qwen_hybrid_config();
    short["text_config"]["layer_types"] = json!(["full_attention", "linear_attention"]);
    assert_eq!(kv_shape_from_config(&short), None);

    let mut unknown = qwen_hybrid_config();
    let kinds: Vec<&str> = (0..64).map(|_| "mamba_state").collect();
    unknown["text_config"]["layer_types"] = json!(kinds);
    assert_eq!(
        kv_shape_from_config(&unknown),
        None,
        "a layer kind Kronn has never seen is not assumed to cost nothing"
    );
}

#[test]
fn a_dense_model_caches_every_layer() {
    // 80 layers × 2 × 8 heads × 128 × 2 bytes = 320 KiB per token.
    let shape = kv_shape_from_config(&dense_70b_config()).unwrap();
    assert_eq!(shape.bytes_per_token(KvCacheType::F16), 327_680);
    assert_eq!(
        shape.bytes_per_token(KvCacheType::F16) / QWEN_MLX_KV_PER_TOKEN,
        5,
        "the hybrid is five times lighter per token than a dense 70B"
    );
}

#[test]
fn an_incomplete_config_is_not_guessed_at() {
    for missing in ["num_hidden_layers", "num_key_value_heads"] {
        let mut config = dense_70b_config();
        config.as_object_mut().unwrap().remove(missing);
        assert_eq!(kv_shape_from_config(&config), None, "without {missing}");
    }
    // No head_dim, and nothing to derive one from.
    let mut config = dense_70b_config();
    config.as_object_mut().unwrap().remove("hidden_size");
    assert_eq!(kv_shape_from_config(&config), None);
    // A config of the wrong type is not a number of zero.
    let mut config = dense_70b_config();
    config["num_key_value_heads"] = json!("eight");
    assert_eq!(kv_shape_from_config(&config), None);
    assert_eq!(kv_shape_from_config(&json!({})), None);
    assert_eq!(kv_shape_from_config(&json!("not an object")), None);
}

#[test]
fn windowed_and_latent_attention_are_not_priced_per_token() {
    // Sliding window with no per-layer pattern written down.
    let mut windowed = dense_70b_config();
    windowed["sliding_window"] = json!(4096);
    assert_eq!(kv_shape_from_config(&windowed), None);
    // The same key switched off (Qwen2 ships it that way) changes nothing.
    windowed["use_sliding_window"] = json!(false);
    assert!(kv_shape_from_config(&windowed).is_some());
    windowed["layer_types"] = json!(vec!["sliding_attention"; 80]);
    assert_eq!(kv_shape_from_config(&windowed), None);
    // Multi-head latent attention caches one compressed vector per token.
    let mut latent = dense_70b_config();
    latent["kv_lora_rank"] = json!(512);
    assert_eq!(kv_shape_from_config(&latent), None);
}

#[test]
fn only_a_loopback_server_can_use_the_local_model_store_and_gpu() {
    for base in [
        "http://localhost:11434",
        "http://127.0.0.1:11434",
        "http://[::1]:11434",
    ] {
        assert!(server_is_local(base), "{base}");
    }
    for base in [
        "http://ollama.lan:11434",
        "http://192.168.1.2:11434",
        "http://localhost.example",
        "not a URL",
    ] {
        assert!(!server_is_local(base), "{base}");
    }
}

#[tokio::test]
async fn a_remote_model_does_not_inherit_this_macs_gpu_budget() {
    let profile = parse_ollama_model_profile(&json!({"model_info": {
        "general.architecture": "llama", "llama.block_count": 80,
        "llama.attention.head_count_kv": 8, "llama.attention.key_length": 128,
        "llama.attention.value_length": 128, "llama.context_length": 131072
    }}));
    let decided = ollama_machine_ceiling(
        &apple(64 * GIB, Some(KvCacheType::F16)),
        "http://ollama.lan:11434",
        "remote-dense",
        Some(&profile),
        Some(40_000_000_000),
    )
    .await;
    assert_eq!(decided, ModelCeiling::from(65_536));
}

#[test]
fn a_gguf_model_publishes_its_shape_in_api_show() {
    let show = json!({
        "model_info": {
            "general.architecture": "llama",
            "llama.block_count": 80,
            "llama.attention.head_count": 64,
            "llama.attention.head_count_kv": 8,
            "llama.embedding_length": 8192,
            "llama.context_length": 131_072,
        }
    });
    let shape = kv_shape_from_show(&show).unwrap();
    assert_eq!(shape.bytes_per_token(KvCacheType::F16), 327_680);

    // Explicit key and value lengths are used as written.
    let mut explicit = show.clone();
    explicit["model_info"]["llama.attention.key_length"] = json!(128);
    explicit["model_info"]["llama.attention.value_length"] = json!(128);
    assert_eq!(kv_shape_from_show(&explicit), Some(shape));
}

#[test]
fn a_gguf_hybrid_and_a_per_layer_count_are_read_from_api_show() {
    let hybrid = json!({
        "model_info": {
            "general.architecture": "qwen35",
            "qwen35.block_count": 64,
            "qwen35.full_attention_interval": 4,
            "qwen35.attention.head_count_kv": 4,
            "qwen35.attention.key_length": 256,
            "qwen35.attention.value_length": 256,
        }
    });
    assert_eq!(kv_shape_from_show(&hybrid), Some(hybrid_shape()));

    // One KV-head count per layer, 0 where a layer has no attention.
    let per_layer = json!({
        "model_info": {
            "general.architecture": "jamba",
            "jamba.block_count": 4,
            "jamba.attention.head_count_kv": [0, 8, 0, 8],
            "jamba.attention.key_length": 128,
            "jamba.attention.value_length": 128,
        }
    });
    let shape = kv_shape_from_show(&per_layer).unwrap();
    assert_eq!(shape.bytes_per_token(KvCacheType::F16), 16 * 2 * 128 * 2);

    let mut wrong_length = per_layer.clone();
    wrong_length["model_info"]["jamba.attention.head_count_kv"] = json!([0, 8]);
    assert_eq!(kv_shape_from_show(&wrong_length), None);
}

#[test]
fn api_show_without_a_shape_gives_none() {
    // What an MLX model's /api/show says: a window, no heads, no head_dim.
    assert_eq!(
        kv_shape_from_show(&json!({
            "details": { "format": "safetensors" },
            "model_info": { "qwen3_5.context_length": 262_144 },
        })),
        None
    );
    assert_eq!(kv_shape_from_show(&json!({})), None);
    let windowed = json!({
        "model_info": {
            "general.architecture": "gemma3",
            "gemma3.block_count": 34,
            "gemma3.attention.head_count_kv": 4,
            "gemma3.attention.key_length": 256,
            "gemma3.attention.value_length": 256,
            "gemma3.attention.sliding_window": 1024,
        }
    });
    assert_eq!(kv_shape_from_show(&windowed), None);
}

// ── The cache type of the server ────────────────────────────────────────────

#[test]
fn a_quantised_cache_costs_what_ggml_packs_it_into() {
    assert_eq!(KvCacheType::from_setting(None), Some(KvCacheType::F16));
    assert_eq!(KvCacheType::from_setting(Some("")), Some(KvCacheType::F16));
    assert_eq!(
        KvCacheType::from_setting(Some(" F16 ")),
        Some(KvCacheType::F16)
    );
    assert_eq!(
        KvCacheType::from_setting(Some("Q8_0")),
        Some(KvCacheType::Q8_0)
    );
    assert_eq!(
        KvCacheType::from_setting(Some("q4_0")),
        Some(KvCacheType::Q4_0)
    );
    assert_eq!(
        KvCacheType::from_setting(Some("q5_1")),
        None,
        "a width Kronn cannot price is not guessed"
    );

    // 16 × 4 heads × 512 = 32 768 values a token: blocks of 32 values, 34 bytes
    // (q8_0) or 18 (q4_0) each.
    let shape = hybrid_shape();
    assert_eq!(shape.bytes_per_token(KvCacheType::F16), 65_536);
    assert_eq!(shape.bytes_per_token(KvCacheType::Q8_0), 34_816);
    assert_eq!(shape.bytes_per_token(KvCacheType::Q4_0), 18_432);
}

// ── The memory the GPU may use ──────────────────────────────────────────────

#[test]
fn the_gpu_budget_is_the_wired_limit_or_three_quarters_of_memory() {
    let total = 64 * GIB;
    let three_quarters = gpu_budget_bytes(total, None);
    assert!(
        three_quarters.abs_diff(48 * GIB) < 1024,
        "≈ 48 GiB: {three_quarters}"
    );
    // `iogpu.wired_limit_mb = 0` is macOS's way of saying "the default".
    assert_eq!(gpu_budget_bytes(total, Some(0)), three_quarters);
    assert_eq!(gpu_budget_bytes(total, Some(56 * 1024)), 56 * GIB);
    // A limit above the machine is not a machine with more memory.
    assert_eq!(gpu_budget_bytes(total, Some(200 * 1024)), total);
}

#[test]
fn the_margin_is_a_tenth_of_the_budget_and_never_less_than_two_gib() {
    assert!(prefill_margin_bytes(48 * GIB).abs_diff(48 * GIB / 10) < 1024);
    assert_eq!(prefill_margin_bytes(8 * GIB), 2 * GIB);
    assert_eq!(prefill_margin_bytes(0), 2 * GIB);
}

#[test]
fn the_window_is_what_the_budget_leaves_after_weights_and_margin() {
    let budget = 48 * GIB;
    let margin = prefill_margin_bytes(budget);
    let kv = 65_536;
    assert_eq!(
        window_within_gpu_budget(budget, 18 * GIB, kv),
        (budget - 18 * GIB - margin) / kv
    );
    // Weights that fill the budget leave no window at all, and cannot underflow.
    assert_eq!(window_within_gpu_budget(budget, 60 * GIB, kv), 0);
    assert_eq!(window_within_gpu_budget(budget, budget - margin, kv), 0);
    // A cost of zero is a bug upstream, not an infinite window.
    assert_eq!(
        window_within_gpu_budget(budget, 18 * GIB, 0),
        budget - 18 * GIB - margin
    );
}

// ── The ceiling of one model on one machine ─────────────────────────────────

/// DoD 1 — a hybrid model on a 64 GB Mac is no longer held to the 64 GB band.
#[test]
fn a_hybrid_model_on_64_gib_gets_far_more_than_the_coarse_band() {
    let machine = apple(64 * GIB, Some(KvCacheType::F16));
    let computed = ceiling(&machine, Some(QWEN_MLX_WEIGHTS), Some(hybrid_shape()), None);
    assert_eq!(computed.basis, CeilingBasis::ModelEstimate);
    assert_eq!(
        computed.tokens,
        window_within_gpu_budget(
            gpu_budget_bytes(64 * GIB, None),
            QWEN_MLX_WEIGHTS,
            QWEN_MLX_KV_PER_TOKEN
        ),
        "(GPU budget − weights − margin) / cache per token"
    );
    assert!(
        (420_000..440_000).contains(&computed.tokens),
        "about 28 GB of cache room at 64 KiB a token: {}",
        computed.tokens
    );
    assert!(computed.tokens > ram_derived_ceiling(Some(64 * GIB)));

    // Nothing configured: the model's own window, in full, is what it runs at.
    let cap = resolve_ctx_cap_within(None, Some(QWEN_MLX_WINDOW), computed);
    assert_eq!(cap.value, QWEN_MLX_WINDOW);
    assert!(cap.value > 65_536);
    assert_eq!(cap.origin, CtxCapOrigin::ModelWindow);
    assert!(cap.throttle_notice("qwen3.8:27b-mlx").is_none());
}

/// Where the machine really is the limit, the figure is the model's own and says so.
#[test]
fn a_tighter_machine_is_held_to_what_this_model_costs_it() {
    let machine = apple(32 * GIB, Some(KvCacheType::F16));
    let computed = ceiling(&machine, Some(QWEN_MLX_WEIGHTS), Some(hybrid_shape()), None);
    assert_eq!(computed.basis, CeilingBasis::ModelEstimate);
    assert!(
        (70_000..80_000).contains(&computed.tokens),
        "{}",
        computed.tokens
    );
    assert!(
        computed.tokens > ram_derived_ceiling(Some(32 * GIB)),
        "the band would have said {}",
        ram_derived_ceiling(Some(32 * GIB))
    );

    let cap = resolve_ctx_cap_within(None, Some(QWEN_MLX_WINDOW), computed);
    assert_eq!(cap.value, computed.tokens);
    assert_eq!(
        cap.origin,
        CtxCapOrigin::ModelEstimate {
            model_limit: QWEN_MLX_WINDOW
        }
    );
    let notice = cap
        .throttle_notice("qwen3.8:27b-mlx")
        .expect("a model held below its window owes the reader a sentence");
    assert!(notice.contains(&QWEN_MLX_WINDOW.to_string()), "{notice}");
    assert!(notice.contains(&computed.tokens.to_string()), "{notice}");
    assert!(notice.contains("KRONN_OLLAMA_NUM_CTX_CAP"), "{notice}");
}

#[test]
fn the_ceiling_grows_with_the_machine_across_ram_bands() {
    let at = |total: u64| {
        ceiling(
            &apple(total, Some(KvCacheType::F16)),
            Some(QWEN_MLX_WEIGHTS),
            Some(hybrid_shape()),
            None,
        )
    };
    // 16 GiB: a 12 GiB budget cannot hold 18 GB of weights. The model gets the
    // floor — and the origin says it is this model that did not fit.
    let sixteen = at(16 * GIB);
    assert_eq!(sixteen.tokens, OLLAMA_NUM_CTX_FLOOR);
    assert_eq!(sixteen.basis, CeilingBasis::ModelEstimate);
    let (thirty_two, sixty_four, one_twenty_eight) = (at(32 * GIB), at(64 * GIB), at(128 * GIB));
    assert!(sixteen.tokens < thirty_two.tokens);
    assert!(thirty_two.tokens < sixty_four.tokens);
    assert!(sixty_four.tokens < one_twenty_eight.tokens);
    // 128 GiB: room for far more than the model can use.
    assert!(one_twenty_eight.tokens > QWEN_MLX_WINDOW);
}

#[test]
fn a_dense_model_gets_much_less_than_a_hybrid_of_the_same_weight() {
    let machine = apple(64 * GIB, Some(KvCacheType::F16));
    let dense = kv_shape_from_config(&dense_70b_config());
    let weights = 40_000_000_000;
    let dense_ceiling = ceiling(&machine, Some(weights), dense, None);
    let hybrid_ceiling = ceiling(&machine, Some(weights), Some(hybrid_shape()), None);
    assert_eq!(dense_ceiling.basis, CeilingBasis::ModelEstimate);
    assert!(
        (19_000..20_000).contains(&dense_ceiling.tokens),
        "320 KiB a token over ≈ 6 GB of room: {}",
        dense_ceiling.tokens
    );
    assert_eq!(
        hybrid_ceiling.tokens / dense_ceiling.tokens,
        5,
        "five times cheaper a token, five times the window"
    );
}

#[test]
fn a_quantised_server_cache_buys_a_longer_window() {
    let at = |cache| {
        ceiling(
            &apple(32 * GIB, cache),
            Some(QWEN_MLX_WEIGHTS),
            Some(hybrid_shape()),
            None,
        )
        .tokens
    };
    let (f16, q8, q4) = (
        at(Some(KvCacheType::F16)),
        at(Some(KvCacheType::Q8_0)),
        at(Some(KvCacheType::Q4_0)),
    );
    assert!(f16 < q8 && q8 < q4, "{f16} < {q8} < {q4}");
    // Not quite twice: a q8_0 block carries its scale.
    assert!(q8 > f16 * 18 / 10 && q8 < f16 * 2, "{q8} vs {f16}");
    assert!(q4 > f16 * 35 / 10, "{q4} vs {f16}");
}

#[test]
fn a_measured_cost_beats_the_computed_one() {
    let machine = apple(32 * GIB, Some(KvCacheType::F16));
    let by_shape = ceiling(&machine, Some(QWEN_MLX_WEIGHTS), Some(hybrid_shape()), None);
    let measured = ceiling(
        &machine,
        Some(QWEN_MLX_WEIGHTS),
        Some(hybrid_shape()),
        Some(2 * QWEN_MLX_KV_PER_TOKEN),
    );
    assert_eq!(measured.basis, CeilingBasis::ModelEstimate);
    assert!(measured.tokens.abs_diff(by_shape.tokens / 2) <= 1);
    // And it needs no shape at all.
    let alone = ceiling(
        &machine,
        Some(QWEN_MLX_WEIGHTS),
        None,
        Some(2 * QWEN_MLX_KV_PER_TOKEN),
    );
    assert_eq!(alone, measured);
}

/// DoD 2 — every missing input leaves the answer exactly where it was.
#[test]
fn missing_metadata_falls_back_to_the_ram_band_to_the_token() {
    for total in [8, 16, 32, 64, 128, 256] {
        let band = ModelCeiling::from(ram_derived_ceiling(Some(total * GIB)));
        assert_eq!(band.basis, CeilingBasis::MemoryBand);
        let machine = apple(total * GIB, Some(KvCacheType::F16));
        // No attention shape (an MLX model whose config.json cannot be read).
        assert_eq!(
            ceiling(&machine, Some(QWEN_MLX_WEIGHTS), None, None),
            band,
            "{total} GiB, no shape"
        );
        // No weights.
        assert_eq!(
            ceiling(&machine, None, Some(hybrid_shape()), None),
            band,
            "{total} GiB, no weights"
        );
        // A cache type Kronn cannot price.
        assert_eq!(
            ceiling(
                &apple(total * GIB, None),
                Some(QWEN_MLX_WEIGHTS),
                Some(hybrid_shape()),
                None
            ),
            band,
            "{total} GiB, unknown cache type"
        );
    }
    // A machine that will not say how much memory it has.
    let blind = MachineFacts {
        total_ram_bytes: None,
        gpu_budget_bytes: None,
        kv_cache_type: Some(KvCacheType::F16),
    };
    let blind_ceiling = ceiling(&blind, Some(QWEN_MLX_WEIGHTS), Some(hybrid_shape()), None);
    assert_eq!(blind_ceiling.tokens, ram_derived_ceiling(None));
    assert_eq!(blind_ceiling.basis, CeilingBasis::MemoryBand);

    // …and the origin shown is the old one, not an estimate nobody made.
    let cap = resolve_ctx_cap_within(
        None,
        Some(QWEN_MLX_WINDOW),
        ceiling(
            &apple(64 * GIB, Some(KvCacheType::F16)),
            Some(QWEN_MLX_WEIGHTS),
            None,
            None,
        ),
    );
    assert_eq!(cap.value, 65_536);
    assert_eq!(
        cap.origin,
        CtxCapOrigin::MachineCeiling {
            model_limit: QWEN_MLX_WINDOW
        }
    );
}

#[test]
fn off_apple_silicon_the_previous_rule_stands() {
    // No readable GPU budget: the shape is not consulted, whatever it says.
    let machine = MachineFacts {
        total_ram_bytes: Some(64 * GIB),
        gpu_budget_bytes: None,
        kv_cache_type: Some(KvCacheType::F16),
    };
    let without_measure = ceiling(&machine, Some(QWEN_MLX_WEIGHTS), Some(hybrid_shape()), None);
    assert_eq!(without_measure.tokens, 65_536);
    assert_eq!(without_measure.basis, CeilingBasis::MemoryBand);

    // A cost measured on the running server still tightens it, as before.
    let measured = ceiling(
        &machine,
        Some(QWEN_MLX_WEIGHTS),
        None,
        Some(QWEN_MLX_KV_PER_TOKEN),
    );
    assert_eq!(
        measured.tokens,
        crate::agents::runner::ram_ceiling_for_model(
            Some(64 * GIB),
            Some(QWEN_MLX_WEIGHTS),
            Some(QWEN_MLX_KV_PER_TOKEN)
        )
    );
    assert_eq!(measured.basis, CeilingBasis::ModelEstimate);
}

/// What the operator or the user chose is never second-guessed by an estimate.
#[test]
fn an_override_still_wins_over_the_estimate() {
    let tight = ceiling(
        &apple(32 * GIB, Some(KvCacheType::F16)),
        Some(QWEN_MLX_WEIGHTS),
        Some(hybrid_shape()),
        None,
    );
    let env = resolve_ctx_cap_within(Some("131072".into()), Some(QWEN_MLX_WINDOW), tight);
    assert_eq!(env.value, 131_072);
    assert_eq!(env.origin, CtxCapOrigin::OperatorOverride);

    let overrides = std::collections::HashMap::from([("qwen3.8:27b-mlx".to_string(), 200_000)]);
    let saved = resolve_ctx_cap_for_model(
        None,
        "qwen3.8:27b-mlx",
        &overrides,
        Some(QWEN_MLX_WINDOW),
        tight,
    );
    assert_eq!(saved.value, 200_000);
    assert_eq!(saved.origin, CtxCapOrigin::ModelOverride);
    // Another model has no override, and gets the estimate.
    let other =
        resolve_ctx_cap_for_model(None, "qwen3:8b", &overrides, Some(QWEN_MLX_WINDOW), tight);
    assert_eq!(other.value, tight.tokens);
}

#[test]
fn an_estimate_below_the_floor_is_raised_to_it_and_a_missing_window_stays_bounded() {
    let hopeless = ModelCeiling {
        tokens: 10,
        basis: CeilingBasis::ModelEstimate,
    };
    let cap = resolve_ctx_cap_within(None, Some(QWEN_MLX_WINDOW), hopeless);
    assert_eq!(cap.value, OLLAMA_NUM_CTX_FLOOR);
    // /api/show unanswered: the portable fallback, bounded by the same ceiling.
    let fallback = resolve_ctx_cap_within(None, None, hopeless);
    assert_eq!(fallback.origin, CtxCapOrigin::PortableFallback);
    assert_eq!(fallback.value, OLLAMA_NUM_CTX_FLOOR);
}

// ── Putting it together from an /api/show answer ────────────────────────────

#[tokio::test]
async fn the_ceiling_is_decided_from_the_show_answer_of_the_model() {
    let show = json!({
        "model_info": {
            "general.architecture": "llama",
            "llama.block_count": 80,
            "llama.attention.head_count": 64,
            "llama.attention.head_count_kv": 8,
            "llama.embedding_length": 8192,
            "llama.context_length": 131_072,
        }
    });
    let profile = parse_ollama_model_profile(&show);
    assert_eq!(profile.context_length(), Some(131_072));
    let machine = apple(64 * GIB, Some(KvCacheType::F16));
    let decided = ollama_machine_ceiling(
        &machine,
        "http://127.0.0.1:1",
        "kt943-dense-70b",
        Some(&profile),
        Some(40_000_000_000),
    )
    .await;
    assert_eq!(decided.basis, CeilingBasis::ModelEstimate);
    assert!(
        (19_000..20_000).contains(&decided.tokens),
        "{}",
        decided.tokens
    );
    let cap = resolve_ctx_cap_within(None, profile.context_length(), decided);
    assert_eq!(
        cap.origin,
        CtxCapOrigin::ModelEstimate {
            model_limit: 131_072
        }
    );

    // The same model on a machine whose GPU budget Kronn cannot read: the band.
    let elsewhere = MachineFacts {
        gpu_budget_bytes: None,
        ..machine
    };
    let unchanged = ollama_machine_ceiling(
        &elsewhere,
        "http://127.0.0.1:1",
        "kt943-dense-70b",
        Some(&profile),
        Some(40_000_000_000),
    )
    .await;
    assert_eq!(unchanged, ModelCeiling::from(65_536));
}

#[tokio::test]
async fn a_model_whose_metadata_cannot_be_found_gets_the_band() {
    // Ollama did not answer /api/show, and no store holds this name.
    let machine = apple(64 * GIB, Some(KvCacheType::F16));
    let decided = ollama_machine_ceiling(
        &machine,
        "http://127.0.0.1:1",
        "kt943-model-that-is-nowhere:1b",
        None,
        Some(QWEN_MLX_WEIGHTS),
    )
    .await;
    assert_eq!(decided, ModelCeiling::from(65_536));
}

// ── Finding the model's own config.json in Ollama's store ───────────────────

fn digest_hex() -> String {
    "ab".repeat(32)
}

fn write_store(root: &Path, model_path: &str, manifest: &Value, blob: Option<&Value>) {
    let manifest_path = root.join("manifests").join(model_path);
    std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    if let Some(blob) = blob {
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(
            root.join("blobs").join(format!("sha256-{}", digest_hex())),
            blob.to_string(),
        )
        .unwrap();
    }
}

fn mlx_manifest() -> Value {
    json!({
        "schemaVersion": 2,
        "layers": [
            { "name": "model-00001-of-00004.safetensors", "digest": format!("sha256:{}", "cd".repeat(32)) },
            { "name": "config.json", "digest": format!("sha256:{}", digest_hex()) },
        ]
    })
}

#[test]
fn an_mlx_models_shape_is_read_from_the_config_in_its_manifest() {
    let store = tempfile::tempdir().unwrap();
    write_store(
        store.path(),
        "registry.ollama.ai/library/qwen3.8/27b-mlx",
        &mlx_manifest(),
        Some(&qwen_hybrid_config()),
    );
    let config = read_model_config(&[store.path().to_path_buf()], "qwen3.8:27b-mlx")
        .expect("the manifest names a config.json and the blob is there");
    assert_eq!(kv_shape_from_config(&config), Some(hybrid_shape()));
}

#[test]
fn the_first_store_that_has_the_tag_is_the_one_read() {
    let empty = tempfile::tempdir().unwrap();
    let full = tempfile::tempdir().unwrap();
    write_store(
        full.path(),
        "registry.ollama.ai/library/qwen3.8/27b-mlx",
        &mlx_manifest(),
        Some(&qwen_hybrid_config()),
    );
    let stores = [empty.path().to_path_buf(), full.path().to_path_buf()];
    assert!(read_model_config(&stores, "qwen3.8:27b-mlx").is_some());
    // A name as typed with capitals is filed lowercase by the store.
    assert!(read_model_config(&stores, "Qwen3.8:27B-MLX").is_some());
    // Another tag is another manifest.
    assert!(read_model_config(&stores, "qwen3.8:27b").is_none());
}

#[test]
fn a_store_without_a_readable_config_gives_nothing() {
    let store = tempfile::tempdir().unwrap();
    let stores = [store.path().to_path_buf()];
    // No store at all, and a path that does not exist.
    assert!(read_model_config(&stores, "qwen3:8b").is_none());
    assert!(read_model_config(&[PathBuf::from("/nonexistent/kt943")], "qwen3:8b").is_none());
    assert!(read_model_config(&[], "qwen3:8b").is_none());

    // A GGUF model: a manifest, and no config.json layer in it.
    write_store(
        store.path(),
        "registry.ollama.ai/library/qwen3/8b",
        &json!({ "layers": [{ "name": "model", "digest": format!("sha256:{}", digest_hex()) }] }),
        None,
    );
    assert!(read_model_config(&stores, "qwen3:8b").is_none());

    // A manifest naming a config whose blob is gone.
    write_store(
        store.path(),
        "registry.ollama.ai/library/qwen3.8/27b-mlx",
        &mlx_manifest(),
        None,
    );
    assert!(read_model_config(&stores, "qwen3.8:27b-mlx").is_none());

    // A blob that is not JSON.
    std::fs::create_dir_all(store.path().join("blobs")).unwrap();
    std::fs::write(
        store
            .path()
            .join("blobs")
            .join(format!("sha256-{}", digest_hex())),
        "not json",
    )
    .unwrap();
    assert!(read_model_config(&stores, "qwen3.8:27b-mlx").is_none());
}

#[test]
fn a_file_larger_than_its_bound_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"num_hidden_layers": 64}"#).unwrap();
    assert!(read_json_bounded(&path, 1024).is_some());
    assert!(read_json_bounded(&path, 8).is_none());
}

#[test]
fn the_config_blob_is_named_only_by_a_well_formed_digest() {
    let manifest =
        |digest: &str| json!({ "layers": [{ "name": "config.json", "digest": digest }] });
    assert_eq!(
        config_blob_name(&manifest(&format!("sha256:{}", digest_hex()))),
        Some(format!("sha256-{}", digest_hex()))
    );
    let bad_digests = [
        "sha256:../../../etc/passwd".to_string(),
        "sha256:abc".to_string(),
        "md5:abababababababababababababababab".to_string(),
        format!("sha256:{}/x", "a".repeat(62)),
        format!("sha256:{}", "g".repeat(64)),
        String::new(),
    ];
    for bad in &bad_digests {
        assert_eq!(config_blob_name(&manifest(bad)), None, "{bad:?}");
    }
    assert_eq!(config_blob_name(&json!({})), None);
}

#[test]
fn a_model_name_becomes_a_manifest_path_and_nothing_else() {
    let one = |model: &str| manifest_paths(model);
    assert_eq!(
        one("qwen3.8:27b-mlx"),
        vec![PathBuf::from("registry.ollama.ai/library/qwen3.8/27b-mlx")]
    );
    assert_eq!(
        one("qwen3"),
        vec![PathBuf::from("registry.ollama.ai/library/qwen3/latest")]
    );
    assert_eq!(
        one("someone/model:tag"),
        vec![PathBuf::from("registry.ollama.ai/someone/model/tag")]
    );
    assert_eq!(
        one("hf.co/user/repo:Q4_K_M"),
        vec![
            PathBuf::from("hf.co/user/repo/Q4_K_M"),
            PathBuf::from("hf.co/user/repo/q4_k_m"),
        ]
    );
    // The name arrives in an HTTP request: no segment may leave the store.
    for hostile in [
        "",
        "..",
        "../x:1",
        "a/../b",
        "../../etc/passwd",
        "model:..",
        "model:../../x",
        "/etc/passwd",
        "a/b/c/d:tag",
        "model name:tag",
        "model\\evil:tag",
        "mod\0el",
    ] {
        assert!(manifest_paths(hostile).is_empty(), "{hostile:?}");
    }
}
