use super::config::*;
use super::naming::*;
use super::*;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::Response,
    routing::any,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
#[derive(Default)]
struct Data {
    readback_lag: usize,
    pending_instance: Option<Value>,
    config: Option<Value>,
    history: Vec<Value>,
    mutations: Vec<(String, HashMap<String, String>, Option<String>)>,
    service: Value,
    instance: Value,
    read_delay_ms: u64,
    write_delay_ms: u64,
    write_attempts: usize,
    good_history: bool,
    cas_race: bool,
    write_failure: u16,
    corrupt_write_reply: bool,
    hold: Option<Arc<(tokio::sync::Notify, tokio::sync::Notify)>>,
}
fn json_response(status: u16, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}
fn empty(status: u16) -> Response {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}
async fn serve(State(data): State<Arc<Mutex<Data>>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().as_str().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let cas = request
        .headers()
        .get("casMd5")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = axum::body::to_bytes(request.into_body(), 5 * 1024 * 1024)
        .await
        .unwrap();
    let params: HashMap<String, String> =
        url::form_urlencoded::parse(if method == "GET" || method == "DELETE" {
            query.as_bytes()
        } else {
            &body
        })
        .into_owned()
        .collect();
    let delay = {
        let state = data.lock().unwrap();
        if path.ends_with("/cs/configs") {
            if method == "GET" {
                state.read_delay_ms
            } else {
                state.write_delay_ms
            }
        } else {
            0
        }
    };
    if method == "POST" && path.ends_with("/cs/configs") {
        data.lock().unwrap().write_attempts += 1;
    }
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    let hold = data.lock().unwrap().hold.clone();
    if method == "POST" && path.ends_with("/cs/configs") {
        if let Some(hold) = hold {
            hold.0.notify_one();
            hold.1.notified().await;
        }
    }
    let mut state = data.lock().unwrap();
    if path.ends_with("/server/state") {
        return json_response(200, json!({"version":"2.3.2"}));
    }
    if path.ends_with("/console/namespaces") {
        return json_response(
            200,
            json!({"code":200,"data":[{"namespace":"","namespaceShowName":"public"}]}),
        );
    }
    if path.ends_with("/cs/configs") {
        if params.get("tenant").map(String::as_str) != Some("id-a") {
            return json_response(400, json!({"message":"wrong namespace"}));
        }
        if method == "GET" {
            if params.contains_key("search") {
                return json_response(
                    200,
                    json!({"totalCount":1,"pageNumber":1,"pagesAvailable":1,"pageItems":state.config.iter().cloned().collect::<Vec<_>>()}),
                );
            }
            return match state.config.clone() {
                Some(row) => json_response(200, row),
                None => empty(200),
            };
        }
        if method == "POST" {
            state
                .mutations
                .push((method.clone(), params.clone(), cas.clone()));
            if state.write_failure != 0 {
                return json_response(
                    state.write_failure,
                    json!({"message":"never-reflect-fixture-secret"}),
                );
            }
            if state.cas_race {
                state.config.as_mut().unwrap()["md5"] = "00000000000000000000000000000000".into();
            }
            if let Some(old) = state.config.as_ref() {
                if cas.as_deref() != old["md5"].as_str() {
                    return json_response(
                        500,
                        json!({"errorCode":30003,"message":"Cas publish fail"}),
                    );
                }
            }
            let content = params.get("content").cloned().unwrap_or_default();
            let md5 = format!("{:x}", md5::compute(content.as_bytes()));
            let next = json!({"id":1,"dataId":"file.yaml","group":"BLUE","tenant":"id-a","content":content,"md5":md5,"type":params.get("type"),"appName":params.get("appName"),"desc":params.get("desc")});
            state.history.push(next.clone());
            state.config = Some(next);
            if state.corrupt_write_reply {
                return Response::new(Body::from("not-json never-reflect-fixture-secret"));
            }
            return json_response(200, json!(true));
        }
        if method == "DELETE" {
            state.mutations.push((method.clone(), params.clone(), None));
            state.config = None;
            return json_response(200, json!(true));
        }
    }
    if path.ends_with("/cs/history") {
        if params.contains_key("nid") && state.good_history {
            return json_response(
                200,
                state.history[params["nid"].parse::<usize>().unwrap() - 1].clone(),
            );
        }
        if params.contains_key("nid") {
            return json_response(
                200,
                json!({"id":params["nid"],"dataId":"other.yaml","group":"BLUE","tenant":"id-a","content":"wrong target","md5":"wrong"}),
            );
        }
        return json_response(
            200,
            json!({"totalCount":state.history.len(),"pageItems":state.history.iter().cloned().map(|mut row|{row.as_object_mut().unwrap().remove("content");row.as_object_mut().unwrap().remove("md5");row}).collect::<Vec<_>>(),"pageNumber":1,"pagesAvailable":1}),
        );
    }
    if path.ends_with("/catalog/services") {
        return json_response(
            200,
            json!({"count":1,"serviceList":[{"name":"api","groupName":"BLUE","clusterCount":1,"ipCount":1,"healthyInstanceCount":1}]}),
        );
    }
    if path.ends_with("/ns/service") {
        if params.get("serviceName").map(String::as_str) != Some("BLUE@@api") {
            return empty(400);
        }
        if method == "PUT" {
            state.mutations.push((method.clone(), params.clone(), None));
            state.service["protectThreshold"] =
                params["protectThreshold"].parse::<f64>().unwrap().into();
            state.service["metadata"] = serde_json::from_str(&params["metadata"]).unwrap();
            state.service["selector"] = serde_json::from_str(&params["selector"]).unwrap();
            return Response::new(Body::from("ok"));
        }
        return json_response(200, state.service.clone());
    }
    if path.ends_with("/ns/instance/list") {
        let hosts = if state.instance["enabled"] == false {
            vec![]
        } else {
            vec![state.instance.clone()]
        };
        return json_response(200, json!({"hosts":hosts}));
    }
    if path.ends_with("/catalog/instances") {
        if params.get("clusterName").map(String::as_str) != Some("ALT") {
            return empty(500);
        }
        let row = if state.pending_instance.is_some() && state.readback_lag > 0 {
            state.readback_lag -= 1;
            state.pending_instance.clone().unwrap()
        } else {
            state.pending_instance = None;
            state.instance.clone()
        };
        return json_response(200, json!({"list":[row],"count":1}));
    }
    if path.ends_with("/ns/instance") {
        if method == "PUT" {
            state.mutations.push((method.clone(), params.clone(), None));
            if params.get("clusterName").map(String::as_str) != Some("ALT")
                || params.get("ephemeral").map(String::as_str) != Some("false")
            {
                return empty(400);
            }
            if state.readback_lag > 0 {
                state.pending_instance = Some(state.instance.clone());
            }
            state.instance["enabled"] = params["enabled"].parse::<bool>().unwrap().into();
            state.instance["weight"] = params["weight"].parse::<f64>().unwrap().into();
            state.instance["metadata"] = serde_json::from_str(&params["metadata"]).unwrap();
            return Response::new(Body::from("ok"));
        }
        let mut detail = state.instance.clone();
        detail.as_object_mut().unwrap().remove("enabled");
        detail.as_object_mut().unwrap().remove("ephemeral");
        return json_response(200, detail);
    }
    empty(404)
}
async fn fixture() -> (
    NacosService,
    String,
    Arc<Mutex<Data>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let content = "line: value\r\n# retained\n";
    let data = Arc::new(Mutex::new(Data {
        config: Some(
            json!({"id":1,"dataId":"file.yaml","group":"BLUE","tenant":"id-a","content":content,"md5":format!("{:x}",md5::compute(content)),"type":"yaml","appName":"keep-app","desc":"keep-description"}),
        ),
        service: json!({"namespaceId":"public","groupName":"BLUE","name":"api","clusters":[{"name":"ALT"}],"protectThreshold":0.2,"metadata":{"keep":"value"},"selector":{"type":"label","expression":"v=blue"}}),
        instance: json!({"ip":"127.0.0.1","port":12345,"clusterName":"ALT","ephemeral":false,"enabled":true,"healthy":false,"weight":1.0,"metadata":{"old":"value"}}),
        ..Default::default()
    }));
    let app = Router::new().fallback(any(serve)).with_state(data.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let service = NacosService::default();
    let session = service
        .connect(
            NacosSettings {
                endpoint: format!("http://127.0.0.1:{port}/nacos"),
                authentication: "none".into(),
                namespace: "".into(),
            },
            None,
        )
        .await
        .unwrap();
    (service, session.id, data, server)
}
fn key() -> ConfigKey {
    ConfigKey {
        namespace_id: "id-a".into(),
        group: "BLUE".into(),
        data_id: "file.yaml".into(),
    }
}
fn service_key() -> ServiceKey {
    ServiceKey {
        namespace_id: "".into(),
        group: "BLUE".into(),
        name: "api".into(),
    }
}
#[tokio::test]
async fn config_publish_preserves_exact_content_and_attributes_cas_conflicts_never_write_and_history_stays_scoped()
 {
    let (service, id, data, server) = fixture().await;
    let page = service.configs(&id, "id-a", "", "", 1, 20).await.unwrap();
    assert_eq!(page.total, 1);
    let old = service.config(&id, key()).await.unwrap().unwrap();
    assert_eq!(old.content, "line: value\r\n# retained\n");
    assert!(
        service
            .publish(
                &id,
                key(),
                "next\n".into(),
                "yaml".into(),
                Some("stale".into())
            )
            .await
            .is_err()
    );
    assert!(data.lock().unwrap().mutations.is_empty());
    let changed = service
        .publish(&id, key(), "next\n".into(), "yaml".into(), Some(old.md5))
        .await
        .unwrap();
    assert_eq!(changed.content, "next\n");
    let state = data.lock().unwrap();
    assert_eq!(state.mutations[0].1["appName"], "keep-app");
    assert_eq!(state.mutations[0].1["desc"], "keep-description");
    drop(state);
    assert_eq!(service.history(&id, key(), 1, 20).await.unwrap().total, 1);
    assert!(
        service
            .history_config(&id, key(), "1".into())
            .await
            .is_err()
    );
    service
        .delete_config(&id, key(), changed.md5)
        .await
        .unwrap();
    assert!(data.lock().unwrap().config.is_none());
    server.abort();
}
#[tokio::test]
async fn naming_updates_preserve_selector_and_instance_identity_while_rejecting_stale_snapshots() {
    let (service, id, data, server) = fixture().await;
    assert_eq!(
        service
            .services(&id, "", "", "", 1, 20)
            .await
            .unwrap()
            .total,
        1
    );
    let current = service.service(&id, service_key()).await.unwrap();
    let selector = current.selector.clone();
    let mut meta = BTreeMap::new();
    meta.insert("new".into(), "value".into());
    assert!(
        service
            .update_service(&id, service_key(), "stale".into(), 0.3, meta.clone())
            .await
            .is_err()
    );
    assert!(data.lock().unwrap().mutations.is_empty());
    let changed = service
        .update_service(&id, service_key(), current.snapshot, 0.3, meta.clone())
        .await
        .unwrap();
    assert_eq!(changed.selector, selector);
    assert_eq!(changed.protect_threshold, 0.3);
    let instance = &changed.instances[0];
    assert!(!instance.healthy);
    meta.extend(instance.metadata.clone());
    data.lock().unwrap().readback_lag = 2;
    let updated = service
        .update_instance(
            &id,
            service_key(),
            instance.key.clone(),
            instance.snapshot.clone(),
            false,
            2.5,
            meta,
        )
        .await
        .unwrap();
    assert!(!updated.enabled);
    assert!(!updated.healthy);
    assert_eq!(updated.weight, 2.5);
    let state = data.lock().unwrap();
    let last = state.mutations.last().unwrap();
    assert_eq!(last.1["clusterName"], "ALT");
    assert_eq!(last.1["ephemeral"], "false");
    assert!(!last.1.contains_key("healthy"));
    drop(state);
    server.abort();
}

#[tokio::test]
async fn history_rollback_uses_verified_target_and_preserves_current_attributes() {
    let (service, id, data, server) = fixture().await;
    let initial = service.config(&id, key()).await.unwrap().unwrap();
    let first = service
        .publish(
            &id,
            key(),
            "unicode: 中文\r\n".into(),
            "yaml".into(),
            Some(initial.md5),
        )
        .await
        .unwrap();
    let second = service
        .publish(
            &id,
            key(),
            "second\n".into(),
            "yaml".into(),
            Some(first.md5),
        )
        .await
        .unwrap();
    data.lock().unwrap().good_history = true;
    let rolled = service
        .rollback(&id, key(), "1".into(), second.md5)
        .await
        .unwrap();
    assert_eq!(rolled.content, "unicode: 中文\r\n");
    assert_eq!(
        data.lock().unwrap().mutations.last().unwrap().1["appName"],
        "keep-app"
    );
    assert!(
        service
            .history_config(&id, key(), "../1".into())
            .await
            .is_err()
    );
    server.abort();
}
#[tokio::test]
async fn server_cas_race_is_a_conflict_and_is_never_replayed() {
    let (service, id, data, server) = fixture().await;
    let initial = service.config(&id, key()).await.unwrap().unwrap();
    data.lock().unwrap().cas_race = true;
    let error = service
        .publish(
            &id,
            key(),
            "not-stored".into(),
            "yaml".into(),
            Some(initial.md5),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error, CONFLICT);
    assert_eq!(data.lock().unwrap().mutations.len(), 1);
    assert_ne!(
        data.lock().unwrap().config.as_ref().unwrap()["content"],
        "not-stored"
    );
    server.abort();
}
#[tokio::test]
async fn rejected_or_unparseable_write_responses_never_retry_or_claim_a_verified_result() {
    for (status, corrupt) in [(403, false), (500, false), (0, true)] {
        let (service, id, data, server) = fixture().await;
        let old = service.config(&id, key()).await.unwrap().unwrap();
        {
            let mut state = data.lock().unwrap();
            state.write_failure = status;
            state.corrupt_write_reply = corrupt;
        }
        let error = service
            .publish(&id, key(), "changed".into(), "yaml".into(), Some(old.md5))
            .await
            .err()
            .unwrap();
        assert!(!error.contains("never-reflect"));
        assert_eq!(data.lock().unwrap().mutations.len(), 1);
        if status != 403 {
            assert_eq!(error, UNKNOWN);
        } else {
            assert!(error.contains("403"));
        }
        server.abort();
    }
}
#[tokio::test]
async fn close_refuses_an_inflight_write_and_then_closes_only_its_own_runtime() {
    let (service, id, data, server) = fixture().await;
    let service = Arc::new(service);
    let old = service.config(&id, key()).await.unwrap().unwrap();
    let gate = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
    data.lock().unwrap().hold = Some(gate.clone());
    let target = service.clone();
    let session = id.clone();
    let task = tokio::spawn(async move {
        target
            .publish(&session, key(), "next".into(), "yaml".into(), Some(old.md5))
            .await
    });
    gate.0.notified().await;
    assert!(service.close(&id).unwrap_err().contains("写操作"));
    gate.1.notify_one();
    task.await.unwrap().unwrap();
    service.close(&id).unwrap();
    assert!(service.config(&id, key()).await.is_err());
    server.abort();
}
#[tokio::test]
async fn invalid_targets_content_metadata_or_pages_do_not_issue_mutations() {
    let (service, id, data, server) = fixture().await;
    let old = service.config(&id, key()).await.unwrap().unwrap();
    assert!(
        service
            .publish(
                &id,
                key(),
                "x".repeat(1024 * 1024 + 1),
                "yaml".into(),
                Some(old.md5.clone())
            )
            .await
            .is_err()
    );
    assert!(
        service
            .publish(&id, key(), "x".into(), "invalid".into(), Some(old.md5))
            .await
            .is_err()
    );
    assert!(service.configs(&id, "", "", "", 0, 20).await.is_err());
    assert!(service.services(&id, "", "", "", 1, 201).await.is_err());
    let current = service.service(&id, service_key()).await.unwrap();
    let target = current.instances[0].clone();
    assert!(
        service
            .update_instance(
                &id,
                service_key(),
                target.key.clone(),
                "stale".into(),
                false,
                1.0,
                BTreeMap::new()
            )
            .await
            .is_err()
    );
    assert!(
        service
            .update_instance(
                &id,
                service_key(),
                target.key,
                target.snapshot,
                false,
                f64::NAN,
                BTreeMap::new()
            )
            .await
            .is_err()
    );
    let mut huge = BTreeMap::new();
    for i in 0..257 {
        huge.insert(i.to_string(), "value".into());
    }
    assert!(
        service
            .update_service(&id, service_key(), current.snapshot, 0.1, huge)
            .await
            .is_err()
    );
    assert!(data.lock().unwrap().mutations.is_empty());
    server.abort();
}
#[tokio::test]
async fn a_second_write_is_rejected_while_the_first_is_pending_instead_of_queued() {
    let (service, id, data, server) = fixture().await;
    let service = Arc::new(service);
    let old = service.config(&id, key()).await.unwrap().unwrap();
    let gate = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
    data.lock().unwrap().hold = Some(gate.clone());
    let target = service.clone();
    let session = id.clone();
    let md5 = old.md5.clone();
    let first = tokio::spawn(async move {
        target
            .publish(&session, key(), "first".into(), "yaml".into(), Some(md5))
            .await
    });
    gate.0.notified().await;
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        service.publish(&id, key(), "second".into(), "yaml".into(), Some(old.md5)),
    )
    .await;
    gate.1.notify_one();
    first.await.unwrap().unwrap();
    assert!(
        matches!(second, Ok(Err(_))),
        "an overlapping write must be refused before waiting on the mutation mutex"
    );
    server.abort();
}
#[tokio::test]
async fn a_stalled_write_times_out_once_and_returns_unknown_without_replay() {
    let (service, id, data, server) = fixture().await;
    let service = Arc::new(service);
    let old = service.config(&id, key()).await.unwrap().unwrap();
    let gate = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
    data.lock().unwrap().hold = Some(gate.clone());
    let target = service.clone();
    let session = id.clone();
    let task = tokio::spawn(async move {
        target
            .publish(
                &session,
                key(),
                "pending".into(),
                "yaml".into(),
                Some(old.md5),
            )
            .await
    });
    gate.0.notified().await;
    let error = task.await.unwrap().err().unwrap();
    assert_eq!(error, UNKNOWN);
    // The original accepted request is still the only one waiting at the fixture gate.
    assert_eq!(data.lock().unwrap().write_attempts, 1);
    assert_eq!(data.lock().unwrap().mutations.len(), 0);
    service.close(&id).unwrap();
    server.abort();
}

