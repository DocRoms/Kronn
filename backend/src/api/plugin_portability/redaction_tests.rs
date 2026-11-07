use super::*;
use crate::models::{
    ApiAuthKind, ApiConfigKey, ApiDefaultHeader, ApiEndpoint, ApiSpec, McpSource, McpTransport,
    OAuth2ExtraHeader, TokenExchangeBodyFormat, TokenInjection,
};

/// Every secret the fixtures hide, each in a different place.
pub(crate) const SECRETS: &[&str] = &[
    "sk-argsecret123",
    "tokeq456",
    "descsecret789",
    "pathsecretAB",
    "ghp_userinfo1",
    "pwsecret2",
    "bodysecret3",
    "4242",
    "endpointsecret4",
    "ab12",
    "c2VjcmV0+/==",
    "hdrsecret5",
    "docsecret6",
    "plchsecret7",
    "tokenurl8",
    "extrahdr9",
    "cmdsecret10",
    "keydesc11",
    "epdesc12",
    "Bearer_raw13",
    "pS3cretPass",
    "dGhpc2lzYXRva2Vu",
    "abc123secret",
    "sk-abc",
    "keysecret15",
    "SK_LIVE_AB12CD34EF",
    "sk_live_ab12cd34ef",
    "1234567",
    "sk1live9abc",
];

pub(crate) fn no_catalogue() -> ExportContext<'static> {
    ExportContext {
        catalogue: None,
        declared_env_keys: &[],
        salt: "test-salt",
    }
}

fn spec(auth: ApiAuthKind) -> ApiSpec {
    let spec = ApiSpec {
        base_url:
            "https://ghp_userinfo1:x-oauth-basic@api.leak.test/bot/pathsecretAB?q=ab12&b=c2VjcmV0+/=="
                .into(),
        auth,
        endpoints: vec![
            ApiEndpoint {
                path: "/v1/items/{id}".into(),
                method: "GET".into(),
                description: "Use key epdesc12".into(),
            },
            ApiEndpoint {
                path: "/hooks/T0AB/epdesc12".into(),
                method: "POST".into(),
                description: String::new(),
            },
        ],
        docs_url: Some("https://docs.leak.test/x?ticket=docsecret6".into()),
        config_keys: vec![
            ApiConfigKey {
                env_key: "API_TOKEN".into(),
                label: "API Token".into(),
                placeholder: "plchsecret7".into(),
                description: "keydesc11".into(),
                public: false,
            },
            // A field label that is a token, and the env key derived from it.
            ApiConfigKey {
                env_key: "SK_LIVE_AB12CD34EF".into(),
                label: "sk_live_ab12cd34ef".into(),
                placeholder: String::new(),
                description: String::new(),
                public: true,
            },
        ],
        default_headers: vec![
            ApiDefaultHeader {
                name: "X-Static".into(),
                value: "hdrsecret5".into(),
            },
            ApiDefaultHeader {
                name: "Authorization-Extra".into(),
                value: "Bearer ${ENV.API_TOKEN}".into(),
            },
            ApiDefaultHeader {
                name: "X-Raw".into(),
                value: "Bearer Bearer_raw13".into(),
            },
            ApiDefaultHeader {
                name: "X-Account-1234567".into(),
                value: "${ENV.SK_LIVE_AB12CD34EF}".into(),
            },
        ],
        test_endpoint: Some("/hooks/T0AB/epdesc12".into()),
    };
    // A field added to a struct fails to compile here until it is decided.
    let ApiSpec {
        base_url: _,
        auth: _,
        endpoints,
        docs_url,
        config_keys,
        default_headers,
        test_endpoint,
    } = &spec;
    assert!(docs_url.is_some() && test_endpoint.is_some());
    for ApiEndpoint {
        path: _,
        method: _,
        description: _,
    } in endpoints
    {}
    for ApiConfigKey {
        env_key: _,
        label: _,
        placeholder: _,
        description: _,
        public: _,
    } in config_keys
    {}
    for ApiDefaultHeader { name: _, value: _ } in default_headers {}
    spec
}

fn server(id: &str, transport: McpTransport, auth: Option<ApiAuthKind>) -> McpServer {
    let server = McpServer {
        id: id.into(),
        name: "Leaky".into(),
        description: "https://mcp.leak.test/sse?key=descsecret789".into(),
        transport,
        source: McpSource::Detected,
        api_spec: auth.map(spec),
    };
    let McpServer {
        id: _,
        name: _,
        description: _,
        transport: _,
        source: _,
        api_spec: _,
    } = &server;
    server
}

