use super::target;
use kube::config::Kubeconfig;
fn source() -> Kubeconfig {
    Kubeconfig::from_yaml(
        r#"
apiVersion: v1
kind: Config
current-context: other
clusters:
- name: selected-cluster
  cluster: {server: 'http://127.0.0.1:12345'}
- name: other-cluster
  cluster: {server: 'http://127.0.0.1:12346'}
users:
- name: selected-user
  user: {token: fixture-selected-token}
- name: other-user
  user: {token: fixture-other-token}
contexts:
- name: selected
  context: {cluster: selected-cluster, user: selected-user, namespace: original}
- name: other
  context: {cluster: other-cluster, user: other-user, namespace: forbidden}
"#,
    )
    .unwrap()
}
#[test]
fn terminal_target_keeps_only_the_explicit_context_cluster_and_user_without_mutating_source() {
    let original = source();
    let result = target::narrow(&original, "selected", "chosen").unwrap();
    assert_eq!(result.contexts.len(), 1);
    assert_eq!(result.contexts[0].name, "selected");
    assert_eq!(result.clusters.len(), 1);
    assert_eq!(result.clusters[0].name, "selected-cluster");
    assert_eq!(result.auth_infos.len(), 1);
    assert_eq!(result.auth_infos[0].name, "selected-user");
    assert_eq!(result.current_context.as_deref(), Some("selected"));
    assert_eq!(
        result.contexts[0]
            .context
            .as_ref()
            .unwrap()
            .namespace
            .as_deref(),
        Some("chosen")
    );
    assert_eq!(original.current_context.as_deref(), Some("other"));
    assert_eq!(original.contexts.len(), 2);
}
#[test]
fn empty_namespace_removes_source_namespace_instead_of_injecting_all_or_original() {
    let result = target::narrow(&source(), "selected", "").unwrap();
    assert!(
        result.contexts[0]
            .context
            .as_ref()
            .unwrap()
            .namespace
            .is_none()
    );
    for value in [
        "--all-namespaces",
        "../other",
        "invalid namespace",
        "x\n",
        &"x".repeat(64),
    ] {
        assert!(target::narrow(&source(), "selected", value).is_err());
    }
}
#[test]
fn absent_or_ambiguous_context_and_live_file_references_are_rejected() {
    let mut original = source();
    assert!(target::narrow(&original, "absent", "").is_err());
    original.contexts.push(original.contexts[0].clone());
    assert!(target::narrow(&original, "selected", "").is_err());
    let mut original = source();
    original.auth_infos[0]
        .auth_info
        .as_mut()
        .unwrap()
        .token_file = Some("../user-secret".into());
    assert!(target::narrow(&original, "selected", "").is_err());
    let mut original = source();
    original.clusters[0]
        .cluster
        .as_mut()
        .unwrap()
        .certificate_authority = Some("../ca.crt".into());
    assert!(target::narrow(&original, "selected", "").is_err());
}
#[test]
fn terminal_private_files_are_owner_only_namespace_updates_are_atomic_and_drop_cleans_them() {
    let root = tempfile::tempdir().unwrap();
    let target_config = target::narrow(&source(), "selected", "first").unwrap();
    let mut files = target::PrivateTarget::create(root.path(), target_config).unwrap();
    let directory = files.directory().to_path_buf();
    let file = files.path().to_path_buf();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    files.set_namespace("second").unwrap();
    let decoded = Kubeconfig::read_from(&file).unwrap();
    assert_eq!(
        decoded.contexts[0]
            .context
            .as_ref()
            .unwrap()
            .namespace
            .as_deref(),
        Some("second")
    );
    files.set_namespace("").unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(files.set_namespace("not a namespace").is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert!(
        Kubeconfig::read_from(&file).unwrap().contexts[0]
            .context
            .as_ref()
            .unwrap()
            .namespace
            .is_none()
    );
    drop(files);
    assert!(!directory.exists());
}
#[test]
fn stale_cleanup_removes_only_recognized_unlocked_owned_terminal_directories() {
    let root = tempfile::tempdir().unwrap();
    let live = target::PrivateTarget::create(
        root.path(),
        target::narrow(&source(), "selected", "").unwrap(),
    )
    .unwrap();
    let live_path = live.directory().to_owned();
    let stale = root.path().join(format!("terminal-{}", "a".repeat(32)));
    std::fs::create_dir(&stale).unwrap();
    std::fs::write(stale.join(".alive.lock"), b"").unwrap();
    std::fs::write(stale.join("kubeconfig"), b"discard").unwrap();
    let unrelated = root.path().join("unrelated-user-directory");
    std::fs::create_dir(&unrelated).unwrap();
    target::cleanup_stale(root.path()).unwrap();
    assert!(!stale.exists());
    assert!(live_path.exists());
    assert!(unrelated.exists());
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::{
        io::{Read, Write},
        path::Path,
        sync::mpsc,
        time::{Duration, Instant},
    };
    // This test process is a fixture entry point: only explicitly spawned PTY children execute the host.
    #[test]
    fn supervisor_fixture_entry() {
        if std::env::var_os("OPSPIER_TEST_SUPERVISOR").as_deref() == Some(std::ffi::OsStr::new("1"))
        {
            let shell = std::env::var_os("OPSPIER_TEST_SUPERVISOR_SHELL").unwrap();
            let rc = std::env::var_os("OPSPIER_TEST_SUPERVISOR_RC").unwrap();
            std::process::exit(crate::kubectl_terminal::supervisor::run(
                Path::new(&shell),
                Path::new(&rc),
            ));
        }
    }
    struct Host {
        master: Box<dyn portable_pty::MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        output: mpsc::Receiver<Vec<u8>>,
    }
    impl Host {
        fn open(files: &target::PrivateTarget) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 31,
                    cols: 90,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "kubectl_terminal::tests::native::supervisor_fixture_entry",
                "--nocapture",
            ]);
            command.env("OPSPIER_TEST_SUPERVISOR", "1");
            command.env("OPSPIER_TEST_SUPERVISOR_SHELL", "/bin/bash");
            command.env(
                "OPSPIER_TEST_SUPERVISOR_RC",
                files.directory().join("init.bash"),
            );
            command.env("KUBECONFIG", files.path());
            let child = pair.slave.spawn_command(command).unwrap();
            drop(pair.slave);
            let writer = pair.master.take_writer().unwrap();
            let mut reader = pair.master.try_clone_reader().unwrap();
            let (send, output) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if send.send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            Self {
                master: pair.master,
                writer,
                child,
                output,
            }
        }
        fn until(&self, needle: &str) -> String {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut bytes = Vec::new();
            while Instant::now() < deadline {
                if let Ok(chunk) = self.output.recv_timeout(Duration::from_millis(100)) {
                    bytes.extend(chunk);
                    if String::from_utf8_lossy(&bytes).contains(needle) {
                        return String::from_utf8_lossy(&bytes).into_owned();
                    }
                }
            }
            panic!("real PTY did not produce the expected fixture marker");
        }
        fn send(&mut self, value: &[u8]) {
            self.writer.write_all(value).unwrap();
            self.writer.flush().unwrap();
        }
        fn stop(&mut self) {
            self.child.kill().unwrap();
            let deadline = Instant::now() + Duration::from_secs(6);
            while Instant::now() < deadline {
                if self.child.try_wait().unwrap().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("owned supervisor did not exit");
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }
    #[test]
    fn managed_process_bounds_input_drains_large_output_and_removes_private_target_on_exit() {
        use super::super::process::Terminal;
        let root = tempfile::tempdir().unwrap();
        let files = target::PrivateTarget::create(
            root.path(),
            target::narrow(&source(), "selected", "").unwrap(),
        )
        .unwrap();
        let directory = files.directory().to_owned();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "kubectl_terminal::tests::native::supervisor_fixture_entry",
            "--nocapture",
        ]);
        command.env("OPSPIER_TEST_SUPERVISOR", "1");
        command.env("OPSPIER_TEST_SUPERVISOR_SHELL", "/bin/bash");
        command.env(
            "OPSPIER_TEST_SUPERVISOR_RC",
            files.directory().join("init.bash"),
        );
        command.env("KUBECONFIG", files.path());
        let terminal = Terminal::spawn(files, command, 90, 31).unwrap();
        let until = |marker: &str| {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut output = Vec::new();
            while Instant::now() < deadline {
                let result = terminal.poll();
                output.extend(result.data);
                if String::from_utf8_lossy(&output).contains(marker) {
                    return output;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("managed PTY fixture marker absent");
        };
        until("kubectl> ");
        assert!(terminal.input(&vec![b'x'; 65537]).is_err());
        terminal
            .input(b"stty -echo; printf 'ready-%s\\n' marker\r")
            .unwrap();
        until("ready-marker");
        terminal.input(b"python3 -c \"import sys;sys.stdout.write('X'*900000);sys.stdout.flush()\"; printf '\\nend-%s\\n' marker\r").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let output = until("end-marker");
        assert_eq!(output.iter().filter(|&&b| b == b'X').count(), 900000);
        terminal.resize(110, 40).unwrap();
        terminal.input(b"stty size\r").unwrap();
        until("40 110");
        terminal.set_namespace("next").unwrap();
        terminal.input(b"exit\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        while directory.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!directory.exists());
        terminal.close().unwrap();
    }
    #[test]
    fn selected_symlinks_to_versioned_kubectl_and_bash_remain_the_executed_entries() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let entries = root.path().join("entries");
        let versions = root.path().join("versions");
        std::fs::create_dir(&entries).unwrap();
        std::fs::create_dir(&versions).unwrap();
        let kubectl = versions.join("kubectl-fixture-version");
        std::fs::write(&kubectl,b"#!/bin/sh\nif [ \"$1\" = completion ]; then exit 0; fi\nprintf 'chosen-%s\\n' fixture\n").unwrap();
        std::fs::set_permissions(&kubectl, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&kubectl, entries.join("kubectl")).unwrap();
        let bash = versions.join("bash-fixture-version");
        std::fs::copy("/bin/bash", &bash).unwrap();
        std::fs::set_permissions(&bash, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&bash, entries.join("bash")).unwrap();
        let files = target::PrivateTarget::create(
            root.path(),
            target::narrow(&source(), "selected", "").unwrap(),
        )
        .unwrap();
        let command = super::super::runtime::command(
            &files,
            entries.join("kubectl").to_str(),
            entries.join("bash").to_str(),
        )
        .unwrap();
        let terminal = super::super::process::Terminal::spawn(files, command, 90, 31).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while Instant::now() < deadline {
            output.extend(terminal.poll().data);
            if String::from_utf8_lossy(&output).contains("kubectl> ") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        terminal.input(b"kubectl\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            output.extend(terminal.poll().data);
            if String::from_utf8_lossy(&output).contains("chosen-fixture") {
                terminal.close().unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("selected kubectl entry did not run");
    }
    #[test]
    fn real_supervised_pty_delivers_unicode_stderr_resize_and_ctrl_c_to_the_foreground_job() {
        let root = tempfile::tempdir().unwrap();
        let files = target::PrivateTarget::create(
            root.path(),
            target::narrow(&source(), "selected", "").unwrap(),
        )
        .unwrap();
        let mut host = Host::open(&files);
        host.until("kubectl> ");
        host.send(b"printf '\\u4e2d\\u6587\\n'; printf 'err-%s\\n' result >&2; stty size\r");
        let output = host.until("31 90");
        assert!(output.contains("中文") && output.contains("err-result"));
        host.master
            .resize(PtySize {
                rows: 41,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        host.send(b"stty size\r");
        host.until("41 120");
        host.send(b"printf 'run-%s\\n' marker; sleep 30\r");
        host.until("run-marker");
        host.send(&[3]);
        host.until("kubectl> ");
        host.stop();
    }
    #[test]
    fn supervisor_exit_reaps_background_and_double_forked_owned_descendants() {
        let root = tempfile::tempdir().unwrap();
        let files = target::PrivateTarget::create(
            root.path(),
            target::narrow(&source(), "selected", "").unwrap(),
        )
        .unwrap();
        let code = files.directory().join("fork.py");
        let pid_file = files.directory().join("detached.pid");
        std::fs::write(&code,format!("import os,time\nif os.fork():os._exit(0)\nos.setsid()\nif os.fork():os._exit(0)\nopen({:?},'w').write(str(os.getpid()))\nwhile True:time.sleep(1)\n",pid_file.to_str().unwrap())).unwrap();
        let mut before = 0;
        unsafe {
            libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut before, 0, 0, 0);
        }
        let mut host = Host::open(&files);
        host.until("kubectl> ");
        host.send(format!("python3 '{}'\r", code.display()).as_bytes());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert!(pid > 0);
        host.stop();
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "owned detached descendant must be reaped"
        );
        let mut after = 0;
        unsafe {
            libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut after, 0, 0, 0);
        }
        assert_eq!(
            before, after,
            "GUI/test parent process attributes must not change"
        );
    }
}

mod buffers {
    use super::super::process::ByteQueue;
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };
    #[test]
    fn output_backpressure_is_bounded_and_polling_resumes_the_producer_without_losing_bytes() {
        let queue = Arc::new(ByteQueue::new(8));
        let producer = queue.clone();
        let (send, received) = mpsc::channel();
        let task = std::thread::spawn(move || {
            assert!(producer.push_blocking(b"abcdefghijkl"));
            send.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(queue.take(4), b"abcd");
        received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(queue.take(64), b"efghijkl");
        task.join().unwrap();
    }
    #[test]
    fn input_rejects_overflow_atomically_and_close_wakes_backpressured_output() {
        let queue = Arc::new(ByteQueue::new(8));
        assert!(queue.try_push(b"123456"));
        assert!(!queue.try_push(b"789"));
        assert_eq!(queue.take(64), b"123456");
        assert!(queue.try_push(b"12345678"));
        let producer = queue.clone();
        let task = std::thread::spawn(move || producer.push_blocking(b"x"));
        queue.close();
        assert!(!task.join().unwrap());
        assert!(!queue.try_push(b"z"));
    }
}

mod reservations {
    use super::super::runtime::Registry;
    #[test]
    fn close_before_open_or_during_startup_prevents_late_attach_and_old_close_cannot_touch_new_revision()
     {
        let mut slots = Registry::default();
        slots.close_slot("s", 2, 1, "first").unwrap();
        assert!(slots.reserve("s", 2, 1, "first").is_err());
        slots.reserve("s", 2, 2, "second").unwrap();
        assert!(slots.reserve("s", 2, 3, "third").is_err());
        assert!(slots.close_slot("s", 2, 1, "first").is_err());
        slots.close_slot("s", 2, 2, "second").unwrap();
        assert!(slots.matches("s", 2, 2, "second").is_err());
        assert!(slots.reserve("s", 1, 99, "old-generation").is_err());
        slots.reserve("s", 3, 1, "new-generation").unwrap();
    }
    #[test]
    fn reservations_count_towards_the_limit_and_never_cross_sessions() {
        let mut slots = Registry::default();
        for index in 0..8 {
            slots.reserve(&format!("s{index}"), 1, 1, "t").unwrap();
        }
        assert!(slots.reserve("ninth", 1, 1, "t").is_err());
        assert!(slots.matches("s0", 1, 1, "wrong-id").is_err());
        assert!(slots.matches("s0", 2, 1, "t").is_err());
        slots.close_slot("s0", 1, 1, "t").unwrap();
        slots.reserve("ninth", 1, 1, "t").unwrap();
        assert!(slots.matches("s1", 1, 1, "t").is_ok());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn runtime_rejects_cross_terminal_input_and_workspace_close_reaps_the_real_process() {
    use super::runtime::{Scope, TerminalService};
    use crate::kubernetes::KubeRuntimes;
    use std::time::{Duration, Instant};
    let (_executables, kubectl) = kubectl_fixture();
    let root = tempfile::tempdir().unwrap();
    let terminals = TerminalService::new(root.path().to_owned()).unwrap();
    let kube = KubeRuntimes::default();
    let opened = kube
        .open_config(source(), Some("selected"), Some("first"))
        .await
        .unwrap();
    let scope = Scope {
        session_id: opened.session_id.clone(),
        generation: opened.data.generation,
        revision: 1,
        terminal_id: uuid::Uuid::new_v4().to_string(),
    };
    terminals
        .open(
            &kube,
            scope.clone(),
            String::new(),
            Some(kubectl.clone()),
            None,
            90,
            31,
        )
        .await
        .unwrap();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        output.extend(terminals.poll(&kube, &scope).await.unwrap().data);
        if String::from_utf8_lossy(&output).contains("kubectl> ") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        String::from_utf8_lossy(&output).contains("kubectl> "),
        "runtime must start a real supervised shell"
    );
    let mut wrong = scope.clone();
    wrong.terminal_id = uuid::Uuid::new_v4().to_string();
    assert!(terminals.input(&kube, &wrong, b"bad\r").await.is_err());
    assert!(
        terminals
            .open(&kube, scope.clone(), String::new(), None, None, 90, 31)
            .await
            .is_err()
    );
    terminals.input(&kube, &scope, b"sleep 30\r").await.unwrap();
    kube.close(&opened.session_id, scope.generation)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while std::fs::read_dir(root.path()).unwrap().next().is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        std::fs::read_dir(root.path()).unwrap().next().is_none(),
        "closed workspace must reap PTY and release private files"
    );
    assert!(terminals.input(&kube, &scope, b"bad\r").await.is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn close_while_a_real_startup_result_is_delayed_cleans_only_that_process_and_allows_a_new_revision()
 {
    use super::runtime::{Scope, TerminalService};
    use crate::kubernetes::KubeRuntimes;
    use std::{sync::Arc, time::Duration};
    let (_executables, kubectl) = kubectl_fixture();
    let root = tempfile::tempdir().unwrap();
    let terminals = Arc::new(TerminalService::new(root.path().to_owned()).unwrap());
    let kube = Arc::new(KubeRuntimes::default());
    let opened = kube
        .open_config(source(), Some("selected"), Some("first"))
        .await
        .unwrap();
    let scope = Scope {
        session_id: opened.session_id,
        generation: opened.data.generation,
        revision: 1,
        terminal_id: uuid::Uuid::new_v4().to_string(),
    };
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    *terminals.startup_gate.lock().unwrap() = Some((started, wait));
    let task = {
        let service = terminals.clone();
        let kube = kube.clone();
        let scope = scope.clone();
        let kubectl = kubectl.clone();
        tokio::spawn(async move {
            service
                .open(
                    &kube,
                    scope,
                    String::new(),
                    Some(kubectl.clone()),
                    None,
                    90,
                    31,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    terminals.close(&kube, scope.clone()).await.unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    let next = Scope {
        revision: 2,
        terminal_id: uuid::Uuid::new_v4().to_string(),
        ..scope.clone()
    };
    terminals
        .open(
            &kube,
            next.clone(),
            String::new(),
            Some(kubectl.clone()),
            None,
            90,
            31,
        )
        .await
        .unwrap();
    assert!(terminals.close(&kube, scope).await.is_err());
    assert!(
        terminals
            .input(&kube, &next, b"printf 'next'\r")
            .await
            .is_ok()
    );
    terminals.close(&kube, next).await.unwrap();
}

#[cfg(target_os = "linux")]
fn kubectl_fixture() -> (tempfile::TempDir, String) {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("kubectl");
    std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let name = path.to_string_lossy().into_owned();
    (directory, name)
}
#[cfg(target_os = "linux")]
#[test]
fn active_terminal_survives_the_creating_tokio_blocking_worker_retirement() {
    use super::runtime::{Scope, TerminalService};
    use crate::kubernetes::KubeRuntimes;
    use std::time::{Duration, Instant};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_keep_alive(Duration::from_millis(20))
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_executables, kubectl) = kubectl_fixture();
        let root = tempfile::tempdir().unwrap();
        let service = TerminalService::new(root.path().to_owned()).unwrap();
        let kube = KubeRuntimes::default();
        let selection = kube
            .open_config(source(), Some("selected"), Some("first"))
            .await
            .unwrap();
        let scope = Scope {
            session_id: selection.session_id,
            generation: selection.data.generation,
            revision: 1,
            terminal_id: uuid::Uuid::new_v4().to_string(),
        };
        service
            .open(
                &kube,
                scope.clone(),
                String::new(),
                Some(kubectl),
                None,
                90,
                31,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            !service.poll(&kube, &scope).await.unwrap().done,
            "retiring the blocking creator thread must not kill the active terminal"
        );
        service
            .input(&kube, &scope, b"printf 'alive-%s\\n' marker\r")
            .await
            .unwrap();
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            output.extend(service.poll(&kube, &scope).await.unwrap().data);
            if String::from_utf8_lossy(&output).contains("alive-marker") {
                service.close(&kube, scope).await.unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("shell did not remain interactive after worker retirement");
    });
}
