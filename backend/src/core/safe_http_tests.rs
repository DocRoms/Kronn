use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn test_client(redirects: Redirects) -> SafeClient {
    client(
        SafeHttpPolicy::PublicOrLoopbackForTests,
        ClientOptions::new(redirects),
    )
    .unwrap()
}

#[test]
fn embedded_ipv4_forms_are_classified_as_their_ipv4_host() {
    for blocked in [
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::ffff:10.0.0.1",
        "::127.0.0.1",
        "64:ff9b::a9fe:a9fe",
        "2002:7f00:1::",
        "2001:0:4136:e378:8000:63bf:3fff:fdd2",
        "100.64.0.1",
        "198.18.0.1",
        "192.0.0.8",
        "0.1.2.3",
        "fec0::1",
        "2001:db8::1",
        "::1",
        "::",
        "64:ff9b:1::a00:1",
        "2001:2::1",
        "3fff::1",
        "5f00::1",
        "100::1",
        "100:0:0:1::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "4000::1",
        "192.88.99.1",
    ] {
        assert!(!is_global(ip(blocked)), "{blocked} must not be global");
    }
    for public in [
        "93.184.216.34",
        "2606:4700::1111",
        "::ffff:93.184.216.34",
        "2002:5db8:d822::",
    ] {
        assert!(is_global(ip(public)), "{public} is global");
    }
}

#[test]
fn mapped_ipv6_literals_are_refused_even_in_test_mode() {
    for target in [
        "http://[::ffff:127.0.0.1]:3140/",
        "http://[::ffff:169.254.169.254]/latest/meta-data/",
    ] {
        assert!(
            check_url(&url(target), SafeHttpPolicy::Public).is_err(),
            "{target}"
        );
    }
    assert!(check_url(
        &url("http://[::ffff:169.254.169.254]/"),
        SafeHttpPolicy::PublicOrLoopbackForTests
    )
    .is_err());
    assert!(check_url(&url("file:///etc/passwd"), SafeHttpPolicy::Configured).is_err());
}

#[tokio::test]
async fn a_literal_metadata_address_is_refused_before_any_connection() {
    let err = test_client(Redirects::Manual)
        .get(url("http://[::ffff:169.254.169.254]/latest/meta-data/"))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_blocked(), "{err}");
}

/// The old guard resolved once to check and reqwest resolved again to
/// connect. Here the first answer is public and every later one is loopback:
/// the connection must use the answer it checked, so it is refused.
#[tokio::test]
async fn a_rebinding_answer_never_reaches_the_connection() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let lookup: LookupFn = Arc::new(move |_host: String| {
        let n = seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(if n == 0 {
                vec![ip("93.184.216.34")]
            } else {
                vec![ip("127.0.0.1")]
            })
        })
    });
    let client = client_with_lookup(
        SafeHttpPolicy::Public,
        ClientOptions::new(Redirects::Manual),
        lookup.clone(),
    )
    .unwrap();
    // A pre-flight check sees the public answer.
    assert_eq!(
        lookup("rebind.test".into()).await.unwrap(),
        vec![ip("93.184.216.34")]
    );
    let target = url(&format!("http://rebind.test:{}/", server.address().port()));
    let err = client.get(target).send().await.unwrap_err();
    assert!(err.is_blocked(), "{err}");
    assert!(calls.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn a_mixed_public_and_private_answer_is_refused() {
    let lookup: LookupFn =
        Arc::new(|_host: String| Box::pin(async { Ok(vec![ip("93.184.216.34"), ip("10.0.0.7")]) }));
    let client = client_with_lookup(
        SafeHttpPolicy::Public,
        ClientOptions::new(Redirects::Manual),
        lookup,
    )
    .unwrap();
    let err = client
        .get(url("http://mixed.test/"))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_blocked(), "{err}");
}

