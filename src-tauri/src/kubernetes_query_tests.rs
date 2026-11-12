use super::query::QueryInput;
use super::*;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::sync::Mutex as StdMutex;
#[derive(Clone, Default)]
struct ApiState {
    trace: Arc<StdMutex<Vec<String>>>,
    accepts: Arc<StdMutex<Vec<String>>>,
    deny_catalog: Arc<std::sync::atomic::AtomicBool>,
    large_catalog: Arc<std::sync::atomic::AtomicBool>,
    header_error: Arc<std::sync::atomic::AtomicU16>,
    catalog_header_error: Arc<std::sync::atomic::AtomicBool>,
    catalog_tail_error: Arc<std::sync::atomic::AtomicU16>,
}
fn http_failure(code: u16, hang: bool) -> Response {
    let status = StatusCode::from_u16(code).unwrap();
    if hang {
        return (
            status,
            axum::body::Body::from_stream(futures::stream::pending::<
                Result<Vec<u8>, std::io::Error>,
            >()),
        )
            .into_response();
    }
    (status, vec![b'x'; 8 * 1024 * 1024]).into_response()
}
async fn api(State(s): State<ApiState>, method: Method, headers: HeaderMap, uri: Uri) -> Response {
    assert_eq!(method, Method::GET);
    s.accepts.lock().unwrap().push(
        headers
            .get("accept")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    s.trace.lock().unwrap().push(uri.to_string());
    let path = uri.path();
    let query = uri.query().unwrap_or("");
    if path.contains("/namespaces/denied/") {
        return (StatusCode::FORBIDDEN, "fixture-sensitive-server-body").into_response();
    }
    if path.contains("/namespaces/headers-error/") {
        return http_failure(
            s.header_error.load(std::sync::atomic::Ordering::SeqCst),
            true,
        );
    }
    if path.contains("/namespaces/error-tail/") {
        let page = query
            .split("continue=page")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        if page >= 5 {
            return http_failure(
                s.header_error.load(std::sync::atomic::Ordering::SeqCst),
                false,
            );
        }
        return axum::Json(json!({"metadata":{"continue":format!("page{}",page+1)},"items":[{"metadata":{"name":"plain","namespace":"error-tail"}}],"padding":"x".repeat(7*1024*1024)})).into_response();
    }
    if path == "/apis/apiextensions.k8s.io/v1/customresourcedefinitions" {
        if s.catalog_header_error
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return http_failure(
                s.header_error.load(std::sync::atomic::Ordering::SeqCst),
                true,
            );
        }
        let tail = s
            .catalog_tail_error
            .load(std::sync::atomic::Ordering::SeqCst);
        if tail > 0 {
            let page = query
                .split("continue=page")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            if page >= 3 {
                return http_failure(tail, false);
            }
            return axum::Json(json!({"metadata":{"continue":format!("page{}",page+1)},"items":[{"spec":{"group":"partial.example","names":{"plural":"partials","kind":"Partial"},"scope":"Namespaced","versions":[{"name":"v1","served":true}]}}],"padding":"x".repeat(7*1024*1024)})).into_response();
        }
        if s.deny_catalog.load(std::sync::atomic::Ordering::SeqCst) {
            return (StatusCode::FORBIDDEN, "fixture-sensitive-server-body").into_response();
        }
        let mut body = json!({"metadata":{},"items":[
         {"metadata":{"name":"widgets.a.example"},"spec":{"group":"a.example","names":{"plural":"widgets","kind":"Widget"},"scope":"Namespaced","versions":[{"name":"v1","served":true},{"name":"v2","served":true},{"name":"v0","served":false}]}},
         {"metadata":{"name":"widgets.b.example"},"spec":{"group":"b.example","names":{"plural":"widgets","kind":"Widget"},"scope":"Cluster","versions":[{"name":"v1","served":true}]}},
         {"spec":{"group":"../unsafe","names":{"plural":"pods","kind":"Secret"},"scope":"Namespaced","versions":[{"name":"v1","served":true}]}}
        ]});
        if s.large_catalog.load(std::sync::atomic::Ordering::SeqCst) {
            let page = query
                .split("continue=page")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            body["metadata"]["continue"] = json!(format!("page{}", page + 1));
            body["padding"] = json!("x".repeat(3 * 1024 * 1024));
        }
        return axum::Json(body).into_response();
    }
    let ns = if path.contains("/namespaces/") {
        path.split("/namespaces/")
            .nth(1)
            .unwrap()
            .split('/')
            .next()
            .unwrap()
    } else {
        "review"
    };
    if path.contains("/namespaces/slow/") {
        return axum::body::Body::from_stream(futures::stream::pending::<
            Result<Vec<u8>, std::io::Error>,
        >())
        .into_response();
    }
    if path.ends_with("/log") {
        return "fixture log\n".into_response();
    }
    if path.contains("/secrets") {
        let row = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"target","namespace":ns,"annotations":{"private":"fixture-sensitive-secret"}},"data":{"password":"fixture-sensitive-secret"}});
        return axum::Json(if path.ends_with("/secrets") {
            json!({"metadata":{},"items":[row]})
        } else {
            row
        })
        .into_response();
    }
    if path.ends_with("/target") {
        return axum::Json(json!({"kind":"Widget","metadata":{"name":"target","namespace":ns},"status":{"phase":"Running"}})).into_response();
    }
    let page = query
        .split("continue=page")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    if ["many", "scan", "pages", "exact", "bytes"].contains(&ns) {
        let count = if ns == "pages" || ns == "bytes" {
            1
        } else {
            200
        };
        let next = if ns == "exact" && page == 5 {
            String::new()
        } else {
            format!("page{}", page + 1)
        };
        let items:Vec<_>=(0..count).map(|i|json!({"metadata":{"name":format!("row-{page}-{i}"),"namespace":ns},"status":{"phase":"Running"}})).collect();
        return axum::Json(json!({"metadata":{"continue":next},"items":items,"padding":if ns=="bytes"{"x".repeat(1024*1024)}else{String::new()}})).into_response();
    }
    let name = if page == 1 { "front" } else { "target" };
    let mut token = if page == 1 {
        "page2"
    } else if page == 2 {
        "page3"
    } else {
        ""
    };
    if ns == "loop" {
        token = "page2";
    }
    let status = if page == 1 {
        "Pending"
    } else if page == 2 {
        "Running"
    } else {
        "Failed"
    };
    let namespace = if path.starts_with("/apis/b.example/") {
        ""
    } else if !path.contains("/namespaces/") && page == 3 {
        "ops"
    } else {
        ns
    };
    axum::Json(json!({"metadata":{"continue":token,"resourceVersion":"42"},"items":[{"metadata":{"name":name,"namespace":namespace},"status":{"phase":status}}]})).into_response()
}
async fn service() -> (Arc<KubeService>, ApiState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = ApiState::default();
    let router = Router::new().fallback(api).with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let service = Arc::new(KubeService::default());
    let config:Kubeconfig=serde_yaml::from_str(&format!("apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster: {{server: 'http://127.0.0.1:{port}'}}\nusers:\n- name: u\n  user: {{}}\ncontexts:\n- name: local\n  context: {{cluster: c, user: u, namespace: ops}}\n")).unwrap();
    service
        .open_config(config, Some("local"), None)
        .await
        .unwrap();
    (service, state, task)
}
fn input(id: u64, resource: &str, namespace: &str, search: &str) -> QueryInput {
    QueryInput {
        request_id: id,
        catalog_revision: 0,
        resource_id: resource.into(),
        namespace: namespace.into(),
        search: search.into(),
        continue_token: String::new(),
    }
}
#[tokio::test]
async fn query_search_reads_all_pages_in_selected_namespace_and_clear_uses_global_path() {
    let (s, state, server) = service().await;
    let g = s.hub.lock().await.generation;
    let result = s.query(g, input(1, "pods", "ops", "target")).await.unwrap();
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.scanned, 3);
    assert!(!result.truncated);
    assert_eq!(result.namespace, "ops");
    assert!(result.items.iter().all(|r| r.namespace == "ops"));
    let trace = state.trace.lock().unwrap().clone();
    assert_eq!(trace.len(), 3);
    assert!(
        trace
            .iter()
            .all(|p| p.starts_with("/api/v1/namespaces/ops/pods?"))
    );
    let result = s.query(g, input(2, "pods", "", "target")).await.unwrap();
    assert_eq!(result.items.len(), 2);
    assert!(
        result.items.iter().any(|r| r.namespace == "ops")
            && result.items.iter().any(|r| r.namespace == "review")
    );
    for ns in ["ops", "review"] {
        let detail = s
            .resource_detail(g, 0, result.scope_revision, "pods", ns, "target")
            .await
            .unwrap();
        assert!(detail.yaml.contains(&format!("namespace: {ns}")));
    }
    assert!(
        state.trace.lock().unwrap()[3..6]
            .iter()
            .all(|p| p.starts_with("/api/v1/pods?"))
    );
    let error = s
        .query(g, input(3, "pods", "denied", "target"))
        .await
        .err()
        .unwrap();
    assert!(error.contains("403"));
    assert!(!error.contains("fixture-sensitive"));
    assert!(s.query(g, input(2, "pods", "ops", "")).await.is_err());
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn cyclic_pagination_reports_incomplete_search_instead_of_a_complete_empty_list() {
    let (s, _, server) = service().await;
    let g = s.hub.lock().await.generation;
    let result = s
        .query(g, input(1, "pods", "loop", "absent"))
        .await
        .unwrap();
    assert!(result.truncated);
    assert!(result.reason.unwrap().contains("分页"));
    assert!(result.pages <= 3);
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn dynamic_catalog_distinguishes_versions_and_scope_and_rejects_forged_paths() {
    let (s, state, server) = service().await;
    let g = s.hub.lock().await.generation;
    let catalog = s.resources(g).await.unwrap();
    assert!(catalog.warning.is_some());
    let custom: Vec<_> = catalog
        .resources
        .iter()
        .filter(|r| r.id.starts_with("custom:"))
        .collect();
    assert_eq!(custom.len(), 3);
    assert!(custom.iter().all(|r| !r.id.contains("unsafe")));
    for (i, key) in [
        "custom:a.example/v1/widgets",
        "custom:a.example/v2/widgets",
        "custom:b.example/v1/widgets",
    ]
    .iter()
    .enumerate()
    {
        let mut q = input(i as u64 + 1, key, "ops", "");
        q.catalog_revision = catalog.catalog_revision;
        let page = s.query(g, q).await.unwrap();
        let ns = if key.contains("b.example") { "" } else { "ops" };
        assert_eq!(page.namespace, ns);
        let detail = s
            .resource_detail(
                g,
                catalog.catalog_revision,
                page.scope_revision,
                key,
                ns,
                "target",
            )
            .await
            .unwrap();
        assert!(detail.yaml.contains("Widget"));
    }
    let trace = state.trace.lock().unwrap().clone();
    assert!(
        trace
            .iter()
            .any(|p| p.starts_with("/apis/a.example/v2/namespaces/ops/widgets"))
    );
    assert!(
        trace
            .iter()
            .any(|p| p.starts_with("/apis/b.example/v1/widgets"))
    );
    let mut forged = input(4, "custom:../v1/pods", "ops", "");
    forged.catalog_revision = catalog.catalog_revision;
    assert!(s.query(g, forged).await.is_err());
    state
        .deny_catalog
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = s.resources(g).await.unwrap();
    assert_eq!(failed.catalog_revision, catalog.catalog_revision);
    assert!(failed.warning.unwrap().contains("403"));
    assert!(failed.resources.iter().any(|r| r.id == "pods"));
    assert!(
        failed
            .resources
            .iter()
            .any(|r| r.id == "custom:a.example/v1/widgets")
    );
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn new_scope_invalidates_old_row_identity_while_same_scope_refresh_preserves_it() {
    let (s, _, server) = service().await;
    let g = s.hub.lock().await.generation;
    let first = s.query(g, input(1, "pods", "ops", "")).await.unwrap();
    let second = s.query(g, input(2, "pods", "ops", "")).await.unwrap();
    assert_eq!(first.scope_revision, second.scope_revision);
    assert!(
        s.resource_detail(g, 0, first.scope_revision, "pods", "review", "target")
            .await
            .is_err()
    );
    s.query(g, input(3, "pods", "review", "")).await.unwrap();
    assert!(
        s.resource_detail(g, 0, first.scope_revision, "pods", "ops", "target")
            .await
            .is_err()
    );
    assert!(
        s.resource_detail(g, 0, first.scope_revision, "pods", "../ops", "target")
            .await
            .is_err()
    );
    s.close().await;
    server.abort();
}

#[tokio::test]
async fn full_search_distinguishes_incomplete_limits_from_an_exactly_complete_result() {
    let (s, _, server) = service().await;
    let g = s.hub.lock().await.generation;
    let matches = s.query(g, input(1, "pods", "many", "row")).await.unwrap();
    assert_eq!(matches.items.len(), 1000);
    assert!(matches.truncated);
    let scan = s
        .query(g, input(2, "pods", "scan", "absent"))
        .await
        .unwrap();
    assert_eq!(scan.scanned, 10000);
    assert!(scan.truncated);
    let pages = s
        .query(g, input(3, "pods", "pages", "absent"))
        .await
        .unwrap();
    assert_eq!(pages.pages, 64);
    assert!(pages.truncated);
    let complete = s.query(g, input(4, "pods", "exact", "row")).await.unwrap();
    assert_eq!(complete.items.len(), 1000);
    assert!(!complete.truncated);
    let bytes = s
        .query(g, input(5, "pods", "bytes", "absent"))
        .await
        .unwrap();
    assert!(bytes.truncated);
    assert!(bytes.bytes <= 32 * 1024 * 1024 + 1);
    assert!(
        bytes.bytes >= 32 * 1024 * 1024,
        "byte counter must include the partially read last page"
    );
    assert!(bytes.reason.unwrap().contains("传输"));
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn superseding_query_closes_old_http_body_and_only_the_new_identity_completes() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let server = tokio::spawn(async move {
        let (mut old, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            assert_eq!(old.read(&mut b).await.unwrap(), 1);
            request.push(b[0]);
        }
        old.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n{")
            .await
            .unwrap();
        notify.notify_one();
        let mut end = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), old.read(&mut end))
                .await
                .unwrap()
                .unwrap(),
            0,
            "old body transport must close"
        );
        let (mut fresh, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            assert_eq!(fresh.read(&mut b).await.unwrap(), 1);
            request.push(b[0]);
        }
        let body = b"{\"metadata\":{},\"items\":[]}";
        fresh
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        fresh.write_all(body).await.unwrap();
    });
    let service = Arc::new(KubeService::default());
    let config:Kubeconfig=serde_yaml::from_str(&format!("apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster: {{server: 'http://127.0.0.1:{port}'}}\nusers:\n- name: u\n  user: {{}}\ncontexts:\n- name: local\n  context: {{cluster: c, user: u}}\n")).unwrap();
    let g = service
        .open_config(config, Some("local"), None)
        .await
        .unwrap()
        .generation;
    let first_service = service.clone();
    let first =
        tokio::spawn(async move { first_service.query(g, input(1, "pods", "ops", "")).await });
    started.notified().await;
    let fresh = service
        .query(g, input(2, "pods", "review", ""))
        .await
        .unwrap();
    assert_eq!(fresh.request_id, 2);
    assert!(first.await.unwrap().is_err());
    assert!(service.query(g, input(1, "pods", "ops", "")).await.is_err());
    server.await.unwrap();
    service.close().await;
}