/// Kronn's own id format, `custom-<slug>-<8 hex>`: an id the export keeps.
/// The fixtures are named "Leaky", so Kronn would have made `custom-leaky-…`.
pub(crate) fn fixture_id(prefix: &str, index: usize) -> String {
    let group = match prefix {
        "t" => '1',
        "a" => 'a',
        "s" => '5',
        "e" => 'e',
        _ => '0',
    };
    format!("custom-leaky-{group}{index:07x}")
}

fn every_transport() -> Vec<McpTransport> {
    let all = vec![
        McpTransport::Stdio {
            command: "/opt/cmdsecret10/run".into(),
            args: vec![
                "--api-key".into(),
                "sk-argsecret123".into(),
                "--token=tokeq456".into(),
                "-y".into(),
                "-pS3cretPass".into(),
                "-dGhpc2lzYXRva2Vu".into(),
            ],
        },
        McpTransport::Sse {
            url: "https://user:pwsecret2@hooks.leak.test/hooks/catch/1/pathsecretAB/".into(),
        },
        McpTransport::Sse {
            url: "https://h.leak.test:sk-abc/".into(),
        },
        McpTransport::Sse {
            url: "https://[sk1live9abc]/x".into(),
        },
        McpTransport::Streamable {
            url: "https://api.leak.test/mcp?token=tokeq456".into(),
        },
        McpTransport::Streamable {
            url: "https://abc123secret.m.pipedream.net".into(),
        },
        McpTransport::ApiOnly,
    ];
    // A new transport, or a new field of one, fails to compile here.
    for transport in &all {
        match transport {
            McpTransport::Stdio {
                command: _,
                args: _,
            }
            | McpTransport::Sse { url: _ }
            | McpTransport::Streamable { url: _ }
            | McpTransport::ApiOnly => {}
        }
    }
    all
}

fn every_injection() -> Vec<TokenInjection> {
    let all = vec![
        TokenInjection::BearerHeader,
        TokenInjection::CustomHeader {
            name: "X-Token".into(),
        },
        TokenInjection::QueryParam {
            name: "token".into(),
        },
    ];
    for injection in &all {
        match injection {
            TokenInjection::BearerHeader
            | TokenInjection::CustomHeader { name: _ }
            | TokenInjection::QueryParam { name: _ } => {}
        }
    }
    all
}

fn every_auth() -> Vec<ApiAuthKind> {
    let mut all = vec![
        ApiAuthKind::ApiKeyQuery {
            param_name: "apikey".into(),
            env_key: "API_TOKEN".into(),
        },
        ApiAuthKind::ApiKeyHeader {
            header_name: "X-Api-Key".into(),
            env_key: "API_TOKEN".into(),
        },
        ApiAuthKind::Bearer {
            env_key: "API_TOKEN".into(),
        },
        ApiAuthKind::Basic {
            user_env: "USER".into(),
            password_env: "PASS".into(),
        },
        ApiAuthKind::BasicApiKey {
            env_key: "API_TOKEN".into(),
        },
        ApiAuthKind::OAuth2ClientCredentials {
            token_url: "https://auth.leak.test/oauth/token?client_secret=tokenurl8".into(),
            client_id_env: "CLIENT_ID".into(),
            client_secret_env: "CLIENT_SECRET".into(),
            scope: "read write".into(),
            extra_headers: vec![
                OAuth2ExtraHeader {
                    name: "x-api-key".into(),
                    value_template: "{CLIENT_ID}".into(),
                },
                OAuth2ExtraHeader {
                    name: "x-literal".into(),
                    value_template: "extrahdr9".into(),
                },
            ],
        },
        ApiAuthKind::None,
    ];
    for inject in every_injection() {
        all.push(ApiAuthKind::TokenExchange {
            endpoint: "/sessions?api_key=endpointsecret4".into(),
            method: "POST".into(),
            body_template: serde_json::json!({
                "type": "key",
                "key": "${ENV.API_TOKEN}",
                "secret": "bodysecret3",
                "pin": 4242,
                "ratio": 0.5,
                "nested": [{"value": "bodysecret3"}],
                "sk_live_keysecret15": "x",
            }),
            body_format: TokenExchangeBodyFormat::FormUrlEncoded,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 600,
            inject: inject.clone(),
            creds_env_keys: vec!["API_TOKEN".into()],
        });
        all.push(ApiAuthKind::CliToken {
            command: "cmdsecret10".into(),
            args: vec!["token".into(), "sk-argsecret123".into()],
            inject,
            fallback_env_key: Some("API_TOKEN".into()),
        });
    }
    for auth in &all {
        match auth {
            ApiAuthKind::ApiKeyQuery {
                param_name: _,
                env_key: _,
            }
            | ApiAuthKind::ApiKeyHeader {
                header_name: _,
                env_key: _,
            }
            | ApiAuthKind::Bearer { env_key: _ }
            | ApiAuthKind::Basic {
                user_env: _,
                password_env: _,
            }
            | ApiAuthKind::BasicApiKey { env_key: _ }
            | ApiAuthKind::None => {}
            ApiAuthKind::CliToken {
                command: _,
                args: _,
                inject: _,
                fallback_env_key,
            } => assert!(fallback_env_key.is_some()),
            ApiAuthKind::OAuth2ClientCredentials {
                token_url: _,
                client_id_env: _,
                client_secret_env: _,
                scope: _,
                extra_headers,
            } => {
                for OAuth2ExtraHeader {
                    name: _,
                    value_template: _,
                } in extra_headers
                {}
            }
            ApiAuthKind::TokenExchange {
                endpoint: _,
                method: _,
                body_template: _,
                body_format: _,
                token_jsonpath: _,
                ttl_seconds: _,
                inject: _,
                creds_env_keys,
            } => assert!(!creds_env_keys.is_empty()),
        }
    }
    all
}

