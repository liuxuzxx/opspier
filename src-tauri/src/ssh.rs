use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
#[cfg(test)]
#[path = "ssh_protocol_tests.rs"]
mod protocol_tests;
use serde::{Deserialize, Serialize};
use ssh2::{HashType, Session};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
const MAX_OUTPUT: usize = 512 * 1024;
const MAX_INPUT: usize = 64 * 1024;
const MAX_SESSIONS: usize = 8;
#[derive(Default)]
struct Output {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
}
impl Output {
    fn push(&mut self, data: Vec<u8>) -> bool {
        if data.len() > MAX_OUTPUT.saturating_sub(self.bytes) {
            return false;
        }
        self.bytes += data.len();
        self.chunks.push_back(data);
        true
    }
    fn take(&mut self, max: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(max.min(self.bytes));
        while result.len() < max {
            let Some(mut data) = self.chunks.pop_front() else {
                break;
            };
            let n = data.len().min(max - result.len());
            result.extend_from_slice(&data[..n]);
            self.bytes -= n;
            if n < data.len() {
                data.drain(..n);
                self.chunks.push_front(data);
                break;
            }
        }
        result
    }
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub(crate) token: String,
    pub(crate) trust_host: bool,
    pub(crate) password: Option<String>,
    pub(crate) private_key: Option<String>,
    pub(crate) private_key_path: Option<String>,
    pub(crate) passphrase: Option<String>,
    pub(crate) cols: u32,
    pub(crate) rows: u32,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProbe {
    pub(crate) token: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) fingerprint: String,
    pub(crate) trusted: bool,
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestTarget {
    pub endpoint: Endpoint,
    pub authentication: String,
    pub private_key_path: Option<String>,
}
impl TestTarget {
    fn validate(&self) -> Result<(), String> {
        validate_endpoint(
            &self.endpoint.host,
            self.endpoint.port,
            &self.endpoint.username,
        )?;
        match self.authentication.as_str() {
            "password" if self.private_key_path.is_none() => Ok(()),
            "key"
                if self.private_key_path.as_ref().is_some_and(|p| {
                    p.len() <= 4096
                        && !p.chars().any(char::is_control)
                        && std::path::Path::new(p).is_absolute()
                }) =>
            {
                Ok(())
            }
            _ => Err("测试认证方式或私钥路径无效".into()),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestAuthRequest {
    pub target: TestTarget,
    pub token: String,
    pub trust_host: bool,
    pub password: Option<String>,
    pub passphrase: Option<String>,
}
struct AuthMaterial<'a> {
    password: Option<&'a str>,
    private_key: Option<&'a str>,
    private_key_path: Option<&'a str>,
    passphrase: Option<&'a str>,
}
struct Challenge {
    test_target: Option<TestTarget>,
    endpoint: Endpoint,
    fingerprint: String,
    trusted: bool,
    created: Instant,
}
enum Command {
    Input(Vec<u8>),
    Resize(u32, u32),
}
struct Entry {
    tx: SyncSender<Command>,
    stop: Arc<AtomicBool>,
    output: Arc<Mutex<Output>>,
    status: Arc<Mutex<String>>,
    socket: TcpStream,
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
#[derive(Default)]
pub struct SshService {
    challenges: Mutex<HashMap<String, Challenge>>,
    sessions: Mutex<HashMap<String, Entry>>,
    pins: Mutex<HashMap<String, String>>,
    pins_path: Mutex<Option<PathBuf>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    data: Vec<u8>,
    status: String,
    queued_bytes: usize,
}
fn validate_endpoint(host: &str, port: u16, user: &str) -> Result<(), String> {
    if host.is_empty()
        || host.len() > 253
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/@?#\\".contains(c))
        || port == 0
        || user.trim().is_empty()
        || user.len() > 256
        || user.chars().any(char::is_control)
    {
        return Err("主机、端口或用户名无效".into());
    }
    if host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err() {
        return Err("主机请填写域名或IP，端口单独填写".into());
    }
    Ok(())
}
fn verify_fingerprint(expected: &str, actual: &str) -> Result<(), String> {
    if expected != actual {
        Err("SSH主机指纹发生变化，已拒绝认证。请通过可信渠道核验主机。".into())
    } else {
        Ok(())
    }
}
fn handshake(endpoint: &Endpoint) -> Result<(Session, TcpStream), String> {
    validate_endpoint(&endpoint.host, endpoint.port, &endpoint.username)?;
    let addresses: Vec<_> = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|_| "无法解析SSH主机")?
        .take(8)
        .collect();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut socket = None;
    for addr in addresses {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&addr, left) {
            socket = Some(stream);
            break;
        }
    }
    let socket = socket.ok_or("SSH连接失败或超时")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|_| "SSH连接初始化失败")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(8)))
        .map_err(|_| "SSH连接初始化失败")?;
    let mut session = Session::new().map_err(|_| "SSH初始化失败")?;
    session.set_timeout(8000);
    session.set_tcp_stream(socket.try_clone().map_err(|_| "SSH连接初始化失败")?);
    session.handshake().map_err(|_| "SSH握手失败或超时")?;
    Ok((session, socket))
}
fn fingerprint(session: &Session) -> Result<String, String> {
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(
            session
                .host_key_hash(HashType::Sha256)
                .ok_or("SSH主机未提供SHA256指纹")?
        )
    ))
}
fn pin_key(endpoint: &Endpoint) -> String {
    format!("[{}]:{}", endpoint.host, endpoint.port)
}
fn authenticate(
    session: &Session,
    endpoint: &Endpoint,
    request: AuthMaterial<'_>,
) -> Result<(), String> {
    if request.password.is_some_and(|p| p.len() > 64 * 1024)
        || request.passphrase.is_some_and(|p| p.len() > 64 * 1024)
    {
        return Err("密码或口令超过64KiB上限".into());
    }
    let user = &endpoint.username;
    if let Some(path) = request.private_key_path.as_ref().filter(|p| !p.is_empty()) {
        let path = std::path::Path::new(path);
        if !path.is_absolute() {
            return Err("私钥路径必须为绝对路径".into());
        }
        let metadata = std::fs::metadata(path).map_err(|_| "无法读取指定私钥")?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err("指定私钥必须是最多1MiB的普通文件".into());
        }
        let file = std::fs::File::open(path).map_err(|_| "无法读取指定私钥")?;
        if !file.metadata().map_err(|_| "无法读取指定私钥")?.is_file() {
            return Err("指定私钥必须是普通文件".into());
        }
        let mut key = String::new();
        file.take(1024 * 1024 + 1)
            .read_to_string(&mut key)
            .map_err(|_| "无法读取指定私钥")?;
        if key.len() > 1024 * 1024 || key.is_empty() {
            return Err("私钥大小无效（上限1MiB）".into());
        }
        session
            .userauth_pubkey_memory(user, None, &key, request.passphrase.as_deref())
            .map_err(|_| "SSH密钥认证失败")?;
    } else if let Some(key) = request.private_key.as_ref().filter(|p| !p.is_empty()) {
        if key.len() > 1024 * 1024 {
            return Err("私钥超过1MiB上限".into());
        }
        session
            .userauth_pubkey_memory(user, None, key, request.passphrase.as_deref())
            .map_err(|_| "SSH密钥认证失败")?;
    } else {
        session
            .userauth_password(
                user,
                request.password.as_deref().ok_or("请填写密码或指定私钥")?,
            )
            .map_err(|_| "SSH密码认证失败")?;
    }
    if !session.authenticated() {
        return Err("SSH认证失败".into());
    }
    Ok(())
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("SSH测试连接超时".into())
    } else {
        Ok(left)
    }
}
fn handshake_resolved(
    endpoint: &Endpoint,
    addresses: &[SocketAddr],
    deadline: Instant,
) -> Result<(Session, TcpStream), String> {
    validate_endpoint(&endpoint.host, endpoint.port, &endpoint.username)?;
    let mut socket = None;
    for address in addresses.iter().take(8) {
        if let Ok(stream) = TcpStream::connect_timeout(address, remaining(deadline)?) {
            socket = Some(stream);
            break;
        }
    }
    let socket = socket.ok_or("SSH测试连接失败或超时")?;
    socket
        .set_read_timeout(Some(remaining(deadline)?))
        .map_err(|_| "SSH测试初始化失败")?;
    socket
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|_| "SSH测试初始化失败")?;
    let mut session = Session::new().map_err(|_| "SSH测试初始化失败")?;
    session.set_timeout(
        remaining(deadline)?
            .as_millis()
            .max(1)
            .min(u32::MAX as u128) as u32,
    );
    session.set_tcp_stream(socket.try_clone().map_err(|_| "SSH测试初始化失败")?);
    session.handshake().map_err(|_| "SSH测试握手失败或超时")?;
    Ok((session, socket))
}
async fn resolve_test_target(target: &TestTarget) -> Result<Vec<SocketAddr>, String> {
    target.validate()?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::net::lookup_host((target.endpoint.host.as_str(), target.endpoint.port)),
    )
    .await
    .map_err(|_| "SSH测试DNS解析超时")?
    .map_err(|_| "无法解析SSH测试主机")?
    .take(8)
    .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err("SSH测试主机没有可用地址".into());
    }
    Ok(addresses)
}
impl SshService {
    pub fn initialize(&self, path: PathBuf) {
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.len() <= 512 * 1024 {
                if let Ok(pins) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
                    if let Ok(mut slot) = self.pins.lock() {
                        *slot = pins;
                    }
                }
            }
        }
        if let Ok(mut slot) = self.pins_path.lock() {
            *slot = Some(path);
        }
    }
    pub(crate) fn probe(&self, endpoint: Endpoint) -> Result<HostProbe, String> {
        let (session, _socket) = handshake(&endpoint)?;
        self.remember_probe(endpoint, &session, None)
    }
    fn remember_probe(
        &self,
        endpoint: Endpoint,
        session: &Session,
        test_target: Option<TestTarget>,
    ) -> Result<HostProbe, String> {
        let fingerprint = fingerprint(&session)?;
        let pins = self.pins.lock().map_err(|_| "SSH状态错误")?;
        let trusted = if let Some(expected) = pins.get(&pin_key(&endpoint)) {
            verify_fingerprint(expected, &fingerprint)?;
            true
        } else {
            false
        };
        drop(pins);
        let token = uuid::Uuid::new_v4().to_string();
        let result = HostProbe {
            token: token.clone(),
            host: endpoint.host.clone(),
            port: endpoint.port,
            fingerprint: fingerprint.clone(),
            trusted,
        };
        let mut challenges = self.challenges.lock().map_err(|_| "SSH状态错误")?;
        challenges.retain(|_, c| c.created.elapsed() < Duration::from_secs(120));
        if challenges.len() >= 16 {
            return Err("待确认SSH连接过多，请稍后再试".into());
        }
        challenges.insert(
            token,
            Challenge {
                test_target,
                endpoint,
                fingerprint,
                trusted,
                created: Instant::now(),
            },
        );
        Ok(result)
    }
    fn test_probe_resolved(
        &self,
        target: TestTarget,
        addresses: &[SocketAddr],
        deadline: Instant,
    ) -> Result<HostProbe, String> {
        target.validate()?;
        let (session, socket) = handshake_resolved(&target.endpoint, addresses, deadline)?;
        let result = self.remember_probe(target.endpoint.clone(), &session, Some(target));
        let _ = socket.shutdown(Shutdown::Both);
        result
    }
    fn test_authenticate_resolved(
        &self,
        request: TestAuthRequest,
        addresses: &[SocketAddr],
        deadline: Instant,
    ) -> Result<(), String> {
        request.target.validate()?;
        let challenge = self
            .challenges
            .lock()
            .map_err(|_| "SSH状态错误")?
            .remove(&request.token)
            .ok_or("请重新获取并核验测试主机指纹")?;
        if challenge.test_target.as_ref() != Some(&request.target)
            || challenge.created.elapsed() > Duration::from_secs(120)
        {
            return Err("测试目标已变化或指纹确认过期，请重新核验".into());
        }
        if !challenge.trusted && !request.trust_host {
            return Err("尚未确认SSH测试主机指纹".into());
        }
        let (session, socket) = handshake_resolved(&challenge.endpoint, addresses, deadline)?;
        let result = (|| {
            verify_fingerprint(&challenge.fingerprint, &fingerprint(&session)?)?;
            session.set_timeout(
                remaining(deadline)?
                    .as_millis()
                    .max(1)
                    .min(u32::MAX as u128) as u32,
            );
            let key = request.target.authentication == "key";
            authenticate(
                &session,
                &challenge.endpoint,
                AuthMaterial {
                    password: if key {
                        None
                    } else {
                        request.password.as_deref()
                    },
                    private_key: None,
                    private_key_path: request.target.private_key_path.as_deref(),
                    passphrase: if key {
                        request.passphrase.as_deref()
                    } else {
                        None
                    },
                },
            )
        })();
        let _ = socket.shutdown(Shutdown::Both);
        result
    }
    fn save_pin(&self, endpoint: &Endpoint, fingerprint: String) -> Result<(), String> {
        let mut pins = self.pins.lock().map_err(|_| "SSH状态错误")?;
        if let Some(expected) = pins.get(&pin_key(endpoint)) {
            return verify_fingerprint(expected, &fingerprint);
        }
        pins.insert(pin_key(endpoint), fingerprint);
        if let Some(path) = self.pins_path.lock().map_err(|_| "SSH状态错误")?.as_ref() {
            let parent = path.parent().ok_or("指纹存储路径无效")?;
            std::fs::create_dir_all(parent).map_err(|_| "无法创建指纹存储目录")?;
            let temp = parent.join(format!(".host-pins-{}", uuid::Uuid::new_v4()));
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|_| "无法写入指纹存储")?;
            let bytes = serde_json::to_vec(&*pins).map_err(|_| "指纹存储失败")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "指纹存储失败")?;
            std::fs::rename(&temp, path).map_err(|_| "指纹存储失败")?;
        }
        Ok(())
    }
    pub(crate) fn connect(&self, request: ConnectRequest) -> Result<String, String> {
        if !(2..=600).contains(&request.cols) || !(1..=300).contains(&request.rows) {
            return Err("终端尺寸无效".into());
        }
        if self.sessions.lock().map_err(|_| "SSH状态错误")?.len() >= MAX_SESSIONS {
            return Err("最多允许8个SSH会话".into());
        }
        let challenge = self
            .challenges
            .lock()
            .map_err(|_| "SSH状态错误")?
            .remove(&request.token)
            .ok_or("请重新获取并核验SSH主机指纹")?;
        if challenge.test_target.is_some() {
            return Err("测试指纹确认不能用于创建工作区".into());
        }
        if challenge.created.elapsed() > Duration::from_secs(120) {
            return Err("主机指纹确认已超时，请重试".into());
        }
        if !challenge.trusted && !request.trust_host {
            return Err("尚未确认SSH主机指纹".into());
        }
        let (session, socket) = handshake(&challenge.endpoint)?;
        verify_fingerprint(&challenge.fingerprint, &fingerprint(&session)?)?;
        // Record a public host pin before authentication; secrets never reach persistence.
        self.save_pin(&challenge.endpoint, challenge.fingerprint)?;
        authenticate(
            &session,
            &challenge.endpoint,
            AuthMaterial {
                password: request.password.as_deref(),
                private_key: request.private_key.as_deref(),
                private_key_path: request.private_key_path.as_deref(),
                passphrase: request.passphrase.as_deref(),
            },
        )?;
        let mut channel = session.channel_session().map_err(|_| "无法创建SSH终端")?;
        channel
            .request_pty(
                "xterm-256color",
                None,
                Some((request.cols, request.rows, 0, 0)),
            )
            .map_err(|_| "服务器拒绝PTY")?;
        channel.shell().map_err(|_| "服务器拒绝Shell")?;
        socket
            .set_nonblocking(true)
            .map_err(|_| "SSH终端初始化失败")?;
        session.set_blocking(false);
        let (tx, rx) = mpsc::sync_channel::<Command>(32);
        let stop = Arc::new(AtomicBool::new(false));
        let output = Arc::new(Mutex::new(Output::default()));
        let status = Arc::new(Mutex::new("connected".to_string()));
        let entry = Entry {
            tx,
            stop: stop.clone(),
            output: output.clone(),
            status: status.clone(),
            socket: socket.try_clone().map_err(|_| "SSH连接初始化失败")?,
        };
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut sessions = self.sessions.lock().map_err(|_| "SSH状态错误")?;
            if sessions.len() >= MAX_SESSIONS {
                return Err("最多允许8个SSH会话".into());
            }
            sessions.insert(id.clone(), entry);
        }
        std::thread::spawn(move || {
            let _owned_session = session;
            let mut pending = Vec::<u8>::new();
            let mut offset = 0;
            let mut resize = None;
            let mut stream_index = 0;
            let mut buffer = [0u8; 16 * 1024];
            let result = (|| -> Result<(), String> {
                loop {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if pending.is_empty() {
                        match rx.try_recv() {
                            Ok(Command::Input(data)) => {
                                pending = data;
                                offset = 0
                            }
                            Ok(Command::Resize(c, r)) => resize = Some((c, r)),
                            Err(mpsc::TryRecvError::Disconnected) => break,
                            Err(mpsc::TryRecvError::Empty) => {}
                        }
                    }
                    if let Some((cols, rows)) = resize {
                        match channel.request_pty_size(cols, rows, None, None) {
                            Ok(()) => resize = None,
                            Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                            Err(_) => return Err("终端尺寸更新失败".into()),
                        }
                    }
                    if !pending.is_empty() {
                        match channel.write(&pending[offset..]) {
                            Ok(0) => return Err("SSH连接已关闭".into()),
                            Ok(n) => {
                                offset += n;
                                if offset == pending.len() {
                                    pending.clear();
                                    offset = 0;
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => return Err("SSH输入失败".into()),
                        }
                    }
                    let available =
                        MAX_OUTPUT.saturating_sub(output.lock().map_err(|_| "SSH状态错误")?.bytes);
                    let mut read_any = false;
                    if available > 0 {
                        let limit = buffer.len().min(available);
                        for _ in 0..2 {
                            let stream = stream_index;
                            stream_index = 1 - stream_index;
                            match channel.stream(stream).read(&mut buffer[..limit]) {
                                Ok(n) if n > 0 => {
                                    output
                                        .lock()
                                        .map_err(|_| "SSH状态错误")?
                                        .push(buffer[..n].to_vec());
                                    read_any = true;
                                    break;
                                }
                                Ok(_) => {}
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => return Err("SSH输出连接中断".into()),
                            }
                        }
                    }
                    if channel.eof() && available > 0 && !read_any {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(4));
                }
                Ok(())
            })();
            let _ = socket.shutdown(Shutdown::Both);
            if let Ok(mut value) = status.lock() {
                *value = match result {
                    Ok(()) => "disconnected".into(),
                    Err(e) => e,
                };
            }
        });
        Ok(id)
    }
    fn poll(&self, id: &str) -> Result<PollResult, String> {
        let sessions = self.sessions.lock().map_err(|_| "SSH状态错误")?;
        let entry = sessions.get(id).ok_or("SSH会话不存在")?;
        let mut output = entry.output.lock().map_err(|_| "SSH状态错误")?;
        let data = output.take(64 * 1024);
        Ok(PollResult {
            data,
            status: entry.status.lock().map_err(|_| "SSH状态错误")?.clone(),
            queued_bytes: output.bytes,
        })
    }
    fn send(&self, id: &str, command: Command) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|_| "SSH状态错误")?;
        let entry = sessions.get(id).ok_or("SSH会话不存在")?;
        if entry.stop.load(Ordering::Acquire) {
            return Err("SSH会话已断开".into());
        }
        entry
            .tx
            .try_send(command)
            .map_err(|_| "SSH输入队列已满，请稍后再试".into())
    }
    pub(crate) fn disconnect(&self, id: &str) -> Result<(), String> {
        self.sessions.lock().map_err(|_| "SSH状态错误")?.remove(id);
        Ok(())
    }
}
#[tauri::command]
pub async fn ssh_test_probe(
    target: TestTarget,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<HostProbe, String> {
    let addresses = resolve_test_target(&target).await?;
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        state.test_probe_resolved(target, &addresses, Instant::now() + Duration::from_secs(15))
    })
    .await
    .map_err(|_| "SSH测试探测任务已停止")?
}
#[tauri::command]
pub async fn ssh_test_authenticate(
    request: TestAuthRequest,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<(), String> {
    let addresses = resolve_test_target(&request.target).await?;
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        state.test_authenticate_resolved(
            request,
            &addresses,
            Instant::now() + Duration::from_secs(15),
        )
    })
    .await
    .map_err(|_| "SSH测试认证任务已停止")?
}
#[tauri::command]
pub async fn ssh_probe(
    endpoint: Endpoint,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<HostProbe, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || state.probe(endpoint))
        .await
        .map_err(|_| "SSH探测任务失败")?
}
#[tauri::command]
pub async fn ssh_connect(
    request: ConnectRequest,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<String, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || state.connect(request))
        .await
        .map_err(|_| "SSH连接任务失败")?
}
#[tauri::command]
pub fn ssh_poll(
    id: String,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<PollResult, String> {
    state.poll(&id)
}
#[tauri::command]
pub fn ssh_input(
    id: String,
    data: Vec<u8>,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<(), String> {
    if data.len() > MAX_INPUT {
        return Err("单次输入超过64KiB上限".into());
    }
    state.send(&id, Command::Input(data))
}
#[tauri::command]
pub fn ssh_resize(
    id: String,
    cols: u32,
    rows: u32,
    state: tauri::State<'_, Arc<SshService>>,
) -> Result<(), String> {
    if !(2..=600).contains(&cols) || !(1..=300).contains(&rows) {
        return Err("终端尺寸无效".into());
    }
    state.send(&id, Command::Resize(cols, rows))
}
#[tauri::command]
pub fn ssh_disconnect(id: String, state: tauri::State<'_, Arc<SshService>>) -> Result<(), String> {
    state.disconnect(&id)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_is_bounded_and_retains_byte_order() {
        let mut output = Output::default();
        assert!(output.push(vec![7; MAX_OUTPUT]));
        assert!(!output.push(vec![8]));
        assert_eq!(output.take(3), vec![7; 3]);
        assert!(output.push(vec![9]));
        assert_eq!(output.bytes, MAX_OUTPUT - 2);
    }
    #[test]
    fn invalid_endpoint_and_empty_user_rejected() {
        for host in ["", "user@host", "http://host", "host\nsecret"] {
            assert!(validate_endpoint(host, 22, "user").is_err());
        }
        assert!(validate_endpoint("localhost", 0, "user").is_err());
        assert!(validate_endpoint("localhost", 22, "").is_err());
        assert!(validate_endpoint("::1", 22, "user").is_ok());
    }
    #[test]
    fn host_key_change_is_never_accepted() {
        assert!(verify_fingerprint("SHA256:old", "SHA256:new").is_err());
        assert!(verify_fingerprint("SHA256:same", "SHA256:same").is_ok());
    }
}
