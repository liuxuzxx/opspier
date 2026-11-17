use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
};
struct QueueState {
    bytes: VecDeque<u8>,
    closed: bool,
}
pub(super) struct ByteQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    limit: usize,
}
impl ByteQueue {
    pub fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                bytes: VecDeque::new(),
                closed: false,
            }),
            changed: Condvar::new(),
            limit,
        }
    }
    pub fn try_push(&self, data: &[u8]) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.closed || s.bytes.len() + data.len() > self.limit {
            return false;
        }
        s.bytes.extend(data);
        self.changed.notify_all();
        true
    }
    pub fn push_blocking(&self, mut data: &[u8]) -> bool {
        let mut s = self.state.lock().unwrap();
        while !data.is_empty() {
            while !s.closed && s.bytes.len() == self.limit {
                s = self.changed.wait(s).unwrap();
            }
            if s.closed {
                return false;
            }
            let n = (self.limit - s.bytes.len()).min(data.len());
            s.bytes.extend(&data[..n]);
            data = &data[n..];
            self.changed.notify_all();
        }
        true
    }
    pub fn take(&self, max: usize) -> Vec<u8> {
        let mut s = self.state.lock().unwrap();
        let n = max.min(s.bytes.len());
        let result = s.bytes.drain(..n).collect();
        self.changed.notify_all();
        result
    }
    fn read(&self, max: usize) -> Option<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        while !s.closed && s.bytes.is_empty() {
            s = self.changed.wait(s).unwrap();
        }
        if s.closed {
            return None;
        }
        let n = max.min(s.bytes.len());
        let result = s.bytes.drain(..n).collect();
        self.changed.notify_all();
        Some(result)
    }
    pub fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        self.changed.notify_all();
    }
    fn len(&self) -> usize {
        self.state.lock().unwrap().bytes.len()
    }
}

