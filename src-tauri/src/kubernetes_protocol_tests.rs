use super::*;
use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::sync::Mutex as StdMutex;

#[derive(Clone, Default)]
struct Fixture {
    trace: Arc<StdMutex<Vec<String>>>,
    stalled: Arc<tokio::sync::Notify>,
}

async fn api(State(state): State<Fixture>, method: Method, uri: Uri) -> Response {
    state.trace.lock().unwrap().push(format!("{method} {uri}"));
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = uri.path();
    let query = uri.query().unwrap_or("");
    if path.ends_with("/denied") || path.contains("/namespaces/denied/") {
        return (StatusCode::FORBIDDEN,axum::Json(json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Forbidden","message":"fixture-sensitive-server-error","code":403}))).into_response();
    }
    if path.contains("/namespaces/large/") {
        return vec![b'x'; 70 * 1024].into_response();
    }
    if path.ends_with("/stall") {
        state.stalled.notify_one();
        std::future::pending::<()>().await;
    }
    if path.ends_with("/log") {
        let bytes = "2026-11-11T01:00:00Z 容器日志😀\n".as_bytes();
        let chunks = vec![
            Ok::<_, std::io::Error>(bytes[..22].to_vec()),
            Ok(bytes[22..24].to_vec()),
            Ok(bytes[24..].to_vec()),
        ];
        return axum::body::Body::from_stream(futures::stream::iter(chunks)).into_response();
    }
    if query.contains("watch=true") {
        if query.contains("resourceVersion=expired") {
            return (StatusCode::GONE,axum::Json(json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Expired","message":"expired","code":410}))).into_response();
        }
        return format!("{}\n",json!({"type":"MODIFIED","object":{"metadata":{"name":"api","namespace":"ops","resourceVersion":"43"},"status":{"phase":"Running"}}})).into_response();
    }
    if path == "/api/v1/namespaces" {
        return axum::Json(json!({"metadata":{},"items":[{"metadata":{"name":"ops"}},{"metadata":{"name":"review"}}]})).into_response();
    }
    if path.ends_with("/secrets/pin") {
        return axum::Json(json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"pin","namespace":"ops","annotations":{"secret":"fixture-sensitive"}},"data":{"password":"fixture-sensitive"}})).into_response();
    }
    if path.ends_with("/pods/api") {
        return axum::Json(json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"api","namespace":"ops"},"spec":{"containers":[{"name":"app"}]},"status":{"phase":"Running"}})).into_response();
    }
    axum::Json(json!({"metadata":{"resourceVersion":"42","continue":"cursor-2"},"items":[{"metadata":{"name":"api","namespace":"ops"},"spec":{"containers":[{"name":"app"}]},"status":{"phase":"Pending"}}]})).into_response()
}

async fn fixture() -> (KubeService, KubeSelection, Fixture, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Fixture::default();
    let router = Router::new().fallback(api).with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let path = std::env::temp_dir().join(format!("aic-ops-kube-fixture-{}.yaml", Uuid::new_v4()));
    std::fs::write(&path,format!("apiVersion: v1\nkind: Config\ncurrent-context: local\nclusters:\n- name: c\n  cluster:\n    server: http://127.0.0.1:{port}\nusers:\n- name: u\n  user: {{}}\ncontexts:\n- name: local\n  context: {{cluster: c, user: u, namespace: ops}}\n- name: second\n  context: {{cluster: c, user: u, namespace: review}}\n")).unwrap();
    let service = KubeService::default();
    let selection = service.open(path.to_str().unwrap(), None).await.unwrap();
    std::fs::remove_file(path).unwrap();
    (service, selection, state, task)
}