fn every_source() -> Vec<McpSource> {
    let all = vec![
        McpSource::Registry,
        McpSource::Detected,
        McpSource::Manual,
        McpSource::HostImported,
    ];
    for source in &all {
        match source {
            McpSource::Registry
            | McpSource::Detected
            | McpSource::Manual
            | McpSource::HostImported => {}
        }
    }
    all
}

/// Every definition the fixtures can build: each transport, each auth, each source.
pub(crate) fn every_server() -> Vec<McpServer> {
    let mut all = Vec::new();
    for (index, transport) in every_transport().into_iter().enumerate() {
        all.push(server(&fixture_id("t", index), transport, None));
    }
    for (index, auth) in every_auth().into_iter().enumerate() {
        all.push(server(
            &fixture_id("a", index),
            McpTransport::ApiOnly,
            Some(auth),
        ));
    }
    for (index, source) in every_source().into_iter().enumerate() {
        let mut one = server(&fixture_id("s", index), McpTransport::ApiOnly, None);
        one.source = source;
        all.push(one);
    }
    all
}

fn leaf_patterns(value: &Value, pattern: &str, out: &mut BTreeSet<String>) {
    if pattern.starts_with(BODY) {
        out.insert(BODY.to_string());
        return;
    }
    match value {
        Value::Array(items) => {
            assert!(
                !items.is_empty(),
                "{pattern}: an empty list in the fixtures"
            );
            for item in items {
                leaf_patterns(item, &format!("{pattern}[]"), out);
            }
        }
        Value::Object(map) => {
            assert!(
                !map.is_empty(),
                "{pattern}: an empty object in the fixtures"
            );
            for (key, item) in map {
                leaf_patterns(item, &join(pattern, key), out);
            }
        }
        Value::Null => panic!("{pattern}: a None in the fixtures"),
        _ => {
            out.insert(pattern.to_string());
        }
    }
}

/// A field added to the definition without a decision fails here: the
/// fixtures destructure every struct and variant without `..`; every Option is
/// set and every list filled; the leaves they reach are exactly the decided
/// paths, both ways.
#[test]
fn every_field_of_the_definition_has_a_decision() {
    let mut seen = BTreeSet::new();
    for server in every_server() {
        let redacted = redact_plugin(
            &server,
            &no_catalogue(),
            "Exported plugin",
            &["API_TOKEN".into()],
        );
        assert!(
            redacted.unknown.is_empty(),
            "{}: undecided {:?}",
            server.id,
            redacted.unknown
        );
        leaf_patterns(&serde_json::to_value(&server).unwrap(), "", &mut seen);
    }
    let decided: BTreeSet<String> = RULES
        .iter()
        .map(|(path, _)| path.to_string())
        .filter(|path| !["label", "env_keys[]"].contains(&path.as_str()))
        .collect();
    let missing: Vec<_> = decided.difference(&seen).collect();
    let extra: Vec<_> = seen.difference(&decided).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "decided but not in the fixtures: {missing:?}; in the fixtures but undecided: {extra:?}"
    );
    assert_eq!(rule("some.new.field"), Rule::Unknown);
}