#[tokio::test]
async fn catalog_transfer_budget_publishes_validated_partial_entries_with_an_explicit_warning() {
    let (s, state, server) = service().await;
    let g = s.hub.lock().await.generation;
    state
        .large_catalog
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let catalog = s.resources(g).await.unwrap();
    assert!(catalog.warning.as_deref().unwrap().contains("不完整"));
    assert!(
        catalog
            .resources
            .iter()
            .any(|r| r.id == "custom:a.example/v1/widgets")
    );
    s.close().await;
    server.abort();
}

#[tokio::test]
async fn new_browser_secret_identity_requires_metadata_requests_and_removes_payloads() {
    let (s, state, server) = service().await;
    let g = s.hub.lock().await.generation;
    let page = s.query(g, input(1, "secrets", "ops", "")).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("fixture-sensitive")
    );
    let detail = s
        .resource_detail(g, 0, page.scope_revision, "secrets", "ops", "target")
        .await
        .unwrap();
    let yaml: Value = serde_yaml::from_str(&detail.yaml).unwrap();
    assert!(yaml.get("data").is_none());
    assert!(yaml.get("stringData").is_none());
    assert!(yaml.pointer("/metadata/annotations").is_none());
    assert!(!detail.yaml.contains("annotations:"));
    assert!(!detail.yaml.contains("fixture-sensitive"));
    assert!(
        state
            .accepts
            .lock()
            .unwrap()
            .iter()
            .all(|h| h.contains("PartialObjectMetadata"))
    );
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn all_namespace_pod_logs_use_the_selected_row_namespace_and_dynamic_resources_cannot_log() {
    let (s, state, server) = service().await;
    let g = s.hub.lock().await.generation;
    let page = s.query(g, input(1, "pods", "", "")).await.unwrap();
    for ns in ["ops", "review"] {
        let log = s
            .resource_logs_start(
                g,
                0,
                page.scope_revision,
                "pods",
                ns,
                "target",
                "app",
                false,
                10,
            )
            .await
            .unwrap();
        let mut done = false;
        for _ in 0..100 {
            let output = s.logs_poll(g, &log.log_id).await.unwrap();
            if output.done {
                assert!(output.error.is_none());
                done = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(done);
        assert!(
            state
                .trace
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains(&format!("/namespaces/{ns}/pods/target/log?")))
        );
    }
    let catalog = s.resources(g).await.unwrap();
    let mut q = input(2, "custom:a.example/v1/widgets", "ops", "");
    q.catalog_revision = catalog.catalog_revision;
    let custom = s.query(g, q).await.unwrap();
    assert!(
        s.resource_logs_start(
            g,
            catalog.catalog_revision,
            custom.scope_revision,
            "custom:a.example/v1/widgets",
            "ops",
            "target",
            "app",
            false,
            10
        )
        .await
        .is_err()
    );
    s.close().await;
    server.abort();
}
#[tokio::test]
async fn full_search_total_deadline_returns_an_explicit_incomplete_result() {
    let (s, _, server) = service().await;
    let g = s.hub.lock().await.generation;
    let started = std::time::Instant::now();
    let page = s
        .query(g, input(1, "pods", "slow", "target"))
        .await
        .unwrap();
    assert!(page.truncated);
    assert!(page.reason.unwrap().contains("30秒"));
    assert!(started.elapsed() < Duration::from_secs(35));
    s.close().await;
    server.abort();
}

#[tokio::test]
async fn known_http_headers_fail_without_waiting_for_a_hanging_error_body() {
    for code in [403, 500] {
        let (s, state, server) = service().await;
        let g = s.hub.lock().await.generation;
        state
            .header_error
            .store(code, std::sync::atomic::Ordering::SeqCst);
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            s.query(g, input(1, "pods", "headers-error", "absent")),
        )
        .await;
        assert!(
            result.is_ok(),
            "known HTTP {code} must fail before waiting for body"
        );
        let error = result
            .unwrap()
            .err()
            .expect("HTTP failure cannot publish a partial search");
        assert!(error.contains(&code.to_string()));
        s.close().await;
        server.abort();
    }
}
#[tokio::test]
async fn directory_known_header_failure_preserves_the_previous_catalog_immediately() {
    for code in [403, 500] {
        let (s, state, server) = service().await;
        let g = s.hub.lock().await.generation;
        let initial = s.resources(g).await.unwrap();
        state
            .header_error
            .store(code, std::sync::atomic::Ordering::SeqCst);
        state
            .catalog_header_error
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(1), s.resources(g)).await;
        assert!(
            result.is_ok(),
            "known HTTP status must not wait for CRD body"
        );
        let result = result.unwrap().unwrap();
        assert_eq!(result.catalog_revision, initial.catalog_revision);
        assert!(result.resources == initial.resources);
        assert!(result.warning.unwrap().contains(&code.to_string()));
        s.close().await;
        server.abort();
    }
}
#[tokio::test]
async fn failing_budget_tail_page_cannot_be_published_as_a_partial_search() {
    for code in [403, 500] {
        let (s, state, server) = service().await;
        let g = s.hub.lock().await.generation;
        state
            .header_error
            .store(code, std::sync::atomic::Ordering::SeqCst);
        let result = s.query(g, input(1, "pods", "error-tail", "absent")).await;
        assert!(
            result.is_err(),
            "HTTP {code} after successful pages cannot become budget success"
        );
        assert!(result.err().unwrap().contains(&code.to_string()));
        s.close().await;
        server.abort();
    }
}
#[tokio::test]
async fn failing_budget_tail_directory_keeps_the_previous_catalog_and_row_scope() {
    for code in [403, 500] {
        let (s, state, server) = service().await;
        let g = s.hub.lock().await.generation;
        let initial = s.resources(g).await.unwrap();
        let mut q = input(1, "custom:a.example/v1/widgets", "ops", "");
        q.catalog_revision = initial.catalog_revision;
        let shown = s.query(g, q).await.unwrap();
        state
            .catalog_tail_error
            .store(code, std::sync::atomic::Ordering::SeqCst);
        let result = s.resources(g).await.unwrap();
        assert_eq!(
            result.catalog_revision, initial.catalog_revision,
            "HTTP error cannot replace a good catalog"
        );
        assert!(result.resources == initial.resources);
        assert!(result.warning.unwrap().contains(&code.to_string()));
        assert!(
            s.resource_detail(
                g,
                initial.catalog_revision,
                shown.scope_revision,
                "custom:a.example/v1/widgets",
                "ops",
                "target"
            )
            .await
            .is_ok()
        );
        s.close().await;
        server.abort();
    }
}