#[tokio::test]
async fn get_only_protocol_lists_pages_details_watch_and_utf8_logs() {
    let (service, selection, state, task) = fixture().await;
    let g = selection.generation;
    assert_eq!(
        service.namespaces(g).await.unwrap().namespaces,
        vec!["ops", "review"]
    );
    let page = service.list(g, "pods", 500, "cursor-1").await.unwrap();
    assert_eq!(page.items[0].name, "api");
    assert_eq!(page.continue_token, "cursor-2");
    assert_eq!(page.resource_version, "42");
    let detail = service.detail(g, "pods", "api").await.unwrap();
    assert_eq!(detail.containers, vec!["app"]);
    assert!(detail.yaml.contains("Running"));
    let secret = service.detail(g, "secrets", "pin").await.unwrap();
    assert!(!secret.yaml.contains("fixture-sensitive"));
    let secret_yaml: Value = serde_yaml::from_str(&secret.yaml).unwrap();
    assert!(secret_yaml.get("data").is_none());
    let update = service.watch(g, "pods", "42").await.unwrap();
    assert_eq!(update.changes[0].event, "MODIFIED");
    assert_eq!(update.changes[0].row.status, "Running");
    assert_eq!(update.resource_version, "43");
    let expired = service.watch(g, "pods", "expired").await.unwrap();
    assert!(expired.expired);
    let log = service
        .logs_start(g, "api", "app", true, 99999)
        .await
        .unwrap();
    let mut output = String::new();
    let mut done = false;
    for _ in 0..100 {
        let value = service.logs_poll(g, &log.log_id).await.unwrap();
        output.push_str(&value.text);
        if value.done {
            assert!(value.error.is_none());
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(done);
    assert_eq!(output, "2026-11-11T01:00:00Z 容器日志😀\n");
    service.logs_stop(g, None).await.unwrap();
    assert!(service.logs_poll(g, &log.log_id).await.is_err());
    let trace = state.trace.lock().unwrap().clone();
    assert!(trace.iter().all(|s| s.starts_with("GET ")));
    assert!(
        trace
            .iter()
            .any(|s| s.contains("limit=200") && s.contains("continue=cursor-1"))
    );
    assert!(
        trace
            .iter()
            .any(|s| s.contains("/api/v1/namespaces/ops/pods/api/log")
                && s.contains("container=app")
                && s.contains("tailLines=1000"))
    );
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn cancellation_namespace_switch_and_errors_never_return_old_or_sensitive_data() {
    let (service, selection, state, task) = fixture().await;
    let g = selection.generation;
    let message = service.detail(g, "pods", "denied").await.err().unwrap();
    assert!(message.contains("403"));
    assert!(!message.contains("sensitive"));
    let result = tokio::join!(service.detail(g, "pods", "stall"), async {
        state.stalled.notified().await;
        service.stop(g).await.unwrap()
    });
    assert!(result.0.err().unwrap().contains("取消"));
    let new_g = result.1.generation;
    assert!(service.list(g, "pods", 100, "").await.is_err());
    let selected = service
        .select(new_g, "second", Some("review"))
        .await
        .unwrap();
    service
        .list(selected.generation, "deployments", 10, "")
        .await
        .unwrap();
    assert!(
        state
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("/apis/apps/v1/namespaces/review/deployments"))
    );
    assert!(
        service
            .list(selected.generation, "pods/exec", 100, "")
            .await
            .is_err()
    );
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn failed_context_switch_keeps_the_previous_selection_usable() {
    let (service, selection, _, task) = fixture().await;
    {
        let mut hub = service.hub.lock().await;
        let config = &mut hub.session.as_mut().unwrap().config;
        config
            .contexts
            .iter_mut()
            .find(|c| c.name == "second")
            .unwrap()
            .context
            .as_mut()
            .unwrap()
            .cluster = "missing-fixture-cluster".into();
    }
    assert!(
        service
            .select(selection.generation, "second", None)
            .await
            .is_err()
    );
    assert!(
        service
            .list(selection.generation, "pods", 10, "")
            .await
            .is_ok(),
        "failed target must not strand the old generation"
    );
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn a_stop_between_log_snapshot_and_install_cannot_resurrect_the_stream() {
    let (service, selection, _, task) = fixture().await;
    let g = selection.generation;
    let guard = service.hub.lock().await;
    let start = service.logs_start(g, "api", "app", true, 10);
    let stop = service.logs_stop(g, None);
    futures::pin_mut!(start, stop);
    assert!(futures::poll!(start.as_mut()).is_pending());
    assert!(futures::poll!(stop.as_mut()).is_pending());
    drop(guard);
    assert!(futures::poll!(start.as_mut()).is_pending());
    stop.await.unwrap();
    assert!(
        start.await.is_err(),
        "a stopped in-flight start must not install a hidden log stream"
    );
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn explicit_context_opens_a_config_without_current_context() {
    let (service, _, _, task) = fixture().await;
    let mut config = service
        .hub
        .lock()
        .await
        .session
        .as_ref()
        .unwrap()
        .config
        .clone();
    config.current_context = None;
    let path = std::env::temp_dir().join(format!("aic-ops-context-{}.yaml", Uuid::new_v4()));
    std::fs::write(&path, serde_yaml::to_string(&config).unwrap()).unwrap();
    assert!(service.open(path.to_str().unwrap(), None).await.is_err());
    let selected = service
        .open(path.to_str().unwrap(), Some("second"))
        .await
        .unwrap();
    assert_eq!(selected.context, "second");
    assert_eq!(selected.namespace, "review");
    std::fs::remove_file(path).unwrap();
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn runtime_ids_isolate_same_generation_responses_logs_and_terminal_close() {
    let (service, _, state, task) = fixture().await;
    let config = service
        .hub
        .lock()
        .await
        .session
        .as_ref()
        .unwrap()
        .config
        .clone();
    let runtimes = KubeRuntimes::default();
    let a = runtimes
        .open_config(config.clone(), None, None)
        .await
        .unwrap();
    let b = runtimes.open_config(config, None, None).await.unwrap();
    assert_ne!(a.session_id, b.session_id);
    assert_eq!(a.data.generation, b.data.generation);
    let ag = a.data.generation;
    let bg = b.data.generation;
    let sa = runtimes.get(&a.session_id).await.unwrap();
    let sb = runtimes.get(&b.session_id).await.unwrap();
    let logb = sb.logs_start(bg, "api", "app", true, 10).await.unwrap();
    let pending = sa.detail(ag, "pods", "stall");
    let closed = async {
        state.stalled.notified().await;
        runtimes.close(&a.session_id, ag).await.unwrap()
    };
    let (late, _) = tokio::join!(pending, closed);
    assert!(late.is_err());
    assert!(runtimes.get(&a.session_id).await.is_err());
    assert!(sb.list(bg, "pods", 10, "").await.is_ok());
    assert!(sb.watch(bg, "pods", "42").await.is_ok());
    assert!(sb.logs_poll(bg, &logb.log_id).await.is_ok());
    assert!(runtimes.close(&b.session_id, bg + 100).await.is_err());
    assert!(runtimes.get(&b.session_id).await.is_ok());
    runtimes.close(&a.session_id, ag).await.unwrap();
    let selected = sb.select(bg, "second", None).await.unwrap();
    assert!(selected.generation > bg);
    runtimes.close(&b.session_id, bg).await.unwrap();
    assert!(sb.list(selected.generation, "pods", 10, "").await.is_err());
    service.close().await;
    task.abort();
}

#[tokio::test]
async fn draft_kube_probe_uses_one_bounded_get_without_registering_a_runtime() {
    let (service, selection, state, task) = fixture().await;
    let config = service
        .hub
        .lock()
        .await
        .session
        .as_ref()
        .unwrap()
        .config
        .clone();
    test_connection(config.clone(), Some("local"), Some("ops"))
        .await
        .unwrap();
    assert!(
        test_connection(config.clone(), Some("unknown"), Some("ops"))
            .await
            .is_err()
    );
    assert!(
        test_connection(config.clone(), Some("local"), Some("denied"))
            .await
            .err()
            .unwrap()
            .contains("403")
    );
    assert!(
        test_connection(config, Some("local"), Some("large"))
            .await
            .err()
            .unwrap()
            .contains("64KiB")
    );
    assert_eq!(service.hub.lock().await.generation, selection.generation);
    let trace = state.trace.lock().unwrap();
    assert!(trace.iter().all(|q| q.starts_with("GET ")));
    assert!(
        trace
            .iter()
            .any(|q| q.contains("/namespaces/ops/pods?") && q.contains("limit=1"))
    );
    drop(trace);
    service.close().await;
    task.abort();
}

// Raw HTTP fixture: error status must never select kube-client's unbounded collector.
async fn bounded_error_probe(status: u16, chunked: bool, oversized: bool) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /api/v1/namespaces/ops/pods?"));
        assert!(request.contains("limit=1"));
        let total = if oversized {
            if chunked { 1024 * 1024 } else { 70 * 1024 }
        } else {
            48
        };
        let framing = if chunked {
            "Transfer-Encoding: chunked".to_string()
        } else {
            format!("Content-Length: {total}")
        };
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {status} Error\r\n{framing}\r\nContent-Type: application/json\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (mut read, mut write) = socket.into_split();
        let mut sent = 0;
        let mut eof = [0];
        loop {
            if sent >= total {
                if chunked {
                    let _ = write.write_all(b"0\r\n\r\n").await;
                }
                let closed = matches!(
                    tokio::time::timeout(Duration::from_secs(2), read.read(&mut eof)).await,
                    Ok(Ok(0)) | Ok(Err(_))
                );
                return (sent, closed);
            }
            let count = (total - sent).min(4096);
            let mut bytes = vec![b'x'; count];
            let marker = b"fixture-sensitive-error-sentinel";
            if count >= marker.len() {
                bytes[..marker.len()].copy_from_slice(marker);
            }
            let send = async {
                if chunked {
                    write.write_all(format!("{count:x}\r\n").as_bytes()).await?;
                }
                write.write_all(&bytes).await?;
                if chunked {
                    write.write_all(b"\r\n").await?;
                }
                Ok::<_, std::io::Error>(())
            };
            tokio::select! {
                closed = read.read(&mut eof) => return (sent, matches!(closed, Ok(0) | Err(_))),
                done = send => if done.is_err() { return (sent, true); }
            }
            sent += count;
            if chunked {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    });
    let config: Kubeconfig = serde_yaml::from_str(&format!("apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster: {{server: 'http://127.0.0.1:{port}'}}\nusers:\n- name: u\n  user: {{}}\ncontexts:\n- name: local\n  context: {{cluster: c, user: u}}\n")).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        test_connection(config, Some("local"), Some("ops")),
    )
    .await
    .unwrap();
    let error = result.unwrap_err();
    let (sent, closed) = server.await.unwrap();
    assert!(closed, "temporary HTTP transport must close");
    assert!(!error.contains("fixture-sensitive"));
    if oversized {
        assert!(
            error.contains("64KiB"),
            "oversized HTTP {status} must hit byte limit: {error}"
        );
        if chunked {
            assert!(sent < 128 * 1024, "must stop streaming early: {sent}");
        }
    } else {
        assert!(error.contains(&status.to_string()));
    }
}
#[tokio::test]
async fn draft_probe_bounds_large_403_error_body() {
    bounded_error_probe(403, false, true).await;
}
#[tokio::test]
async fn draft_probe_bounds_large_500_error_body() {
    bounded_error_probe(500, false, true).await;
}
#[tokio::test]
async fn draft_probe_stops_chunked_403_error_early() {
    bounded_error_probe(403, true, true).await;
}
#[tokio::test]
async fn draft_probe_stops_chunked_500_error_early() {
    bounded_error_probe(500, true, true).await;
}
#[tokio::test]
async fn draft_probe_sanitizes_short_auth_errors() {
    bounded_error_probe(401, false, false).await;
    bounded_error_probe(403, true, false).await;
}
