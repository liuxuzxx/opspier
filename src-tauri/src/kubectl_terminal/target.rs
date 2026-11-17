use kube::config::Kubeconfig;
const BAD: &str = "终端仅接受当前已验证 Context 的自包含配置";
pub fn namespace(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 63
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
    {
        return Err("命名空间必须为合法名称；空表示 kubectl 默认 default".into());
    }
    Ok(())
}
pub fn narrow(
    source: &Kubeconfig,
    context: &str,
    selected_namespace: &str,
) -> Result<Kubeconfig, String> {
    crate::kubernetes::validate_config(source)?;
    namespace(selected_namespace)?;
    let contexts: Vec<_> = source
        .contexts
        .iter()
        .filter(|c| c.name == context)
        .cloned()
        .collect();
    if contexts.len() != 1 {
        return Err("请选择唯一、已验证的 Context".into());
    }
    let c = contexts[0].context.as_ref().ok_or(BAD)?;
    let clusters: Vec<_> = source
        .clusters
        .iter()
        .filter(|v| v.name == c.cluster)
        .cloned()
        .collect();
    let users: Vec<_> = if c.user.is_empty() {
        Vec::new()
    } else {
        source
            .auth_infos
            .iter()
            .filter(|v| v.name == c.user)
            .cloned()
            .collect()
    };
    if clusters.len() != 1 || (!c.user.is_empty() && users.len() != 1) {
        return Err(BAD.into());
    }
    let cluster = clusters[0].cluster.as_ref().ok_or(BAD)?;
    if cluster.certificate_authority.is_some() {
        return Err(BAD.into());
    }
    for user in &users {
        let user = user.auth_info.as_ref().ok_or(BAD)?;
        if user.client_certificate.is_some()
            || user.client_key.is_some()
            || user.token_file.is_some()
            || user.exec.is_some()
            || user.auth_provider.is_some()
        {
            return Err(BAD.into());
        }
    }
    let mut result = source.clone();
    result.contexts = contexts;
    result.clusters = clusters;
    result.auth_infos = users;
    result.current_context = Some(context.into());
    result.contexts[0].context.as_mut().ok_or(BAD)?.namespace = if selected_namespace.is_empty() {
        None
    } else {
        Some(selected_namespace.into())
    };
    // Preferences/extensions from unrelated sources are unnecessary for a terminal target.
    result.preferences = None;
    result.extensions = None;
    Ok(result)
}

use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
const ROOT_ERROR: &str = "无法创建或更新终端私有配置";
pub struct PrivateTarget {
    _lock: File,
    directory: tempfile::TempDir,
    config: Kubeconfig,
    path: PathBuf,
}
impl PrivateTarget {
    pub fn create(root: &Path, config: Kubeconfig) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|_| ROOT_ERROR)?;
        super::platform::secure(root, true)?;
        let directory = tempfile::Builder::new()
            .prefix("terminal-")
            .rand_bytes(32)
            .tempdir_in(root)
            .map_err(|_| ROOT_ERROR)?;
        super::platform::secure(directory.path(), true)?;
        let lock = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.path().join(".alive.lock"))
            .map_err(|_| ROOT_ERROR)?;
        super::platform::secure(&directory.path().join(".alive.lock"), false)?;
        FileExt::lock_shared(&lock).map_err(|_| ROOT_ERROR)?;
        let path = directory.path().join("kubeconfig");
        write_config(&config, &path)?;
        let rc = directory.path().join("init.bash");
        std::fs::write(&rc,b"unset HISTFILE\nHISTSIZE=2000\nHISTFILESIZE=0\nHISTCONTROL=ignoreboth\nPS1='kubectl> '\nif command -v kubectl >/dev/null 2>&1; then source <(kubectl completion bash 2>/dev/null); fi\n").map_err(|_|ROOT_ERROR)?;
        super::platform::secure(&rc, false)?;
        Ok(Self {
            directory,
            _lock: lock,
            config,
            path,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    pub fn set_namespace(&mut self, value: &str) -> Result<(), String> {
        namespace(value)?;
        let mut next = self.config.clone();
        next.contexts[0]
            .context
            .as_mut()
            .ok_or(ROOT_ERROR)?
            .namespace = if value.is_empty() {
            None
        } else {
            Some(value.into())
        };
        write_config(&next, &self.path)?;
        self.config = next;
        Ok(())
    }
}
fn write_config(config: &Kubeconfig, path: &Path) -> Result<(), String> {
    let data = serde_yaml::to_string(config).map_err(|_| ROOT_ERROR)?;
    if data.len() > 2 * 1024 * 1024 {
        return Err("终端私有配置超过2MiB上限".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or(ROOT_ERROR)?)
        .map_err(|_| ROOT_ERROR)?;
    super::platform::secure(file.path(), false)?;
    file.write_all(data.as_bytes()).map_err(|_| ROOT_ERROR)?;
    file.as_file().sync_all().map_err(|_| ROOT_ERROR)?;
    file.persist(path).map_err(|_| ROOT_ERROR)?;
    #[cfg(unix)]
    {
        File::open(path.parent().ok_or(ROOT_ERROR)?)
            .and_then(|file| file.sync_all())
            .map_err(|_| ROOT_ERROR)?;
    }
    Ok(())
}
pub fn cleanup_stale(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    super::platform::secure(root, true)?;
    for entry in std::fs::read_dir(root).map_err(|_| ROOT_ERROR)?.take(1024) {
        let entry = entry.map_err(|_| ROOT_ERROR)?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(suffix) = name.strip_prefix("terminal-") else {
            continue;
        };
        if suffix.len() != 32
            || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
            || !entry.file_type().map_err(|_| ROOT_ERROR)?.is_dir()
        {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if std::fs::symlink_metadata(&path)
                .map_err(|_| ROOT_ERROR)?
                .uid()
                != unsafe { libc::geteuid() }
            {
                continue;
            }
        }
        let lock_path = path.join(".alive.lock");
        if !std::fs::symlink_metadata(&lock_path)
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
        {
            continue;
        }
        let Ok(lock) = OpenOptions::new().read(true).write(true).open(lock_path) else {
            continue;
        };
        if FileExt::try_lock_exclusive(&lock).is_err() {
            continue;
        }
        #[cfg(windows)]
        drop(lock);
        std::fs::remove_dir_all(&path).map_err(|_| ROOT_ERROR)?;
    }
    Ok(())
}
