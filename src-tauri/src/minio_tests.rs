//! Read/write API tests use an owned loopback S3 endpoint only.
use super::*;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    response::Response,
    routing::any,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct Fixture {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_get: AtomicBool,
    slow_create: AtomicBool,
    create_seen: AtomicBool,
    abort_seen: AtomicBool,
}
async fn serve(State(state): State<Arc<Fixture>>, request: Request) -> Response {
    let method = request.method().as_str().to_owned();
    let uri = request.uri().clone();
    if !request.headers().get("authorization").is_some_and(|v| {
        v.as_bytes()
            .starts_with(b"AWS4-HMAC-SHA256 Credential=test-access/")
    }) {
        return xml(
            403,
            "<Error><Code>AccessDenied</Code><Message>not reflected</Message></Error>",
        );
    }
    let path = url::Url::parse(&format!("http://localhost{}", uri)).unwrap();
    let key = path.path().trim_start_matches("/test-bucket/").to_owned();
    if method == "GET" && uri.path() == "/" {
        return xml(
            200,
            "<ListAllMyBucketsResult><Buckets><Bucket><Name>test-bucket</Name><CreationDate>2025-01-01T00:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>",
        );
    }
    if method == "GET" && uri.query().is_some_and(|s| s.contains("list-type=2")) {
        return xml(
            200,
            "<ListBucketResult><Name>test-bucket</Name><Prefix></Prefix><MaxKeys>200</MaxKeys><IsTruncated>true</IsTruncated><NextContinuationToken>next-page</NextContinuationToken><Contents><Key>readme.txt</Key><LastModified>2025-01-01T00:00:00.000Z</LastModified><ETag>abc</ETag><Size>5</Size></Contents><CommonPrefixes><Prefix>photos/</Prefix></CommonPrefixes></ListBucketResult>",
        );
    }
    if method == "POST" && uri.query().is_some_and(|s| s.contains("uploads")) {
        state.create_seen.store(true, Ordering::SeqCst);
        if state.slow_create.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        return xml(
            200,
            "<InitiateMultipartUploadResult><Bucket>test-bucket</Bucket><Key>big.bin</Key><UploadId>fixture-upload</UploadId></InitiateMultipartUploadResult>",
        );
    }
    if method == "DELETE" && uri.query().is_some_and(|s| s.contains("uploadId")) {
        state.abort_seen.store(true, Ordering::SeqCst);
        return Response::builder().status(204).body(Body::empty()).unwrap();
    }
    if method == "PUT" {
        let body = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        state.objects.lock().unwrap().insert(key, body.to_vec());
        return Response::builder()
            .status(200)
            .header("etag", "\"abc\"")
            .body(Body::empty())
            .unwrap();
    }
    if method == "DELETE" {
        state.objects.lock().unwrap().remove(&key);
        return Response::builder().status(204).body(Body::empty()).unwrap();
    }
    let content = state.objects.lock().unwrap().get(&key).cloned();
    if let Some(data) = content {
        if method == "HEAD" {
            return Response::builder()
                .status(200)
                .header("content-length", data.len())
                .header("etag", "\"abc\"")
                .body(Body::empty())
                .unwrap();
        }
        if state.fail_get.load(Ordering::SeqCst) {
            return xml(
                500,
                "<Error><Code>InternalError</Code><Message>not reflected</Message></Error>",
            );
        }
        return Response::builder()
            .status(200)
            .header("content-length", data.len())
            .body(Body::from(Bytes::from(data)))
            .unwrap();
    }
    xml(
        404,
        "<Error><Code>NoSuchKey</Code><Message>not reflected</Message></Error>",
    )
}
fn xml(status: u16, text: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .body(Body::from(text.to_owned()))
        .unwrap()
}
async fn fixture() -> (
    MinioService,
    MinioSession,
    Arc<Fixture>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Fixture::default());
    let app = Router::new().fallback(any(serve)).with_state(state.clone());
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let service = MinioService::default();
    let settings = MinioSettings {
        endpoint: format!("http://127.0.0.1:{port}"),
        region: "us-east-1".into(),
        bucket: String::new(),
    };
    let session = service
        .connect(
            settings,
            MinioMaterial {
                access_key: "test-access".into(),
                secret_key: "test-secret".into(),
            },
        )
        .await
        .unwrap();
    (service, session, state, handle)
}
async fn wait(service: &MinioService, session: &str, id: &str) -> TransferView {
    for _ in 0..200 {
        let view = service.transfer(session, id).unwrap();
        if view.status != "running" {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("bounded loopback transfer timed out")
}
#[tokio::test]
async fn lists_signed_buckets_and_one_object_page_with_directory_and_cursor() {
    let (service, session, _, server) = fixture().await;
    assert_eq!(service.buckets(&session.id).await.unwrap(), ["test-bucket"]);
    let page = service
        .objects(&session.id, "test-bucket", "", None)
        .await
        .unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("next-page"));
    assert!(
        page.objects
            .iter()
            .any(|o| o.key == "photos/" && o.directory)
    );
    assert!(
        page.objects
            .iter()
            .any(|o| o.key == "readme.txt" && o.size == 5 && !o.directory)
    );
    service.close(&session.id).unwrap();
    assert!(service.buckets(&session.id).await.is_err());
    server.abort();
}
#[tokio::test]
async fn upload_download_delete_use_real_sdk_bytes_and_preserve_existing_local_file_on_failure() {
    let (service, session, state, server) = fixture().await;
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("report.txt");
    let target = folder.path().join("download.txt");
    tokio::fs::write(&source, b"actual bytes\0with unicode: \xe4\xb8\xad")
        .await
        .unwrap();
    let up = service
        .upload(
            &session.id,
            "test-bucket",
            "",
            source.to_str().unwrap(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&service, &session.id, &up.id).await.status,
        "completed"
    );
    assert_eq!(
        state.objects.lock().unwrap().get("report.txt").unwrap(),
        b"actual bytes\0with unicode: \xe4\xb8\xad"
    );
    assert!(
        service
            .upload(
                &session.id,
                "test-bucket",
                "",
                source.to_str().unwrap(),
                false
            )
            .await
            .is_err()
    );
    let down = service
        .download(
            &session.id,
            "test-bucket",
            "report.txt",
            target.to_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        wait(&service, &session.id, &down.id).await.status,
        "completed"
    );
    assert_eq!(
        tokio::fs::read(&source).await.unwrap(),
        tokio::fs::read(&target).await.unwrap()
    );
    tokio::fs::write(&target, b"original").await.unwrap();
    state.fail_get.store(true, Ordering::SeqCst);
    let failed = service
        .download(
            &session.id,
            "test-bucket",
            "report.txt",
            target.to_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        wait(&service, &session.id, &failed.id).await.status,
        "failed"
    );
    assert_eq!(tokio::fs::read(&target).await.unwrap(), b"original");
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    service
        .delete(&session.id, "test-bucket", "report.txt")
        .await
        .unwrap();
    assert!(!state.objects.lock().unwrap().contains_key("report.txt"));
    server.abort();
}
#[test]
fn unsafe_endpoints_and_control_paths_never_become_network_clients() {
    for endpoint in [
        "https://user:pass@host.test",
        "https://host.test/path",
        "https://host.test?token=bad",
        "ftp://host.test",
        "https://host.test/#frag",
    ] {
        assert!(
            MinioSettings {
                endpoint: endpoint.into(),
                region: "us-east-1".into(),
                bucket: "".into()
            }
            .validate()
            .is_err()
        );
    }
}

#[tokio::test]
async fn cancelling_during_multipart_creation_waits_for_id_and_aborts_our_upload() {
    let (service, session, state, server) = fixture().await;
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("big.bin");
    let file = std::fs::File::create(&source).unwrap();
    file.set_len(9 * 1024 * 1024).unwrap();
    state.slow_create.store(true, Ordering::SeqCst);
    let job = service
        .upload(
            &session.id,
            "test-bucket",
            "",
            source.to_str().unwrap(),
            false,
        )
        .await
        .unwrap();
    for _ in 0..100 {
        if state.create_seen.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(state.create_seen.load(Ordering::SeqCst));
    assert!(service.close(&session.id).is_err());
    service.cancel_transfer(&session.id, &job.id).unwrap();
    let result = wait(&service, &session.id, &job.id).await;
    assert_eq!(result.status, "cancelled");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        state.abort_seen.load(Ordering::SeqCst),
        "cancel must abort the upload after its create response reveals the id"
    );
    service.close(&session.id).unwrap();
    server.abort();
}