#[tokio::test]
async fn same_origin_client_follows_on_origin_and_refuses_off_origin() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/b"))
        .mount(&server)
        .await;
    Mock::given(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    Mock::given(path("/away"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/sink", other.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&other)
        .await;
    let client = test_client(Redirects::SameOrigin);
    let followed = client
        .get(url(&format!("{}/a", server.uri())))
        .send()
        .await
        .unwrap();
    assert_eq!(followed.text().await.unwrap(), "ok");
    let err = client
        .get(url(&format!("{}/away", server.uri())))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_blocked(), "{err}");
}

#[test]
fn a_hop_may_not_downgrade_leave_the_plugin_host_or_replay_a_body_elsewhere() {
    let policy = SafeHttpPolicy::Configured;
    let from = url("https://api.example.com/x");
    assert!(validate_hop(
        &from,
        &url("http://api.example.com/x"),
        StatusCode::FOUND,
        policy,
        None
    )
    .unwrap_err()
    .contains("downgrade"));
    let base = url("https://api.example.com");
    assert!(validate_hop(
        &from,
        &url("https://other.example.com/x"),
        StatusCode::FOUND,
        policy,
        Some(&base)
    )
    .is_err());
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        assert!(validate_hop(
            &from,
            &url("https://other.example.com/x"),
            status,
            policy,
            None
        )
        .unwrap_err()
        .contains("307/308"));
    }
    assert_eq!(
        validate_hop(
            &from,
            &url("https://api.example.com/y"),
            StatusCode::TEMPORARY_REDIRECT,
            policy,
            None
        ),
        Ok(false)
    );
    assert_eq!(
        validate_hop(
            &from,
            &url("https://cdn.example.com/y"),
            StatusCode::FOUND,
            policy,
            None
        ),
        Ok(true)
    );
}

#[tokio::test]
async fn a_cross_origin_307_is_refused_and_the_body_never_reaches_it() {
    let origin = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(path("/token"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", format!("{}/steal", other.uri())),
        )
        .mount(&origin)
        .await;
    Mock::given(path("/steal"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&other)
        .await;
    let attach = |r: SafeRequest| r.body("client_secret=s3cr3t");
    let err = send_following(
        &test_client(Redirects::Manual),
        Outbound {
            method: Method::POST,
            url: url(&format!("{}/token", origin.uri())),
            headers: HeaderMap::new(),
            secret_headers: &[],
            secret_query_keys: &[],
            attach_body: &attach,
            has_body: true,
            pinned_base: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.is_blocked(), "{err}");
}

#[tokio::test]
async fn a_cross_origin_hop_drops_secret_slots_whatever_their_name_and_secret_query_keys() {
    let origin = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(path("/start"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}/sink?apikey=k3y&lang=fr", other.uri()),
        ))
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&other)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::USER_AGENT, "secret-in-ua".parse().unwrap());
    headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
    headers.insert(
        reqwest::header::AUTHORIZATION,
        "Bearer t0k".parse().unwrap(),
    );
    let attach = |r: SafeRequest| r;
    let response = send_following(
        &test_client(Redirects::Manual),
        Outbound {
            method: Method::GET,
            url: url(&format!("{}/start", origin.uri())),
            headers,
            secret_headers: &[reqwest::header::USER_AGENT],
            secret_query_keys: &["apikey".to_string()],
            attach_body: &attach,
            has_body: false,
            pinned_base: None,
        },
    )
    .await
    .unwrap();
    assert!(response.status().is_success());
    let received = other.received_requests().await.unwrap();
    let sink = &received[0];
    let ua = sink
        .headers
        .get("user-agent")
        .map(|v| v.to_str().unwrap().to_string());
    assert_ne!(ua.as_deref(), Some("secret-in-ua"));
    assert!(sink.headers.get("authorization").is_none());
    assert_eq!(sink.headers.get("accept").unwrap(), "application/json");
    assert_eq!(sink.url.query(), Some("lang=fr"));
}

#[tokio::test]
async fn a_cross_origin_302_that_would_resend_a_put_body_is_refused() {
    let origin = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(path("/put"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/sink", other.uri())),
        )
        .mount(&origin)
        .await;
    Mock::given(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&other)
        .await;
    let attach = |r: SafeRequest| r.body("{\"secret\":\"x\"}");
    let err = send_following(
        &test_client(Redirects::Manual),
        Outbound {
            method: Method::PUT,
            url: url(&format!("{}/put", origin.uri())),
            headers: HeaderMap::new(),
            secret_headers: &[],
            secret_query_keys: &[],
            attach_body: &attach,
            has_body: true,
            pinned_base: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("PUT body"), "{err}");
}

#[tokio::test]
async fn a_peer_client_does_not_follow_a_redirect_off_the_peer() {
    let peer = MockServer::start().await;
    Mock::given(path("/api/disc/claim-by-token"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
        )
        .mount(&peer)
        .await;
    let err = peer_client(Duration::from_secs(5))
        .unwrap()
        .post(url(&format!("{}/api/disc/claim-by-token", peer.uri())))
        .json(&serde_json::json!({"from_invite_code": "code"}))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_blocked(), "{err}");
}
