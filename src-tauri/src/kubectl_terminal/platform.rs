use std::path::Path;
#[cfg(unix)]
pub fn secure(path: &Path, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::symlink_metadata(path).map_err(|_| "无法检查终端私有文件")?;
    if meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
        return Err("终端目录或文件所有者无效".into());
    }
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
    .map_err(|_| "无法设置终端私有文件权限".into())
}
#[cfg(windows)]
pub fn secure(path: &Path, _directory: bool) -> Result<(), String> {
    windows::secure(path)
}
#[cfg(windows)]
pub use windows::Job;
#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::ffi::OsStrExt,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            DACL_SECURITY_INFORMATION, GetFileSecurityW, GetSecurityDescriptorControl,
            GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_SET_QUOTA,
                PROCESS_TERMINATE,
            },
        },
    };
    const BAD: &str = "无法建立或验证当前用户专用的 Windows 终端 ACL";
    unsafe fn text(value: *const u16) -> String {
        let mut n = 0;
        while unsafe { *value.add(n) } != 0 {
            n += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(value, n) })
    }
    pub fn secure(path: &Path) -> Result<(), String> {
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(BAD.into());
            }
            let mut needed = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
            let mut user = vec![0u8; needed as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                user.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                CloseHandle(token);
                return Err(BAD.into());
            }
            CloseHandle(token);
            let user = &*(user.as_ptr().cast::<TOKEN_USER>());
            let mut sid_text = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid_text) == 0 {
                return Err(BAD.into());
            }
            let sid = text(sid_text);
            LocalFree(sid_text.cast());
            let rule = format!("D:P(A;;FA;;;SY)(A;;FA;;;{sid})");
            let wide: Vec<u16> = rule.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err(BAD.into());
            }
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let ok = SetFileSecurityW(
                name.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            );
            LocalFree(descriptor.cast());
            if ok == 0 {
                return Err(BAD.into());
            }
            GetFileSecurityW(
                name.as_ptr(),
                DACL_SECURITY_INFORMATION,
                null_mut(),
                0,
                &mut needed,
            );
            let mut check = vec![0u8; needed as usize];
            if GetFileSecurityW(
                name.as_ptr(),
                DACL_SECURITY_INFORMATION,
                check.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                return Err(BAD.into());
            }
            let mut control = 0;
            let mut revision = 0;
            if GetSecurityDescriptorControl(check.as_mut_ptr().cast(), &mut control, &mut revision)
                == 0
                || control & 0x1000 == 0
            {
                return Err(BAD.into());
            }
            let mut result = null_mut();
            if ConvertSecurityDescriptorToStringSecurityDescriptorW(
                check.as_mut_ptr().cast(),
                1,
                DACL_SECURITY_INFORMATION,
                &mut result,
                null_mut(),
            ) == 0
            {
                return Err(BAD.into());
            }
            let actual = text(result);
            LocalFree(result.cast());
            if !actual.contains(&sid)
                || !actual.contains(";;;SY)")
                || actual.matches("(A;").count() != 2
            {
                return Err(BAD.into());
            }
            Ok(())
        }
    }
    pub struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
    impl Job {
        pub fn attach(pid: u32) -> Result<Self, String> {
            unsafe {
                let handle = CreateJobObjectW(null(), null());
                if handle.is_null() {
                    return Err("无法创建终端进程作业".into());
                }
                let job = Self(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                    std::mem::size_of_val(&info) as u32,
                ) == 0
                {
                    return Err("无法设置终端进程树退出策略".into());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err("无法绑定终端进程".into());
                }
                let ok = AssignProcessToJobObject(handle, process);
                CloseHandle(process);
                if ok == 0 {
                    return Err("无法绑定终端进程树".into());
                }
                Ok(job)
            }
        }
        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