/// Unknown means withheld in production too, a number or a flag included,
/// with a placeholder that carries the type.
#[test]
fn an_undecided_leaf_is_withheld_whatever_its_type() {
    let mut value = serde_json::to_value(server("custom-x", McpTransport::ApiOnly, None)).unwrap();
    value["new_flag"] = Value::Bool(true);
    value["new_count"] = Value::from(3);
    value["new_text"] = Value::from("tok");
    let mut walk = Walk::new(None);
    let out = walk.value(&value, "", "", None);
    walk.unknown.sort();
    assert_eq!(walk.unknown, vec!["new_count", "new_flag", "new_text"]);
    for key in ["new_flag", "new_count", "new_text"] {
        assert!(is_placeholder(out[key].as_str().unwrap()), "{key}");
    }
    let types: BTreeMap<_, _> = walk
        .placeholders
        .iter()
        .map(|p| (p.field.clone(), p.value_type))
        .collect();
    assert_eq!(types["new_flag"], ExportValueType::Boolean);
    assert_eq!(types["new_count"], ExportValueType::Number);
    assert_eq!(types["new_text"], ExportValueType::Text);
}

#[test]
fn every_source_tag_is_kept() {
    for source in every_source() {
        let mut one = server("custom-src", McpTransport::ApiOnly, None);
        one.source = source.clone();
        let (redacted, _) = redact_server(&one, None);
        assert_eq!(
            serde_json::to_value(&redacted.source).unwrap(),
            serde_json::to_value(&source).unwrap()
        );
    }
}

#[test]
fn no_secret_of_any_field_survives_the_redaction() {
    for server in every_server() {
        let (redacted, placeholders) = redact_server(&server, None);
        let text = serde_json::to_string(&redacted).unwrap();
        let listed = serde_json::to_string(&placeholders).unwrap();
        for secret in SECRETS {
            assert!(!text.contains(secret), "{}: {secret} in {text}", server.id);
            assert!(
                !listed.contains(secret),
                "{}: {secret} in {listed}",
                server.id
            );
        }
    }
}

#[test]
fn names_references_and_origins_survive() {
    let original = server(
        "custom-plain",
        McpTransport::ApiOnly,
        Some(ApiAuthKind::Bearer {
            env_key: "API_TOKEN".into(),
        }),
    );
    let (redacted, placeholders) = redact_server(&original, None);
    assert_eq!(redacted.id, "custom-plain");
    let spec = redacted.api_spec.unwrap();
    assert_eq!(redacted.name, "Leaky");
    assert_eq!(spec.default_headers[0].name, "X-Static");
    assert_eq!(spec.default_headers[1].value, "Bearer ${ENV.API_TOKEN}");
    assert_eq!(spec.endpoints[0].path, "/v1/items/{id}");
    assert_eq!(spec.endpoints[0].method, "GET");
    assert_eq!(spec.config_keys[0].env_key, "API_TOKEN");
    assert_eq!(spec.config_keys[0].label, "API Token");
    assert_eq!(
        spec.base_url, "${ENV.KRONN_EXPORT_API_SPEC_BASE_URL}",
        "{placeholders:?}"
    );
    let base = placeholders
        .iter()
        .find(|p| p.field == "api_spec.base_url")
        .unwrap();
    assert!(base.required);
    assert_eq!(base.hint.as_deref(), Some("https://api.leak.test"));
    let description = placeholders
        .iter()
        .find(|p| p.field == "description")
        .unwrap();
    assert!(!description.required);
    // A withheld test endpoint is a placeholder, the same as its endpoint's.
    assert_eq!(
        spec.test_endpoint.as_deref(),
        Some(spec.endpoints[1].path.as_str())
    );
    assert!(is_placeholder(&spec.endpoints[1].path));
}

