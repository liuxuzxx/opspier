use super::*;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::Response,
    routing::any,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
#[derive(Default)]
struct Fixture {
    logins: AtomicUsize,
    reads: AtomicUsize,
    auth: bool,
}
fn response(status: u16, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}
async fn route(State(state): State<Arc<Fixture>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if path.ends_with("/server/state") {
        return response(200, json!({"version":"2.3.2"}));
    }
    if path.ends_with("/auth/users/login") {
        let body = axum::body::to_bytes(request.into_body(), 8192)
            .await
            .unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        if pairs.get("username").map(String::as_str) != Some("fixture-user")
            || pairs.get("password").map(String::as_str) != Some("fixture-pass")
        {
            return response(403, json!({"message":"must never be reflected"}));
        }
        state.logins.fetch_add(1, Ordering::SeqCst);
        return response(200, json!({"accessToken":"fixture-token","tokenTtl":3600}));
    }
    if state.auth
        && request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some("Bearer fixture-token")
    {
        return response(403, json!({"message":"secret-reflection-not-allowed"}));
    }
    if path.ends_with("/console/namespaces") {
        state.reads.fetch_add(1, Ordering::SeqCst);
        return response(
            200,
            json!({"code":200,"data":[{"namespace":"","namespaceShowName":"public"},{"namespace":"id-a","namespaceShowName":"Team"}]}),
        );
    }
    response(404, json!({"message":"not found"}))
}
async fn fixture(auth: bool) -> (String, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Fixture {
        auth,
        ..Default::default()
    });
    let app = Router::new().fallback(any(route)).with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}/custom"), state, handle)
}
fn settings(endpoint: String, auth: bool) -> NacosSettings {
    NacosSettings {
        endpoint,
        authentication: if auth { "password" } else { "none" }.into(),
        namespace: String::new(),
    }
}
#[tokio::test]
async fn authenticated_connection_uses_header_only_token_and_shares_one_login_for_parallel_reads() {
    let (endpoint, state, server) = fixture(true).await;
    let service = NacosService::default();
    let session = service
        .connect(
            settings(endpoint, true),
            Some(NacosMaterial {
                username: "fixture-user".into(),
                password: "fixture-pass".into(),
            }),
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        service.namespaces(&session.id),
        service.namespaces(&session.id)
    );
    assert_eq!(a.unwrap()[1].id, "id-a");
    assert_eq!(b.unwrap()[0].id, "");
    assert_eq!(state.logins.load(Ordering::SeqCst), 1);
    service.close(&session.id).unwrap();
    assert!(service.namespaces(&session.id).await.is_err());
    server.abort();
}
#[tokio::test]
async fn anonymous_connection_needs_no_material_and_invalid_auth_errors_do_not_reflect_server_body()
{
    let (endpoint, state, server) = fixture(false).await;
    let service = NacosService::default();
    let session = service
        .connect(settings(endpoint.clone(), false), None)
        .await
        .unwrap();
    assert_eq!(state.logins.load(Ordering::SeqCst), 0);
    assert_eq!(service.namespaces(&session.id).await.unwrap().len(), 2);
    let err = service
        .connect(
            settings(endpoint, true),
            Some(NacosMaterial {
                username: "wrong".into(),
                password: "bad".into(),
            }),
        )
        .await
        .err()
        .unwrap();
    assert!(!err.contains("must never"));
    assert!(err.contains("认证"));
    server.abort();
}
#[test]
fn nacos_settings_reject_credential_urls_queries_and_non_http_schemes() {
    for endpoint in [
        "http://user:pass@registry.test/nacos",
        "https://registry.test/nacos?accessToken=value",
        "ftp://registry.test",
        "https://registry.test/#key",
    ] {
        assert!(settings(endpoint.into(), true).validate().is_err());
    }
}
