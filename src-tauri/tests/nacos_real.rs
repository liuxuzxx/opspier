#![cfg(all(feature = "external-fixtures", target_os = "linux"))]
use opspier::nacos::{
    NacosMaterial, NacosService, NacosSettings, config::ConfigKey, naming::ServiceKey,
};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Server {
    process: Child,
    output: BufReader<std::process::ChildStdout>,
    endpoint: String,
}
impl Server {
    fn start(password: &str) -> Self {
        let distribution = std::env::var("OPSPIER_NACOS_DISTRIBUTION")
            .expect("set the owned Nacos fixture distribution");
        let guard =
            std::env::var("OPSPIER_NACOS_BIND_GUARD").expect("set the verified owned bind adapter");
        let mut process = Command::new("python3")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/nacos_server.py"),
            )
            .args(["--distribution", &distribution, "--guard", &guard])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        serde_json::to_writer(
            process.stdin.as_mut().unwrap(),
            &serde_json::json!({"password":password}),
        )
        .unwrap();
        process.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        process.stdin.as_mut().unwrap().flush().unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&line).expect("owned real fixture did not become ready");
        Self {
            process,
            output,
            endpoint: data["endpoint"].as_str().unwrap().into(),
        }
    }
    fn stop(mut self) {
        self.process.stdin.take().unwrap().write_all(b"\n").unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let result: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(result["cleaned"], true);
        assert_eq!(result["exited"], true);
        assert!(self.process.wait().unwrap().success());
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if self.process.try_wait().ok().flatten().is_none() {
            let _ = self.process.stdin.take();
            let until = Instant::now() + Duration::from_secs(20);
            while Instant::now() < until {
                if self.process.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}
#[tokio::test]
async fn real_nacos_authenticated_configuration_history_and_naming_round_trip() {
    let password = uuid::Uuid::new_v4().to_string();
    let server = Server::start(&password);
    let service = NacosService::default();
    let session = service
        .connect(
            NacosSettings {
                endpoint: server.endpoint.clone(),
                authentication: "password".into(),
                namespace: "fixture-blue".into(),
            },
            Some(NacosMaterial {
                username: "nacos".into(),
                password,
            }),
        )
        .await
        .expect("actual authenticated product connection");
    let id = session.id;
    assert!(
        service
            .namespaces(&id)
            .await
            .unwrap()
            .iter()
            .any(|n| n.id == "fixture-blue")
    );
    let key = ConfigKey {
        namespace_id: "fixture-blue".into(),
        group: "BLUE".into(),
        data_id: "file.yaml".into(),
    };
    let first = service.config(&id, key.clone()).await.unwrap().unwrap();
    assert_eq!(first.content, "initial: value\r\n");
    assert!(
        service
            .publish(
                &id,
                key.clone(),
                "must-not-publish".into(),
                "yaml".into(),
                Some("stale".into())
            )
            .await
            .is_err()
    );
    let next = service
        .publish(
            &id,
            key.clone(),
            "unicode: 中文\r\n# preserved\n".into(),
            "yaml".into(),
            Some(first.md5),
        )
        .await
        .unwrap();
    let history = service.history(&id, key.clone(), 1, 20).await.unwrap();
    assert!(history.total >= 2);
    let mut old_id = None;
    for row in history.items {
        let detail = service
            .history_config(&id, key.clone(), row.id.clone())
            .await
            .unwrap();
        if detail.content == "initial: value\r\n" {
            old_id = Some(row.id);
        }
    }
    let rolled = service
        .rollback(
            &id,
            key.clone(),
            old_id.expect("initial historical revision"),
            next.md5,
        )
        .await
        .unwrap();
    assert_eq!(rolled.content, "initial: value\r\n");
    let list = service
        .configs(&id, "fixture-blue", "BLUE", "*file*", 1, 20)
        .await
        .unwrap();
    assert_eq!(list.total, 1);
    let sk = ServiceKey {
        namespace_id: "".into(),
        group: "BLUE".into(),
        name: "api".into(),
    };
    assert!(
        service
            .services(&id, "", "BLUE", "api", 1, 20)
            .await
            .unwrap()
            .total
            >= 1
    );
    let view = service.service(&id, sk.clone()).await.unwrap();
    assert_eq!(view.instances.len(), 1);
    let selector = view.selector.clone();
    let mut metadata = BTreeMap::new();
    metadata.insert("updated".into(), "fixture".into());
    let updated = service
        .update_service(&id, sk.clone(), view.snapshot, 0.3, metadata.clone())
        .await
        .unwrap();
    assert_eq!(updated.selector, selector);
    let instance = updated.instances[0].clone();
    let mut instance_metadata = instance.metadata.clone();
    instance_metadata.insert("updated".into(), "fixture".into());
    let changed = service
        .update_instance(
            &id,
            sk,
            instance.key.clone(),
            instance.snapshot,
            false,
            2.5,
            instance_metadata,
        )
        .await
        .unwrap();
    assert!(!changed.enabled);
    assert_eq!(changed.weight, 2.5);
    assert_eq!(changed.key, instance.key);
    service
        .delete_config(&id, key.clone(), rolled.md5)
        .await
        .unwrap();
    assert!(service.config(&id, key).await.unwrap().is_none());
    service.close(&id).unwrap();
    server.stop();
}