#[tokio::test]
async fn a_write_has_one_total_deadline_across_preflight_mutation_and_readback() {
    let (service, id, data, server) = fixture().await;
    let old = service.config(&id, key()).await.unwrap().unwrap();
    {
        let mut state = data.lock().unwrap();
        state.read_delay_ms = 8000;
        state.write_delay_ms = 8000;
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(15300),
        service.publish(&id, key(), "uncertain".into(), "yaml".into(), Some(old.md5)),
    )
    .await;
    assert!(
        matches!(&result,Ok(Err(message)) if message==UNKNOWN),
        "the whole operation must fail unknown within 15 seconds, not 15 seconds per request"
    );
    assert_eq!(data.lock().unwrap().write_attempts, 1);
    service.close(&id).unwrap();
    server.abort();
}

#[tokio::test]
async fn instance_metadata_removal_is_rejected_before_any_write_instead_of_silently_merging() {
    let (service, id, data, server) = fixture().await;
    let current = service.service(&id, service_key()).await.unwrap();
    let instance = current.instances[0].clone();
    let error = service
        .update_instance(
            &id,
            service_key(),
            instance.key,
            instance.snapshot,
            false,
            2.0,
            BTreeMap::new(),
        )
        .await
        .err()
        .expect("metadata removal must fail");
    assert!(error.contains("新增或修改"));
    assert!(data.lock().unwrap().mutations.is_empty());
    server.abort();
}