#[test]
fn an_id_is_kept_in_kronns_format_and_replaced_otherwise_never_a_placeholder() {
    let named = |id: &str, name: &str| {
        let mut one = server(id, McpTransport::ApiOnly, None);
        one.name = name.into();
        one
    };
    // Kronn's format whose slug is the slug of a kept name, or a plain slug.
    for (kept, name) in [
        ("custom-leaky-1a2b3c4d", "Leaky"),
        ("custom-acme-api-0a1b2c3d", "Acme API"),
        (
            "custom-salesforce-sales-api-89abcdef",
            "Salesforce Sales API",
        ),
        ("custom-oauth2-api-0a1b2c3d", "OAuth2 API"),
        ("custom-s3-4e5f6a7b", "S3"),
        ("custom-gpt4-89abcdef", "GPT4"),
        ("custom-k8s-0badf00d", "K8s"),
        ("github", "Leaky"),
        ("mcp-github", "Leaky"),
        ("custom-plain", "Leaky"),
    ] {
        let (redacted, _) = redact_server(&named(kept, name), None);
        assert_eq!(redacted.id, kept);
    }
    // The slug is the lowercased name: when the name is withheld, or is not
    // the one the slug came from, the id would carry it, so it is replaced.
    for (withheld, name) in [
        ("custom-acme-sk1live9abc-1a2b3c4d", "Acme sk1live9abc"),
        ("custom-acme-sk1live-1a2b3c4d", "Leaky"),
        ("sk_live_ab12cd34ef", "Leaky"),
        ("custom-acme-XYZ", "Leaky"),
        ("custom-Acme-1a2b3c4d", "Leaky"),
    ] {
        let (redacted, placeholders) = redact_server(&named(withheld, name), None);
        assert!(redacted.id.starts_with("custom-export-"), "{withheld}");
        assert!(kronn_custom_slug(&redacted.id).is_some(), "{}", redacted.id);
        assert!(!is_placeholder(&redacted.id));
        let text = serde_json::to_string(&(&redacted, &placeholders)).unwrap();
        assert!(!text.contains(withheld), "{withheld}");
        assert!(!text.contains("sk1live"), "{withheld}");
    }
}

#[test]
fn a_prefixed_secret_inside_a_value_with_other_references_is_withheld() {
    let mut declared = BTreeSet::new();
    declared.insert("API_TOKEN".to_string());
    for value in [
        "Bearer ${ENV.KRONN_EXPORT_SK1LIVE9ABC}",
        "${ENV.API_TOKEN}:${ENV.KRONN_EXPORT_SK1LIVE9ABC}",
        "${ENV.KRONN_EXPORT_1234567}",
    ] {
        assert!(!references_only(value, "${", &declared), "{value}");
    }
    assert!(references_only(
        "${ENV.API_TOKEN}:${ENV.KRONN_EXPORT_API_SPEC_BASE_URL}",
        "${",
        &declared
    ));
    // Through the walk, in a header and in a request body.
    let mut one = server(
        "custom-leaky-0000abcd",
        McpTransport::ApiOnly,
        Some(ApiAuthKind::TokenExchange {
            endpoint: "/sessions".into(),
            method: "POST".into(),
            body_template: serde_json::json!({
                "key": "${ENV.API_TOKEN} ${ENV.KRONN_EXPORT_SK1LIVE9ABC}",
            }),
            body_format: TokenExchangeBodyFormat::Json,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 60,
            inject: TokenInjection::BearerHeader,
            creds_env_keys: vec!["API_TOKEN".into()],
        }),
    );
    one.api_spec.as_mut().unwrap().default_headers[1].value =
        "Bearer ${ENV.API_TOKEN} ${ENV.KRONN_EXPORT_SK1LIVE9ABC}".into();
    let (redacted, placeholders) = redact_server(&one, None);
    let text = serde_json::to_string(&(&redacted, &placeholders)).unwrap();
    assert!(!text.contains("SK1LIVE9ABC"), "{text}");
}

#[test]
fn an_env_key_derived_from_a_withheld_label_is_withheld_too() {
    let original = server(
        &fixture_id("e", 0),
        McpTransport::ApiOnly,
        Some(ApiAuthKind::Bearer {
            env_key: "API_TOKEN".into(),
        }),
    );
    let redacted = redact_plugin(
        &original,
        &no_catalogue(),
        "Exported plugin",
        &["API_TOKEN".into(), "SK_LIVE_AB12CD34EF".into()],
    );
    let spec = redacted.server.api_spec.unwrap();
    assert!(is_placeholder(&spec.config_keys[1].env_key));
    assert!(is_placeholder(&spec.config_keys[1].label));
    // The bundle's env-key list shares the definition's placeholder.
    assert_eq!(redacted.env_keys[0], "API_TOKEN");
    assert_eq!(redacted.env_keys[1], spec.config_keys[1].env_key);
    for env_key in ["SK_LIVE_AB12CD34EF", "ACCOUNT_1234567", "OAUTH2_AB12CD34EF"] {
        assert!(!env_key_allowed(env_key), "{env_key}");
    }
    for env_key in ["API_TOKEN", "GITHUB_PERSONAL_ACCESS_TOKEN", "V2_URL"] {
        assert!(env_key_allowed(env_key), "{env_key}");
    }
    // A header referencing that key would name it: withheld as well.
    assert!(is_placeholder(&spec.default_headers[3].value));
    assert!(is_placeholder(&spec.default_headers[3].name));
}

