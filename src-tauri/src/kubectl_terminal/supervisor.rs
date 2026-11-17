//! Runs only in this terminal's subprocess, never in the GUI process.
use fs2::FileExt;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::SeqCst);
}
extern "C" fn interrupt(_: i32) {}
fn parent(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
fn descendants(root: i32) -> Vec<i32> {
    let mut pairs = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            if let Some(pid) = e.file_name().to_str().and_then(|v| v.parse::<i32>().ok()) {
                if let Some(ppid) = parent(pid) {
                    pairs.insert(pid, ppid);
                }
            }
        }
    }
    let mut owned = HashSet::from([root]);
    loop {
        let before = owned.len();
        for (pid, ppid) in &pairs {
            if owned.contains(ppid) {
                owned.insert(*pid);
            }
        }
        if owned.len() == before {
            break;
        }
    }
    owned.remove(&root);
    owned.into_iter().collect()
}
fn is_descendant(mut pid: i32, root: i32) -> bool {
    for _ in 0..256 {
        let Some(ppid) = parent(pid) else {
            return false;
        };
        if ppid == root {
            return true;
        }
        if ppid <= 1 || ppid == pid {
            return false;
        }
        pid = ppid;
    }
    false
}
fn signal_owned(pid: i32, root: i32, signal: i32) {
    if pid <= 1 || pid == root {
        return;
    }
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return;
    }
    if is_descendant(pid, root) {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd,
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
    unsafe {
        libc::close(fd);
    }
}
fn reap() {
    loop {
        let result = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if result <= 0 {
            break;
        }
    }
}
fn cleanup(root: i32) -> bool {
    let start = Instant::now();
    loop {
        reap();
        let owned = descendants(root);
        if owned.is_empty() {
            reap();
            return true;
        }
        let signal = if start.elapsed() < Duration::from_millis(300) {
            libc::SIGTERM
        } else {
            libc::SIGKILL
        };
        for pid in owned {
            signal_owned(pid, root, signal);
        }
        if start.elapsed() > Duration::from_secs(4) {
            reap();
            return descendants(root).is_empty();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
pub fn run(shell: &Path, rc: &Path) -> i32 {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Some(directory) = rc.parent() else {
        return 2;
    };
    let Some(name) = directory
        .file_name()
        .and_then(|v| v.to_str())
        .and_then(|v| v.strip_prefix("terminal-"))
    else {
        return 2;
    };
    if name.len() != 32
        || !name.bytes().all(|v| v.is_ascii_alphanumeric())
        || rc.file_name().and_then(|v| v.to_str()) != Some("init.bash")
        || !shell.is_absolute()
        || shell.file_name().and_then(|v| v.to_str()) != Some("bash")
    {
        return 2;
    }
    for (path, mode) in [(directory, 0o700), (rc, 0o600)] {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return 2;
        };
        if meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.permissions().mode() & 0o777 != mode
        {
            return 2;
        }
    }
    let Ok(alive) = File::open(directory.join(".alive.lock")) else {
        return 2;
    };
    if FileExt::lock_shared(&alive).is_err() {
        return 2;
    }
    let parent_pid = unsafe { libc::getppid() };
    let pid = unsafe { libc::getpid() };
    if parent_pid <= 1 {
        return 2;
    }
    // Attribute changes and handlers are deliberately confined to this helper process.
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, interrupt as *const () as libc::sighandler_t);
        libc::signal(libc::SIGQUIT, interrupt as *const () as libc::sighandler_t);
        if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
            || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0
            || libc::getppid() != parent_pid
        {
            return 2;
        }
    }
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return 2;
    }
    unsafe {
        libc::close(fd);
    }
    let mut command = Command::new(shell);
    command
        .args([
            std::ffi::OsStr::new("--noprofile"),
            std::ffi::OsStr::new("--rcfile"),
            rc.as_os_str(),
            std::ffi::OsStr::new("-i"),
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let Ok(mut child) = command.spawn() else {
        return 2;
    };
    let code = loop {
        if STOP.load(Ordering::SeqCst) {
            break 0;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(1),
            Ok(None) => {}
            Err(_) => break 1,
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if cleanup(pid) { code } else { 1 }
}
