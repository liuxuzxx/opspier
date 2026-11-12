use super::*;
use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command as ProcessCommand, Stdio},
};

struct Fixture {
    process: Child,
    output: BufReader<ChildStdout>,
    port: u16,
    fingerprint: String,
}
impl Fixture {
    fn new() -> Self {
        Self::with_trace(None)
    }
    fn with_trace(trace: Option<&std::path::Path>) -> Self {
        let mut command = ProcessCommand::new("python3");
        command.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/ssh_server.py"
        ));
        if let Ok(path) = std::env::var("AIC_OPS_FIXTURE_PYTHON_PATH") {
            command.env("PYTHONPATH", path);
        }
        if let Some(trace) = trace {
            command.env("AIC_OPS_FIXTURE_TRACE", trace);
        }
        let mut process = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = BufReader::new(process.stdout.take().unwrap());
        let mut fixture = Self {
            process,
            output,
            port: 0,
            fingerprint: String::new(),
        };
        fixture.read_info();
        fixture
    }
    fn read_info(&mut self) {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).expect(
            "start isolated fixture; install tests/fixtures/requirements.txt into fixture cache",
        );
        self.port = value["port"].as_u64().unwrap() as u16;
        self.fingerprint = value["fingerprint"].as_str().unwrap().into();
    }
    fn rotate(&mut self) {
        self.process
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"rotate\n")
            .unwrap();
        self.read_info();
    }
    fn endpoint(&self) -> Endpoint {
        Endpoint {
            host: "127.0.0.1".into(),
            port: self.port,
            username: "fixture-user".into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
fn password_request(token: String) -> ConnectRequest {
    ConnectRequest {
        token,
        trust_host: true,
        password: Some("fixture-only".into()),
        private_key: None,
        private_key_path: None,
        passphrase: None,
        cols: 120,
        rows: 36,
    }
}
fn until(service: &SshService, id: &str, text: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut bytes = vec![];
    while Instant::now() < deadline {
        let page = service.poll(id).unwrap();
        assert!(page.queued_bytes <= MAX_OUTPUT);
        bytes.extend(page.data);
        if String::from_utf8_lossy(&bytes).contains(text) {
            return bytes;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("isolated SSH output deadline exceeded");
}
#[test]
fn protocol_password_pty_unicode_resize_backpressure_and_exit() {
    let fixture = Fixture::new();
    let service = SshService::default();
    let probe = service.probe(fixture.endpoint()).unwrap();
    assert!(!probe.trusted);
    assert_eq!(probe.fingerprint, fixture.fingerprint);
    let id = service
        .connect(password_request(probe.token.clone()))
        .unwrap();
    assert!(service.connect(password_request(probe.token)).is_err());
    assert_eq!(
        String::from_utf8(until(&service, &id, "READY")).unwrap(),
        "READY 中文😀\r\n"
    );
    service
        .send(&id, Command::Input("回显😀\n".as_bytes().to_vec()))
        .unwrap();
    assert!(
        String::from_utf8(until(&service, &id, "回显😀"))
            .unwrap()
            .contains("回显😀")
    );
    service.send(&id, Command::Resize(91, 27)).unwrap();
    service
        .send(&id, Command::Input(b"size\n".to_vec()))
        .unwrap();
    until(&service, &id, "SIZE 91 27");
    service
        .send(&id, Command::Input(b"flood\n".to_vec()))
        .unwrap();
    std::thread::sleep(Duration::from_millis(350));
    let mut bytes = until(&service, &id, "FLOOD-END");
    if !String::from_utf8_lossy(&bytes).contains("STDERR-END") {
        bytes.extend(until(&service, &id, "STDERR-END"));
    }
    assert_eq!(bytes.iter().filter(|c| **c == b'x').count(), 1024 * 1024);
    service
        .send(&id, Command::Input(b"exit\n".to_vec()))
        .unwrap();
    for _ in 0..200 {
        if service.poll(&id).unwrap().status == "disconnected" {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(service.poll(&id).unwrap().status, "disconnected");
    service.disconnect(&id).unwrap();
    assert!(service.poll(&id).is_err());
}
#[test]
fn protocol_key_auth_and_host_changes_are_checked_before_authentication() {
    let mut fixture = Fixture::new();
    let service = SshService::default();
    let mut refused = password_request(service.probe(fixture.endpoint()).unwrap().token);
    refused.trust_host = false;
    assert!(service.connect(refused).is_err());
    let mut wrong = password_request(service.probe(fixture.endpoint()).unwrap().token);
    wrong.password = Some("wrong-fixture-password".into());
    assert!(service.connect(wrong).is_err());
    let probe = service.probe(fixture.endpoint()).unwrap();
    assert!(probe.trusted);
    let output = ProcessCommand::new("openssl")
        .args(["genrsa", "-traditional", "2048"])
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut request = password_request(probe.token);
    request.password = None;
    request.private_key = Some(String::from_utf8(output.stdout).unwrap());
    let id = service.connect(request).unwrap();
    until(&service, &id, "READY");
    service.disconnect(&id).unwrap();
    let pending = service.probe(fixture.endpoint()).unwrap();
    fixture.rotate();
    assert!(
        service
            .connect(password_request(pending.token))
            .err()
            .unwrap()
            .contains("指纹")
    );
    assert!(
        service
            .probe(fixture.endpoint())
            .err()
            .unwrap()
            .contains("指纹")
    );
}

#[test]
fn protocol_explicit_private_key_file_and_encrypted_rsa_passphrase() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let service = SshService::default();
    let directory = std::env::temp_dir().join(format!("aic-ops-test-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.join("fixture-rsa.pem");
    // Keys are generated for this fixture, never logged, and removed after use.
    for encrypted in [false, true] {
        let mut command = ProcessCommand::new("openssl");
        command.args(["genrsa", "-traditional"]);
        if encrypted {
            command.args(["-aes256", "-passout", "stdin"]);
        }
        let mut child = command
            .arg("2048")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"fixture-key-passphrase\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        std::fs::write(&path, output.stdout).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut request = password_request(service.probe(fixture.endpoint()).unwrap().token);
        request.password = None;
        request.private_key_path = Some(path.to_str().unwrap().into());
        request.passphrase = encrypted.then(|| "fixture-key-passphrase".into());
        let result = service.connect(request);
        std::fs::remove_file(&path).unwrap();
        let id = result.expect("explicit generated RSA path must authenticate");
        until(&service, &id, "READY");
        service.disconnect(&id).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn draft_test_authenticates_without_channel_shell_pin_or_registered_session() {
    let path = std::env::temp_dir().join(format!("ops-ssh-probe-{}", uuid::Uuid::new_v4()));
    let fixture = Fixture::with_trace(Some(&path));
    let service = SshService::default();
    let target = TestTarget {
        endpoint: fixture.endpoint(),
        authentication: "password".into(),
        private_key_path: None,
    };
    let addresses = vec![std::net::SocketAddr::from(([127, 0, 0, 1], fixture.port))];
    let probe = service
        .test_probe_resolved(
            target.clone(),
            &addresses,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    let request = TestAuthRequest {
        target: target.clone(),
        token: probe.token,
        trust_host: true,
        password: Some("fixture-only".into()),
        passphrase: None,
    };
    service
        .test_authenticate_resolved(request, &addresses, Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert!(service.sessions.lock().unwrap().is_empty());
    assert!(service.pins.lock().unwrap().is_empty());
    let deadline = Instant::now() + Duration::from_secs(2);
    let trace = loop {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.contains("authentication") {
            break text;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(trace.contains("true"));
    for event in ["channel", "pty", "shell"] {
        assert!(!trace.contains(event));
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn draft_test_challenge_refuses_changed_target_and_requires_fingerprint_confirmation() {
    let fixture = Fixture::new();
    let service = SshService::default();
    let target = TestTarget {
        endpoint: fixture.endpoint(),
        authentication: "password".into(),
        private_key_path: None,
    };
    let addresses = vec![std::net::SocketAddr::from(([127, 0, 0, 1], fixture.port))];
    for changed in [true, false] {
        let probe = service
            .test_probe_resolved(
                target.clone(),
                &addresses,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        let mut other = target.clone();
        if changed {
            other.endpoint.host = "localhost".into();
        }
        let request = TestAuthRequest {
            target: other,
            token: probe.token,
            trust_host: changed,
            password: Some("fixture-only".into()),
            passphrase: None,
        };
        assert!(
            service
                .test_authenticate_resolved(
                    request,
                    &addresses,
                    Instant::now() + Duration::from_secs(5)
                )
                .is_err()
        );
    }
    assert!(service.sessions.lock().unwrap().is_empty());
    assert!(service.pins.lock().unwrap().is_empty());
}

#[test]
fn draft_key_authentication_works_without_opening_an_interactive_session() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let service = SshService::default();
    let root = std::env::temp_dir().join(format!("ops-draft-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join("key.pem");
    let output = ProcessCommand::new("openssl")
        .args(["genrsa", "-traditional", "2048"])
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(&path, output.stdout).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let target = TestTarget {
        endpoint: fixture.endpoint(),
        authentication: "key".into(),
        private_key_path: Some(path.to_str().unwrap().into()),
    };
    let addresses = vec![std::net::SocketAddr::from(([127, 0, 0, 1], fixture.port))];
    let probe = service
        .test_probe_resolved(
            target.clone(),
            &addresses,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    service
        .test_authenticate_resolved(
            TestAuthRequest {
                target,
                token: probe.token,
                trust_host: true,
                password: None,
                passphrase: None,
            },
            &addresses,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    assert!(service.sessions.lock().unwrap().is_empty());
    assert!(service.pins.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn draft_test_rechecks_changed_fingerprint_before_authentication() {
    let mut fixture = Fixture::new();
    let service = SshService::default();
    let target = TestTarget {
        endpoint: fixture.endpoint(),
        authentication: "password".into(),
        private_key_path: None,
    };
    let addresses = vec![std::net::SocketAddr::from(([127, 0, 0, 1], fixture.port))];
    let probe = service
        .test_probe_resolved(
            target.clone(),
            &addresses,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    fixture.rotate();
    assert!(
        service
            .test_authenticate_resolved(
                TestAuthRequest {
                    target,
                    token: probe.token,
                    trust_host: true,
                    password: Some("fixture-only".into()),
                    passphrase: None
                },
                &addresses,
                Instant::now() + Duration::from_secs(5)
            )
            .err()
            .unwrap()
            .contains("指纹")
    );
    assert!(service.sessions.lock().unwrap().is_empty());
}
#[test]
fn draft_ssh_handshake_deadline_closes_a_stalled_transport() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = vec![];
        stream.read_to_end(&mut bytes).is_ok()
    });
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port: address.port(),
        username: "fixture".into(),
    };
    let started = Instant::now();
    assert!(
        handshake_resolved(
            &endpoint,
            &[address],
            Instant::now() + Duration::from_millis(100)
        )
        .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(server.join().unwrap());
}