#[test]
fn only_a_single_letter_or_a_lowercase_long_flag_is_an_argument_to_keep() {
    for kept in ["-y", "--api-key", "--port", "${ENV.TOKEN}"] {
        assert!(argument_allowed(kept), "{kept}");
    }
    for withheld in [
        "-pS3cretPass",
        "-dGhpc2lzYXRva2Vu",
        "--Token",
        "--token=abc",
        "-ab",
        "--",
        "sk-abc",
    ] {
        assert!(!argument_allowed(withheld), "{withheld}");
    }
}

#[test]
fn an_origin_is_kept_only_with_a_clean_host_and_a_numeric_port() {
    for kept in [
        "https://api.example.test",
        "https://api.example.test/",
        "http://127.0.0.1:8080",
        "http://[::1]:3000",
        "{JIRA_URL}",
    ] {
        assert!(url_allowed(kept, None), "{kept}");
    }
    for withheld in [
        "https://api.example.test/v1",
        "https://u@api.example.test",
        "https://api.example.test?x=1",
        "ftp://api.example.test",
        "https://abc123secret.m.pipedream.net",
        "https://h.example.test:sk-abc",
        "https://averyveryverylonglabelname.example.test",
        "https://hook-x9k2.example.test",
        "https://[sk1live9abc]",
        "https://[sk1live9abc]:443",
        "https://acct-1234567.example.test",
    ] {
        assert!(!url_allowed(withheld, None), "{withheld}");
    }
    // The catalogue's own host is kept, whatever its shape.
    assert!(url_allowed(
        "https://abc123.vendor.test",
        Some("https://abc123.vendor.test/api")
    ));
    // A withheld host never reaches the hint.
    let (redacted, placeholders) = redact_server(
        &server(
            "custom-host",
            McpTransport::Streamable {
                url: "https://abc123secret.m.pipedream.net".into(),
            },
            None,
        ),
        None,
    );
    let McpTransport::Streamable { url } = redacted.transport else {
        panic!("streamable");
    };
    assert!(is_placeholder(&url));
    let listed = placeholders
        .iter()
        .find(|p| p.field == "transport.Streamable.url")
        .unwrap();
    assert_eq!(listed.hint, None);
    assert_eq!(
        url_for_display("https://abc123secret.m.pipedream.net/x"),
        "…"
    );
}

#[test]
fn a_name_with_a_token_shaped_word_is_withheld() {
    for kept in [
        "Leaky",
        "API Token",
        "X-Api-Key",
        "Acme API v2",
        "$.access_token",
        "read write",
        "Port 443",
    ] {
        assert!(name_allowed(kept), "{kept}");
    }
    for withheld in [
        "prod sk1live9abc",
        "Token abcdefghijklmnopqrstuv",
        "ab12cd34ef",
        "Account 1234567",
        "123456",
    ] {
        assert!(!name_allowed(withheld), "{withheld}");
    }
    let one = server("custom-label", McpTransport::ApiOnly, None);
    let redacted = redact_plugin(&one, &no_catalogue(), "prod sk1live9abc", &[]);
    assert_eq!(redacted.label, "${ENV.KRONN_EXPORT_LABEL}");
    assert!(redacted.placeholders.iter().any(|p| p.field == "label"));
    assert_eq!(
        redact_plugin(&one, &no_catalogue(), "Production", &[]).label,
        "Production"
    );
}