use super::target::PrivateTarget;
#[cfg(not(target_os = "linux"))]
use portable_pty::ChildKiller;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
const IO_ERROR: &str = "终端已退出或本地 PTY 不可用";
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Poll {
    pub data: Vec<u8>,
    pub done: bool,
    pub queued_bytes: usize,
}
struct Inner {
    master: Mutex<Box<dyn MasterPty + Send>>,
    #[cfg(not(target_os = "linux"))]
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(target_os = "linux")]
    signal: std::fs::File,
    target: Mutex<Option<PrivateTarget>>,
    input: Arc<ByteQueue>,
    output: Arc<ByteQueue>,
    exited: Mutex<bool>,
    changed: Condvar,
    eof: AtomicBool,
    #[cfg(windows)]
    job: super::platform::Job,
}
pub struct Terminal {
    inner: Arc<Inner>,
}
fn dimensions(cols: u16, rows: u16) -> Result<PtySize, String> {
    if !(2..=500).contains(&cols) || !(2..=300).contains(&rows) {
        return Err("终端尺寸超出2–500列、2–300行范围".into());
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
impl Terminal {
    pub fn spawn(
        target: PrivateTarget,
        command: CommandBuilder,
        cols: u16,
        rows: u16,
    ) -> Result<Self, String> {
        let size = dimensions(cols, rows)?;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        // PDEATHSIG follows the creating thread. This launcher remains alive until its child exits.
        std::thread::Builder::new()
            .name("kubectl-launcher".into())
            .spawn(move || {
                match Self::start(target, command, size) {
                    Err(error) => {
                        let _ = send.send(Err(error));
                    }
                    Ok((inner, mut child)) => {
                        let unsent = send
                            .send(Ok(Self {
                                inner: inner.clone(),
                            }))
                            .err()
                            .and_then(|error| error.0.ok());
                        // Keep an undelivered terminal alive until wait/cleanup completes; Drop waits for it.
                        if let Some(terminal) = &unsent {
                            terminal.request_stop();
                        }
                        let _ = child.wait();
                        #[cfg(windows)]
                        inner.job.terminate();
                        inner.input.close();
                        inner.target.lock().unwrap().take();
                        *inner.exited.lock().unwrap() = true;
                        inner.changed.notify_all();
                        drop(unsent);
                    }
                }
            })
            .map_err(|_| "无法启动终端监督线程")?;
        receive.recv().map_err(|_| "终端监督线程未完成启动")?
    }
    fn start(
        target: PrivateTarget,
        command: CommandBuilder,
        size: PtySize,
    ) -> Result<(Arc<Inner>, Box<dyn Child + Send + Sync>), String> {
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|_| "无法创建本地 PTY")?;
        let mut reader = pair.master.try_clone_reader().map_err(|_| IO_ERROR)?;
        let mut writer = pair.master.take_writer().map_err(|_| IO_ERROR)?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|_| "无法启动已选择的本地 shell")?;
        drop(pair.slave);
        #[cfg(target_os = "linux")]
        let signal = {
            use std::os::fd::FromRawFd;
            let fd = child
                .process_id()
                .map(|pid| unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32)
                .unwrap_or(-1);
            if fd < 0 {
                let _ = child.kill();
                let _ = child.wait();
                return Err("无法绑定终端监督进程".into());
            }
            unsafe { std::fs::File::from_raw_fd(fd) }
        };
        #[cfg(windows)]
        let job = match child
            .process_id()
            .ok_or_else(|| "无法检查终端进程".to_string())
            .and_then(super::platform::Job::attach)
        {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let input = Arc::new(ByteQueue::new(128 * 1024));
        let output = Arc::new(ByteQueue::new(512 * 1024));
        let inner = Arc::new(Inner {
            master: Mutex::new(pair.master),
            #[cfg(not(target_os = "linux"))]
            killer: Mutex::new(child.clone_killer()),
            #[cfg(target_os = "linux")]
            signal,
            target: Mutex::new(Some(target)),
            input: input.clone(),
            output: output.clone(),
            exited: Mutex::new(false),
            changed: Condvar::new(),
            eof: AtomicBool::new(false),
            #[cfg(windows)]
            job,
        });
        let weak = Arc::downgrade(&inner);
        std::thread::spawn(move || {
            let mut bytes = [0u8; 8192];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if !output.push_blocking(&bytes[..n]) {
                            break;
                        }
                    }
                }
            }
            if let Some(inner) = weak.upgrade() {
                inner.eof.store(true, Ordering::Release);
            }
        });
        std::thread::spawn(move || {
            while let Some(bytes) = input.read(8192) {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    input.close();
                    break;
                }
            }
        });
        Ok((inner, child))
    }
    fn request_stop(&self) {
        self.inner.input.close();
        self.inner.output.close();
        #[cfg(windows)]
        self.inner.job.terminate();
        if !*self.inner.exited.lock().unwrap() {
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::AsRawFd;
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        self.inner.signal.as_raw_fd(),
                        libc::SIGHUP,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    );
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = self.inner.killer.lock().unwrap().kill();
            }
        }
    }
    pub fn input(&self, data: &[u8]) -> Result<(), String> {
        if data.len() > 65536 {
            return Err("单次输入超过64KiB上限".into());
        }
        if !self.inner.input.try_push(data) {
            return Err("终端已退出或输入队列已满，请稍后重试".into());
        }
        Ok(())
    }
    pub fn poll(&self) -> Poll {
        let data = self.inner.output.take(65536);
        let queued_bytes = self.inner.output.len();
        let done = *self.inner.exited.lock().unwrap()
            && self.inner.eof.load(Ordering::Acquire)
            && queued_bytes == 0;
        Poll {
            data,
            done,
            queued_bytes,
        }
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.inner
            .master
            .lock()
            .unwrap()
            .resize(dimensions(cols, rows)?)
            .map_err(|_| IO_ERROR.into())
    }
    pub fn set_namespace(&self, value: &str) -> Result<(), String> {
        self.inner
            .target
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(IO_ERROR)?
            .set_namespace(value)
    }
    pub fn close(&self) -> Result<(), String> {
        self.request_stop();
        let mut exited = self.inner.exited.lock().unwrap();
        if !*exited {
            exited = self
                .inner
                .changed
                .wait_timeout_while(exited, Duration::from_secs(6), |done| !*done)
                .unwrap()
                .0;
        }
        if *exited {
            Ok(())
        } else {
            Err("终端进程退出尚未确认".into())
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
