use super::*;

fn agents() -> AgentsConfig {
    crate::core::config::default_config().agents
}

fn to(connection: &str) -> ConnectionFallbackTarget {
    ConnectionFallbackTarget {
        connection: connection.to_string(),
        model: None,
    }
}

#[test]
fn only_the_media_modes_have_a_media_row() {
    let config = agents();
    for mode in [
        FallbackMode::Eco,
        FallbackMode::Standard,
        FallbackMode::Advanced,
        FallbackMode::Decision,
    ] {
        assert_eq!(
            validate_row(&config, COMFYUI_OWNER, mode, &to("conn-a")),
            Err(ConnectionChainError::NotMedia(mode))
        );
    }
    assert!(validate_row(&config, COMFYUI_OWNER, FallbackMode::Image, &to("conn-a")).is_ok());
    assert!(validate_row(&config, COMFYUI_OWNER, FallbackMode::Video, &to("conn-a")).is_ok());
}

#[test]
fn a_source_is_never_its_own_fallback() {
    let config = agents();
    assert_eq!(
        validate_row(&config, "conn-a", FallbackMode::Image, &to("conn-a")),
        Err(ConnectionChainError::SelfFallback("conn-a".into()))
    );
}

#[test]
fn a_bad_model_is_refused() {
    let config = agents();
    for model in ["  ", "a\nb"] {
        let target = ConnectionFallbackTarget {
            connection: "conn-a".into(),
            model: Some(model.into()),
        };
        assert!(matches!(
            validate_row(&config, COMFYUI_OWNER, FallbackMode::Image, &target),
            Err(ConnectionChainError::BadModel(_))
        ));
    }
}

#[test]
fn a_loop_through_the_rows_of_the_fallbacks_is_refused() {
    let mut config = agents();
    set_row(&mut config, "a", FallbackMode::Image, Some(to("b")));
    set_row(&mut config, "b", FallbackMode::Image, Some(to("c")));
    let error = validate_row(&config, "c", FallbackMode::Image, &to("a")).unwrap_err();
    assert!(matches!(error, ConnectionChainError::Cycle(_)), "{error}");
    // The video rows are another chain: the same pair is fine there.
    assert!(validate_row(&config, "c", FallbackMode::Video, &to("a")).is_ok());
}

#[test]
fn a_chain_past_the_depth_limit_is_refused() {
    let mut config = agents();
    // b → c → d → e → f is 4 links, fine on its own.
    for pair in ["b", "c", "d", "e", "f"].windows(2) {
        set_row(&mut config, pair[0], FallbackMode::Image, Some(to(pair[1])));
    }
    // a → b in front of it makes 5.
    let error = validate_row(&config, "a", FallbackMode::Image, &to("b")).unwrap_err();
    assert_eq!(error, ConnectionChainError::TooDeep(5));
}

#[test]
fn rows_are_replaced_removed_and_sorted() {
    let mut config = agents();
    set_row(&mut config, "b", FallbackMode::Video, Some(to("x")));
    set_row(&mut config, "a", FallbackMode::Image, Some(to("y")));
    set_row(&mut config, "a", FallbackMode::Image, Some(to("z")));
    assert_eq!(config.connection_fallbacks.len(), 2);
    assert_eq!(config.connection_fallbacks[0].owner, "a");
    assert_eq!(row(&config, "a", FallbackMode::Image), Some(&to("z")));
    set_row(&mut config, "a", FallbackMode::Image, None);
    assert!(row(&config, "a", FallbackMode::Image).is_none());
    assert_eq!(config.connection_fallbacks.len(), 1);
}

#[test]
fn the_walk_follows_the_row_and_skips_what_it_already_left() {
    let mut config = agents();
    set_row(
        &mut config,
        COMFYUI_OWNER,
        FallbackMode::Image,
        Some(to("a")),
    );
    set_row(&mut config, "a", FallbackMode::Image, Some(to("b")));
    assert_eq!(
        next_target(&config, COMFYUI_OWNER, FallbackMode::Image, &[]),
        Some(&to("a"))
    );
    assert_eq!(
        next_target(&config, "a", FallbackMode::Image, &[COMFYUI_OWNER.into()]),
        Some(&to("b"))
    );
    // A loop written by hand is not walked: b → a again.
    set_row(&mut config, "b", FallbackMode::Image, Some(to("a")));
    assert_eq!(
        next_target(&config, "b", FallbackMode::Image, &["a".into(), "b".into()]),
        None
    );
    // The video mode has no row.
    assert_eq!(
        next_target(&config, COMFYUI_OWNER, FallbackMode::Video, &[]),
        None
    );
}

#[test]
fn the_walk_stops_at_the_depth_limit() {
    let mut config = agents();
    set_row(&mut config, "a", FallbackMode::Image, Some(to("b")));
    let left: Vec<String> = (0..MAX_CHAIN_DEPTH).map(|n| format!("x{n}")).collect();
    assert_eq!(next_target(&config, "a", FallbackMode::Image, &left), None);
}

#[test]
fn the_view_shows_the_link_its_transport_and_the_inherited_links() {
    let mut config = agents();
    set_row(
        &mut config,
        COMFYUI_OWNER,
        FallbackMode::Image,
        Some(to("a")),
    );
    set_row(
        &mut config,
        "a",
        FallbackMode::Image,
        Some(to(COMFYUI_OWNER)),
    );
    // A hand-written loop: the owner comes back to itself.
    let view = chain_view(&config, COMFYUI_OWNER, FallbackMode::Image);
    assert_eq!(view.links.len(), 1);
    assert_eq!(view.links[0].transport, FallbackTransport::Http);
    assert!(
        view.inherited.is_empty(),
        "the walk does not return to ComfyUI"
    );
    assert!(
        view.error.as_deref().is_some_and(|e| e.contains("loop")),
        "{view:?}"
    );

    let mut config = agents();
    set_row(
        &mut config,
        "a",
        FallbackMode::Video,
        Some(to(COMFYUI_OWNER)),
    );
    set_row(
        &mut config,
        COMFYUI_OWNER,
        FallbackMode::Video,
        Some(to("b")),
    );
    let view = chain_view(&config, "a", FallbackMode::Video);
    assert_eq!(view.links[0].transport, FallbackTransport::Local);
    assert_eq!(view.inherited.len(), 1);
    assert_eq!(view.inherited[0].connection, "b");
    assert!(view.error.is_none());
    assert_eq!(overview(&config).len(), 2);
}

#[test]
fn the_rows_round_trip_through_the_config_file() {
    let mut config = crate::core::config::default_config();
    set_row(
        &mut config.agents,
        COMFYUI_OWNER,
        FallbackMode::Image,
        Some(ConnectionFallbackTarget {
            connection: "conn-a".into(),
            model: Some("flux".into()),
        }),
    );
    let text = toml::to_string(&config).unwrap();
    assert!(text.contains("[[agents.connection_fallbacks]]"), "{text}");
    let back: crate::models::AppConfig = toml::from_str(&text).unwrap();
    assert_eq!(
        back.agents.connection_fallbacks,
        config.agents.connection_fallbacks
    );
    // An older file has none.
    let none = toml::to_string(&crate::core::config::default_config()).unwrap();
    assert!(!none.contains("connection_fallbacks"));
    let old: crate::models::AppConfig = toml::from_str(&none).unwrap();
    assert!(old.agents.connection_fallbacks.is_empty());
}