#[test]
fn a_catalogue_value_is_kept_and_a_changed_one_withheld() {
    let catalogue = server(
        "mcp-vendor",
        McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@vendor/server".into()],
        },
        None,
    );
    let mut changed = catalogue.clone();
    changed.transport = McpTransport::Stdio {
        command: "npx".into(),
        args: vec![
            "-y".into(),
            "@vendor/server".into(),
            "sk-argsecret123".into(),
        ],
    };
    let (redacted, _) = redact_server(&changed, Some(&catalogue));
    let McpTransport::Stdio { command, args } = redacted.transport else {
        panic!("stdio");
    };
    assert_eq!(command, "npx");
    assert_eq!(args[..2], ["-y".to_string(), "@vendor/server".to_string()]);
    assert!(args[2].starts_with("${ENV.KRONN_EXPORT_"));
}

#[test]
fn the_same_literal_gets_the_same_placeholder_and_a_reexport_lists_it_again() {
    let original = every_server()
        .into_iter()
        .find(|s| s.id == fixture_id("a", 0))
        .unwrap();
    let (once, first) = redact_server(&original, None);
    let (twice, second) = redact_server(&once, None);
    assert_eq!(
        serde_json::to_value(&once).unwrap(),
        serde_json::to_value(&twice).unwrap()
    );
    let keys =
        |list: &[ExportPlaceholder]| list.iter().map(|p| p.key.clone()).collect::<BTreeSet<_>>();
    assert_eq!(keys(&first), keys(&second));
    assert_eq!(keys(&first), placeholders_in(&once));
}

