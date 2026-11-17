use super::{
    process::{Poll, Terminal},
    target::{PrivateTarget, cleanup_stale},
};
use crate::kubernetes::KubeRuntimes;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
const STALE: &str = "终端请求已取消或工作区代次发生变化";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub session_id: String,
    pub generation: u64,
    pub revision: u64,
    pub terminal_id: String,
}
impl Scope {
    fn validate(&self) -> Result<(), String> {
        if uuid::Uuid::parse_str(&self.session_id).is_err()
            || uuid::Uuid::parse_str(&self.terminal_id).is_err()
            || self.revision == 0
            || self.revision > 9007199254740991
        {
            return Err("终端标识无效".into());
        }
        Ok(())
    }
}
struct Slot {
    generation: u64,
    revision: u64,
    id: String,
    open: bool,
    terminal: Option<Arc<Terminal>>,
    stop: Option<oneshot::Sender<()>>,
}
#[derive(Default)]
pub(super) struct Registry {
    slots: HashMap<String, Slot>,
}
impl Registry {
    pub fn reserve(
        &mut self,
        sid: &str,
        generation: u64,
        revision: u64,
        id: &str,
    ) -> Result<(), String> {
        if let Some(old) = self.slots.get(sid) {
            if generation < old.generation
                || (generation == old.generation && (revision <= old.revision || old.open))
            {
                return Err(STALE.into());
            }
            if old.open {
                return Err("旧终端尚未结束，请稍后重新打开".into());
            }
        }
        if self.slots.values().filter(|s| s.open).count() >= 8 {
            return Err("最多同时打开8个终端".into());
        }
        if !self.slots.contains_key(sid) && self.slots.len() >= 64 {
            return Err("终端工作区记录已满，请关闭旧工作区".into());
        }
        self.slots.insert(
            sid.into(),
            Slot {
                generation,
                revision,
                id: id.into(),
                open: true,
                terminal: None,
                stop: None,
            },
        );
        Ok(())
    }
    pub fn matches(
        &self,
        sid: &str,
        generation: u64,
        revision: u64,
        id: &str,
    ) -> Result<(), String> {
        let s = self.slots.get(sid).ok_or(STALE)?;
        if s.open && s.generation == generation && s.revision == revision && s.id == id {
            Ok(())
        } else {
            Err(STALE.into())
        }
    }
    pub fn close_slot(
        &mut self,
        sid: &str,
        generation: u64,
        revision: u64,
        id: &str,
    ) -> Result<Option<Arc<Terminal>>, String> {
        if let Some(s) = self.slots.get_mut(sid) {
            if s.generation != generation || s.revision != revision || s.id != id {
                return Err(STALE.into());
            }
            s.open = false;
            s.stop.take();
            return Ok(s.terminal.take());
        }
        if self.slots.len() >= 64 {
            return Err(STALE.into());
        }
        self.slots.insert(
            sid.into(),
            Slot {
                generation,
                revision,
                id: id.into(),
                open: false,
                terminal: None,
                stop: None,
            },
        );
        Ok(None)
    }
    fn scoped(&self, scope: &Scope) -> Result<Arc<Terminal>, String> {
        self.matches(
            &scope.session_id,
            scope.generation,
            scope.revision,
            &scope.terminal_id,
        )?;
        self.slots[&scope.session_id]
            .terminal
            .clone()
            .ok_or_else(|| "终端正在启动".into())
    }
}
struct Opening {
    registry: Arc<Mutex<Registry>>,
    scope: Scope,
    armed: bool,
}
impl Drop for Opening {
    fn drop(&mut self) {
        if self.armed {
            let terminal = self
                .registry
                .lock()
                .unwrap()
                .close_slot(
                    &self.scope.session_id,
                    self.scope.generation,
                    self.scope.revision,
                    &self.scope.terminal_id,
                )
                .ok()
                .flatten();
            if let Some(terminal) = terminal {
                std::thread::spawn(move || {
                    let _ = terminal.close();
                });
            }
        }
    }
}
pub struct TerminalService {
    root: PathBuf,
    registry: Arc<Mutex<Registry>>,
    #[cfg(test)]
    pub(super) startup_gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl TerminalService {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        cleanup_stale(&root)?;
        Ok(Self {
            root,
            registry: Arc::new(Mutex::new(Registry::default())),
            #[cfg(test)]
            startup_gate: Mutex::new(None),
        })
    }
    pub async fn open(
        &self,
        kube: &KubeRuntimes,
        scope: Scope,
        namespace: String,
        kubectl_path: Option<String>,
        shell_path: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<Scope, String> {
        scope.validate()?;
        let live = kube.terminal_sessions().await;
        self.registry
            .lock()
            .unwrap()
            .slots
            .retain(|id, s| s.open || live.contains(id));
        self.registry.lock().unwrap().reserve(
            &scope.session_id,
            scope.generation,
            scope.revision,
            &scope.terminal_id,
        )?;
        let mut opening = Opening {
            registry: self.registry.clone(),
            scope: scope.clone(),
            armed: true,
        };
        let (config, mut canceled) = kube
            .terminal_target(&scope.session_id, scope.generation, &namespace)
            .await?;
        let root = self.root.clone();
        let terminal = Arc::new(
            tokio::task::spawn_blocking(move || {
                let files = PrivateTarget::create(&root, config)?;
                let command = command(&files, kubectl_path.as_deref(), shell_path.as_deref())?;
                Terminal::spawn(files, command, cols, rows)
            })
            .await
            .map_err(|_| "本地终端启动任务失败")??,
        );
        #[cfg(test)]
        {
            let gate = self.startup_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.await;
            }
        }
        if *canceled.borrow()
            || kube
                .terminal_current(&scope.session_id, scope.generation)
                .await
                .is_err()
        {
            let t = terminal.clone();
            let _ = tokio::task::spawn_blocking(move || t.close()).await;
            return Err(STALE.into());
        }
        let (stop, stopped) = oneshot::channel();
        let attached = {
            let mut registry = self.registry.lock().unwrap();
            if registry
                .matches(
                    &scope.session_id,
                    scope.generation,
                    scope.revision,
                    &scope.terminal_id,
                )
                .is_ok()
            {
                let slot = registry.slots.get_mut(&scope.session_id).unwrap();
                slot.terminal = Some(terminal.clone());
                slot.stop = Some(stop);
                true
            } else {
                false
            }
        };
        if !attached {
            let t = terminal.clone();
            let _ = tokio::task::spawn_blocking(move || t.close()).await;
            return Err(STALE.into());
        }
        let weak = Arc::downgrade(&self.registry);
        let identity = scope.clone();
        tokio::spawn(async move {
            tokio::select! {_ = stopped=>{},_ = canceled.changed()=>{if let Some(registry)=weak.upgrade(){let process=registry.lock().unwrap().close_slot(&identity.session_id,identity.generation,identity.revision,&identity.terminal_id).ok().flatten();if let Some(process)=process{let _=tokio::task::spawn_blocking(move||process.close()).await;}}}}
        });
        opening.armed = false;
        Ok(scope)
    }
    async fn process(&self, kube: &KubeRuntimes, scope: &Scope) -> Result<Arc<Terminal>, String> {
        scope.validate()?;
        kube.terminal_current(&scope.session_id, scope.generation)
            .await?;
        self.registry.lock().unwrap().scoped(scope)
    }
    pub async fn input(
        &self,
        kube: &KubeRuntimes,
        scope: &Scope,
        data: &[u8],
    ) -> Result<(), String> {
        self.process(kube, scope).await?.input(data)
    }
    pub async fn poll(&self, kube: &KubeRuntimes, scope: &Scope) -> Result<Poll, String> {
        Ok(self.process(kube, scope).await?.poll())
    }
    pub async fn resize(
        &self,
        kube: &KubeRuntimes,
        scope: &Scope,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let process = self.process(kube, scope).await?;
        tokio::task::spawn_blocking(move || process.resize(cols, rows))
            .await
            .map_err(|_| "终端尺寸更新任务失败")?
    }
    pub async fn namespace(
        &self,
        kube: &KubeRuntimes,
        scope: &Scope,
        value: String,
    ) -> Result<(), String> {
        let process = self.process(kube, scope).await?;
        tokio::task::spawn_blocking(move || process.set_namespace(&value))
            .await
            .map_err(|_| "终端目标更新任务失败")?
    }
    pub async fn close(&self, kube: &KubeRuntimes, scope: Scope) -> Result<(), String> {
        scope.validate()?;
        let exists = self
            .registry
            .lock()
            .unwrap()
            .slots
            .contains_key(&scope.session_id);
        if !exists {
            kube.terminal_current(&scope.session_id, scope.generation)
                .await?;
        }
        let process = self.registry.lock().unwrap().close_slot(
            &scope.session_id,
            scope.generation,
            scope.revision,
            &scope.terminal_id,
        )?;
        if let Some(process) = process {
            tokio::task::spawn_blocking(move || process.close())
                .await
                .map_err(|_| "终端关闭任务失败")??;
        }
        Ok(())
    }
    pub fn close_all(&self) {
        let processes = {
            let mut registry = self.registry.lock().unwrap();
            registry
                .slots
                .values_mut()
                .filter_map(|slot| {
                    slot.open = false;
                    slot.stop.take();
                    slot.terminal.take()
                })
                .collect::<Vec<_>>()
        };
        for process in processes {
            let _ = process.close();
        }
    }
}
impl Drop for TerminalService {
    fn drop(&mut self) {
        self.close_all();
    }
}
fn executable(explicit: Option<&str>, name: &str) -> Result<PathBuf, String> {
    let candidates = if let Some(path) = explicit.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(path);
        if !p.is_absolute() {
            return Err(format!("{name} 路径必须为绝对路径"));
        }
        vec![p]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(name))
            .collect()
    };
    for p in candidates {
        if !p.is_absolute() || p.file_name() != Some(OsStr::new(name)) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&p) {
            if !meta.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            std::fs::canonicalize(&p).map_err(|_| format!("无法检查 {name} 路径"))?;
            // Preserve the selected entry: its basename may differ from a versioned symlink target.
            return Ok(p);
        }
    }
    Err(format!("未找到可执行的 {name}，请指定本机绝对路径"))
}
pub(super) fn command(
    files: &PrivateTarget,
    kubectl: Option<&str>,
    shell: Option<&str>,
) -> Result<CommandBuilder, String> {
    #[cfg(windows)]
    let kubectl = executable(kubectl, "kubectl.exe")?;
    #[cfg(not(windows))]
    let kubectl = executable(kubectl, "kubectl")?;
    #[cfg(target_os = "linux")]
    let shell = executable(shell.or(Some("/bin/bash")), "bash")?;
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c =
            CommandBuilder::new(std::env::current_exe().map_err(|_| "无法定位终端监督入口")?);
        #[cfg(not(test))]
        {
            c.arg("--opspier-terminal-supervisor");
            c.arg(&shell);
            c.arg(files.directory().join("init.bash"));
        }
        #[cfg(test)]
        {
            c.args([
                "--exact",
                "kubectl_terminal::tests::native::supervisor_fixture_entry",
                "--nocapture",
            ]);
            c.env("OPSPIER_TEST_SUPERVISOR", "1");
            c.env("OPSPIER_TEST_SUPERVISOR_SHELL", &shell);
            c.env(
                "OPSPIER_TEST_SUPERVISOR_RC",
                files.directory().join("init.bash"),
            );
        }
        c
    };
    #[cfg(windows)]
    let mut command = {
        let default = std::env::var_os("SystemRoot")
            .map(|p| PathBuf::from(p).join("System32/cmd.exe"))
            .ok_or("无法定位系统 cmd.exe")?;
        let shell = executable(shell.or(default.to_str()), "cmd.exe")?;
        let mut c = CommandBuilder::new(shell);
        c.args(["/D", "/Q"]);
        c
    };
    #[cfg(not(any(target_os = "linux", windows)))]
    return Err("当前平台尚未提供本地终端进程树监督".into());
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("BASH_FUNC_")
            || [
                "BASH_ENV",
                "ENV",
                "SHELLOPTS",
                "BASHOPTS",
                "BASH_XTRACEFD",
                "PROMPT_COMMAND",
                "PS0",
                "PS1",
                "PS2",
                "PS4",
                "HISTFILE",
                "KUBECONFIG",
                "KUBERNETES_MASTER",
            ]
            .contains(&name.as_ref())
        {
            continue;
        }
        command.env(key, value);
    }
    #[cfg(all(test, target_os = "linux"))]
    {
        command.env("OPSPIER_TEST_SUPERVISOR", "1");
        command.env("OPSPIER_TEST_SUPERVISOR_SHELL", &shell);
        command.env(
            "OPSPIER_TEST_SUPERVISOR_RC",
            files.directory().join("init.bash"),
        );
    }
    let parent = kubectl.parent().ok_or("kubectl 路径无效")?;
    let paths = std::iter::once(parent.to_path_buf())
        .chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ))
        .collect::<Vec<_>>();
    command.env(
        "PATH",
        std::env::join_paths(paths).map_err(|_| "kubectl 路径无效")?,
    );
    command.env("KUBECONFIG", files.path());
    command.env("TERM", "xterm-256color");
    command.cwd(files.directory());
    Ok(command)
}
#[tauri::command]
pub async fn terminal_open(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
    namespace: String,
    kubectl_path: Option<String>,
    shell_path: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<Scope, String> {
    service
        .open(
            &kube,
            scope,
            namespace,
            kubectl_path,
            shell_path,
            cols,
            rows,
        )
        .await
}
#[tauri::command]
pub async fn terminal_input(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
    data: Vec<u8>,
) -> Result<(), String> {
    service.input(&kube, &scope, &data).await
}
#[tauri::command]
pub async fn terminal_poll(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
) -> Result<Poll, String> {
    service.poll(&kube, &scope).await
}
#[tauri::command]
pub async fn terminal_resize(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    service.resize(&kube, &scope, cols, rows).await
}
#[tauri::command]
pub async fn terminal_namespace(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
    namespace: String,
) -> Result<(), String> {
    service.namespace(&kube, &scope, namespace).await
}
#[tauri::command]
pub async fn terminal_close(
    service: tauri::State<'_, TerminalService>,
    kube: tauri::State<'_, KubeRuntimes>,
    scope: Scope,
) -> Result<(), String> {
    service.close(&kube, scope).await
}
