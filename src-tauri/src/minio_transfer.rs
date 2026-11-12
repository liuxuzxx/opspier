//! Bounded file transfers: one 8 MiB upload part, streaming downloads, atomic local commit.
use super::*;
use minio::s3::{
    multimap_ext::Multimap,
    response_traits::HasEtagFromHeaders,
    segmented_bytes::SegmentedBytes,
    types::{PartInfo, UploadId},
};
use std::path::{Path, PathBuf};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};
const CHUNK: usize = 8 * 1024 * 1024;
const FILE_ERROR: &str = "无法读取或写入本地文件，请检查路径、文件类型和权限";
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    pub id: String,
    pub bucket: String,
    pub key: String,
    pub direction: String,
    pub status: String,
    pub bytes: u64,
    pub total: u64,
    pub error: Option<String>,
}
pub(super) struct TransferJob {
    pub session_id: String,
    view: Mutex<TransferView>,
    cancel: watch::Sender<bool>,
}
impl TransferJob {
    pub fn running(&self) -> bool {
        self.view.lock().is_ok_and(|v| v.status == "running")
    }
    fn progress(&self, bytes: u64, total: u64) {
        if let Ok(mut view) = self.view.lock() {
            view.bytes = bytes;
            view.total = total;
        }
    }
    fn finish(&self, result: Result<(), String>) {
        if let Ok(mut view) = self.view.lock() {
            match result {
                Ok(()) => {
                    view.status = "completed".into();
                    view.bytes = view.total;
                }
                Err(e) => {
                    view.status = if e == "CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    view.error = Some(if e == "CANCELLED" {
                        "传输已取消；如已发送请求，请刷新确认远端对象状态".into()
                    } else {
                        e
                    });
                }
            }
        }
    }
}
impl MinioService {
    fn register(
        &self,
        id: &str,
        bucket: &str,
        key: &str,
        direction: &str,
        total: u64,
    ) -> Result<(Arc<TransferJob>, TransferView), String> {
        let mut state = self.state.lock().map_err(|_| "MinIO 状态不可用")?;
        if !state.sessions.contains_key(id) {
            return Err("MinIO 连接已关闭".into());
        }
        if state.jobs.values().filter(|j| j.running()).count() >= 4 {
            return Err("最多同时运行4个文件传输，请稍后重试".into());
        }
        if state.jobs.len() >= 64 {
            state.jobs.retain(|_, j| j.running());
        }
        let (cancel, _) = watch::channel(false);
        let view = TransferView {
            id: Uuid::new_v4().to_string(),
            bucket: bucket.into(),
            key: key.into(),
            direction: direction.into(),
            status: "running".into(),
            bytes: 0,
            total,
            error: None,
        };
        let job = Arc::new(TransferJob {
            session_id: id.into(),
            view: Mutex::new(view.clone()),
            cancel,
        });
        state.jobs.insert(view.id.clone(), job.clone());
        Ok((job, view))
    }
    pub fn transfer(&self, id: &str, transfer_id: &str) -> Result<TransferView, String> {
        let state = self.state.lock().map_err(|_| "MinIO 状态不可用")?;
        let job = state
            .jobs
            .get(transfer_id)
            .filter(|j| j.session_id == id)
            .ok_or("传输已过期或不属于当前连接")?;
        Ok(job.view.lock().map_err(|_| "传输状态不可用")?.clone())
    }
    pub fn cancel_transfer(&self, id: &str, transfer_id: &str) -> Result<(), String> {
        let state = self.state.lock().map_err(|_| "MinIO 状态不可用")?;
        let job = state
            .jobs
            .get(transfer_id)
            .filter(|j| j.session_id == id)
            .ok_or("传输已过期或不属于当前连接")?;
        job.cancel.send_replace(true);
        Ok(())
    }
    pub async fn upload(
        &self,
        id: &str,
        bucket: &str,
        prefix: &str,
        path: &str,
        overwrite: bool,
    ) -> Result<TransferView, String> {
        let runtime = self.get(id)?;
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(FILE_ERROR.into());
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .ok_or(FILE_ERROR)?;
        let key = format!("{prefix}{name}");
        if key.len() > 1024 || key.chars().any(char::is_control) {
            return Err("对象名称超过1024字节或含控制字符".into());
        }
        let file = tokio::fs::File::open(path).await.map_err(|_| FILE_ERROR)?;
        let metadata = file.metadata().await.map_err(|_| FILE_ERROR)?;
        if !metadata.is_file() {
            return Err("请选择普通文件，不支持上传目录".into());
        }
        // 8 MiB parts fit S3's 10,000-part limit for files through 78 GiB.
        if metadata.len() > CHUNK as u64 * 10000 {
            return Err("单文件超过本版78GiB上传上限".into());
        }
        if !overwrite {
            match bounded(
                runtime
                    .client
                    .stat_object(bucket, key.as_str())
                    .map_err(|_| INVALID)?
                    .region(Some(runtime.region()?))
                    .build()
                    .send(),
            )
            .await
            {
                Ok(_) => return Err("OBJECT_EXISTS".into()),
                Err(e) if e == MISSING => {}
                Err(e) => return Err(e),
            }
        }
        let (job, view) = self.register(id, bucket, &key, "upload", metadata.len())?;
        let bucket = bucket.to_owned();
        tokio::spawn(async move {
            let mut upload_id: Option<UploadId> = None;
            let mut cancel = job.cancel.subscribe();
            // Do not abandon CreateMultipartUpload on cancellation: first learn its ID,
            // then abort exactly that upload. A lost response is still an uncertain remote result.
            let created = async {
                if metadata.len() > CHUNK as u64 {
                    let response = bounded(
                        runtime
                            .client
                            .create_multipart_upload(bucket.as_str(), key.as_str())
                            .map_err(|_| INVALID)?
                            .region(Some(runtime.region()?))
                            .build()
                            .send(),
                    )
                    .await?;
                    upload_id = Some(
                        response
                            .upload_id()
                            .await
                            .map_err(|_| "MinIO 上传标识无效")?,
                    );
                }
                Ok::<(), String>(())
            }
            .await;
            let result = match created {
                Err(error) => Err(error),
                Ok(()) => {
                    if *cancel.borrow() {
                        Err("CANCELLED".into())
                    } else {
                        tokio::select! {biased;
                            _=cancel.changed()=>Err("CANCELLED".into()),
                            result=tokio::time::timeout(Duration::from_secs(3600),upload_file(&runtime,&bucket,&key,file,metadata.len(),overwrite,&job,&mut upload_id))=>result.unwrap_or_else(|_|Err("文件上传超时，请刷新确认对象状态".into()))
                        }
                    }
                }
            };
            // The upload id survives cancellation of the in-flight future; abort only our own multipart upload.
            if result.is_err() {
                if let Some(upload) = upload_id {
                    if let Ok(request) =
                        runtime
                            .client
                            .abort_multipart_upload(bucket.as_str(), key.as_str(), upload)
                    {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(15),
                            request.region(runtime.region().ok()).build().send(),
                        )
                        .await;
                    }
                }
            }
            job.finish(result);
        });
        Ok(view)
    }
    pub fn download(
        &self,
        id: &str,
        bucket: &str,
        key: &str,
        path: &str,
    ) -> Result<TransferView, String> {
        let runtime = self.get(id)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("请选择文件对象下载".into());
        }
        let target = PathBuf::from(path);
        if !target.is_absolute() || target.file_name().is_none() {
            return Err(FILE_ERROR.into());
        }
        let (job, view) = self.register(id, bucket, key, "download", 0)?;
        let bucket = bucket.to_owned();
        let key = key.to_owned();
        tokio::spawn(async move {
            let mut cancel = job.cancel.subscribe();
            let result = if *cancel.borrow() {
                Err("CANCELLED".into())
            } else {
                tokio::select! {biased;_=cancel.changed()=>Err("CANCELLED".into()),result=tokio::time::timeout(Duration::from_secs(3600),download_file(&runtime,&bucket,&key,&target,&job))=>result.unwrap_or_else(|_|Err("文件下载超时，原目标文件已保留".into()))}
            };
            job.finish(result);
        });
        Ok(view)
    }
}
fn condition_headers(overwrite: bool) -> Multimap {
    let mut headers = Multimap::new();
    if !overwrite {
        headers.insert("if-none-match".into(), "*".into());
    }
    headers
}
async fn upload_file(
    runtime: &Runtime,
    bucket: &str,
    key: &str,
    mut file: tokio::fs::File,
    size: u64,
    overwrite: bool,
    job: &TransferJob,
    upload_id: &mut Option<UploadId>,
) -> Result<(), String> {
    let region = runtime.region()?;
    let mut remaining = size;
    let mut uploaded = 0;
    let mut parts = Vec::new();
    loop {
        let count = remaining.min(CHUNK as u64) as usize;
        let mut chunk = vec![0; count];
        file.read_exact(&mut chunk)
            .await
            .map_err(|_| "上传文件在传输期间发生变化或无法读取")?;
        // Detect a changed file before committing the final part/object.
        if remaining == count as u64 {
            let mut extra = [0];
            if file.read(&mut extra).await.map_err(|_| FILE_ERROR)? != 0 {
                return Err("上传文件在传输期间发生变化，请重试".into());
            }
        }
        let data = bytes::Bytes::from(chunk);
        if let Some(upload) = upload_id.as_ref() {
            let number = parts.len() as u16 + 1;
            let response = runtime
                .client
                .upload_part(
                    bucket,
                    key,
                    upload.clone(),
                    number,
                    SegmentedBytes::from(data),
                )
                .map_err(|_| INVALID)?
                .region(Some(region.clone()))
                .build()
                .send()
                .await
                .map_err(sdk_error)?;
            parts.push(PartInfo::new(
                number,
                response.etag().map_err(|_| "MinIO 上传响应缺少 ETag")?,
                count as u64,
                None,
            ));
        } else {
            runtime
                .client
                .put_object_content(bucket, key, data)
                .map_err(|_| INVALID)?
                .region(Some(region.clone()))
                .extra_headers(Some(condition_headers(overwrite)))
                .build()
                .send()
                .await
                .map_err(sdk_error)?;
        }
        uploaded += count as u64;
        remaining -= count as u64;
        job.progress(uploaded, size);
        if remaining == 0 {
            break;
        }
    }
    if let Some(upload) = upload_id.as_ref() {
        runtime
            .client
            .complete_multipart_upload(bucket, key, upload.clone(), parts)
            .map_err(|_| INVALID)?
            .region(Some(region))
            .extra_headers(Some(condition_headers(overwrite)))
            .build()
            .send()
            .await
            .map_err(sdk_error)?;
    }
    Ok(())
}
async fn download_file(
    runtime: &Runtime,
    bucket: &str,
    key: &str,
    target: &Path,
    job: &TransferJob,
) -> Result<(), String> {
    let parent = target.parent().ok_or(FILE_ERROR)?;
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| FILE_ERROR)?;
    let mut file = tokio::fs::File::from_std(temporary.reopen().map_err(|_| FILE_ERROR)?);
    let response = runtime
        .client
        .get_object(bucket, key)
        .map_err(|_| INVALID)?
        .region(Some(runtime.region()?))
        .build()
        .send()
        .await
        .map_err(sdk_error)?;
    let total = response
        .object_size()
        .map_err(|_| "MinIO 文件大小响应无效")?;
    job.progress(0, total);
    let (mut stream, _) = response
        .content()
        .map_err(sdk_error)?
        .to_stream()
        .await
        .map_err(|_| FILE_ERROR)?;
    let mut received = 0;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|_| "MinIO 下载中断，原目标文件已保留")?;
        file.write_all(&bytes).await.map_err(|_| FILE_ERROR)?;
        received += bytes.len() as u64;
        job.progress(received, total);
    }
    if received != total {
        return Err("MinIO 下载长度不匹配，原目标文件已保留".into());
    }
    file.flush().await.map_err(|_| FILE_ERROR)?;
    file.sync_all().await.map_err(|_| FILE_ERROR)?;
    drop(file);
    temporary.persist(target).map_err(|_| FILE_ERROR)?;
    Ok(())
}
#[tauri::command]
pub async fn minio_upload(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    bucket: String,
    prefix: String,
    path: String,
    overwrite: bool,
) -> Result<TransferView, String> {
    service
        .upload(&session_id, &bucket, &prefix, &path, overwrite)
        .await
}
#[tauri::command]
pub async fn minio_download(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    bucket: String,
    key: String,
    path: String,
) -> Result<TransferView, String> {
    service.download(&session_id, &bucket, &key, &path)
}
#[tauri::command]
pub fn minio_transfer(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    transfer_id: String,
) -> Result<TransferView, String> {
    service.transfer(&session_id, &transfer_id)
}
#[tauri::command]
pub fn minio_cancel_transfer(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    transfer_id: String,
) -> Result<(), String> {
    service.cancel_transfer(&session_id, &transfer_id)
}