/// What the importer types back: the exporter's value of each placeholder.
pub(crate) fn collect_fills(before: &Value, after: &Value, values: &mut BTreeMap<String, String>) {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                match b.get(key) {
                    Some(other) => collect_fills(value, other, values),
                    // A withheld body key: find it by its value's position.
                    None => {
                        if let Some((placeholder, other)) = b.iter().find(|(k, v)| {
                            is_placeholder(k) && !a.contains_key(*k) && collect_matches(value, v)
                        }) {
                            values.insert(
                                placeholder[6..placeholder.len() - 1].to_string(),
                                key.clone(),
                            );
                            collect_fills(value, other, values);
                        }
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (x, y) in a.iter().zip(b) {
                collect_fills(x, y, values);
            }
        }
        (original, Value::String(text))
            if is_placeholder(text) && Value::String(text.clone()) != *original =>
        {
            let shown = match original {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            values.insert(text[6..text.len() - 1].to_string(), shown);
        }
        _ => {}
    }
}

fn collect_matches(original: &Value, redacted: &Value) -> bool {
    original == redacted || matches!(redacted, Value::String(text) if is_placeholder(text))
}

#[test]
fn filling_every_placeholder_restores_the_exact_definition() {
    for original in every_server() {
        let (redacted, placeholders) = redact_server(&original, None);
        let mut values = BTreeMap::new();
        collect_fills(
            &serde_json::to_value(&original).unwrap(),
            &serde_json::to_value(&redacted).unwrap(),
            &mut values,
        );
        assert_eq!(
            values.keys().cloned().collect::<BTreeSet<_>>(),
            placeholders.iter().map(|p| p.key.clone()).collect(),
            "{}",
            original.id
        );
        let types = placeholders
            .iter()
            .map(|p| (p.key.clone(), p.value_type))
            .collect();
        let (filled, left) = fill(&redacted, &values, &types).unwrap();
        assert!(left.is_empty(), "{left:?}");
        // Exact, JSON types included: a body number comes back a number.
        assert_eq!(
            serde_json::to_value(&filled).unwrap(),
            serde_json::to_value(&original).unwrap(),
            "{}",
            original.id
        );
    }
}

#[test]
fn an_unfilled_placeholder_stays_and_is_reported() {
    let original = server(
        "custom-unfilled",
        McpTransport::ApiOnly,
        Some(ApiAuthKind::None),
    );
    let (redacted, _) = redact_server(&original, None);
    let (filled, left) = fill(&redacted, &BTreeMap::new(), &BTreeMap::new()).unwrap();
    assert_eq!(
        serde_json::to_value(&filled).unwrap(),
        serde_json::to_value(&redacted).unwrap()
    );
    assert!(left.contains(&"KRONN_EXPORT_API_SPEC_BASE_URL".to_string()));
}

#[test]
fn a_regenerated_id_is_the_same_within_one_export_and_differs_across() {
    let odd = server("sk_live_ab12cd34ef", McpTransport::ApiOnly, None);
    let first = ExportContext {
        catalogue: None,
        declared_env_keys: &[],
        salt: "one",
    };
    let second = ExportContext {
        catalogue: None,
        declared_env_keys: &[],
        salt: "two",
    };
    let a = redact_plugin(&odd, &first, "A", &[]).server.id;
    let b = redact_plugin(&odd, &first, "B", &[]).server.id;
    let c = redact_plugin(&odd, &second, "A", &[]).server.id;
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(kronn_custom_slug(&a).is_some());
}

#[test]
fn a_placeholder_key_passes_the_name_check_and_a_prefixed_secret_does_not() {
    for server in every_server() {
        for placeholder in redact_server(&server, None).1 {
            assert!(env_key_allowed(&placeholder.key), "{}", placeholder.key);
        }
    }
    assert!(!is_placeholder("${ENV.KRONN_EXPORT_SK1LIVE9ABC}"));
    assert!(!env_key_allowed("KRONN_EXPORT_SK1LIVE9ABC"));
    assert!(!name_allowed("KRONN_EXPORT_1234567"));
}

#[test]
fn a_short_letter_digit_word_is_a_name_and_a_long_one_a_token() {
    for word in ["oauth2", "s3", "gpt4", "k8s", "v2", "OAuth2", "E2B"] {
        assert!(!token_like(word), "{word}");
    }
    for word in [
        "sk1live9abc",
        "ab12cd34ef",
        "1234567",
        "abcdefghijklmnopqrstu",
    ] {
        assert!(token_like(word), "{word}");
    }
    for name in ["OAuth2 API", "S3", "GPT4", "K8s", "OAUTH2_TOKEN"] {
        assert!(name_allowed(name), "{name}");
    }
    // A host label keeps the stricter rule: any mix is withheld.
    assert!(!url_allowed("https://k8s.example.test", None));
    assert!(url_allowed("https://api.example.test", None));
}

#[test]
fn a_token_cut_into_segments_is_withheld_whole() {
    for name in [
        "License 4B7D-9F1C-0A2B-3C4D",
        "KEY_4B7D_9F1C_0A2B",
        "custom-4b7d-9f1c-0a2b",
    ] {
        assert!(!name_allowed(name), "{name}");
    }
    assert!(!env_key_allowed("KEY_4B7D_9F1C_0A2B"));
    for kept in [
        "OAUTH2_CLIENT_ID",
        "GPT4_API_KEY",
        "oauth2-api",
        "s3",
        "k8s",
        "v2",
        "API_V2_URL",
    ] {
        assert!(name_allowed(kept), "{kept}");
    }
    // Inside a reference, and as a plain-slug id.
    let declared = BTreeSet::new();
    assert!(!references_only(
        "Bearer ${ENV.KEY_4B7D_9F1C_0A2B}",
        "${",
        &declared
    ));
    let mut one = server("plugin-4b7d-9f1c-0a2b", McpTransport::ApiOnly, None);
    one.name = "License 4B7D-9F1C-0A2B-3C4D".into();
    let (redacted, placeholders) = redact_server(&one, None);
    assert!(redacted.id.starts_with("custom-export-"), "{}", redacted.id);
    let text = serde_json::to_string(&(&redacted, &placeholders)).unwrap();
    for part in ["4b7d", "4B7D", "9f1c", "9F1C"] {
        assert!(!text.contains(part), "{part} in {text}");
    }
}

#[test]
fn a_token_cut_by_any_separator_is_withheld_whole() {
    for name in [
        "License 4B7D 9F1C 0A2B 3C4D",
        "4B7D.9F1C.0A2B",
        "4111 1111 1111 1111",
    ] {
        assert!(!name_allowed(name), "{name}");
    }
    for kept in ["OAuth2 API", "GPT4 Turbo", "S3", "Notion-Version"] {
        assert!(name_allowed(kept), "{kept}");
    }
    // Erring on the safe side: real keys with two numbered segments are
    // withheld too, and come back filled by the importer.
    for withheld in ["S3_BUCKET_V2", "K8S_NODE_1"] {
        assert!(!env_key_allowed(withheld), "{withheld}");
    }
    // Non-ASCII digits are digits, not separators.
    for name in [
        "License \u{FF14}B7D \u{FF19}F1C \u{FF10}A2B",
        "ab\u{FF11}\u{FF12}cd\u{FF13}\u{FF14}ef9",
    ] {
        assert!(!name_allowed(name), "{name}");
    }
    assert!(name_allowed("Données Été"));
    // One numbered segment passes, whatever the length: a residual risk.
    assert!(name_allowed("abcdefgh_ijklmnop_q1"));
}
