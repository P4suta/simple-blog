use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use simple_blog::{
    config::{Config, ConfigSources, Overrides},
    infrastructure::sqlite::SqliteRepository,
    web::{AppState, router},
};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct TraceBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for TraceBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for TraceBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

async fn test_state() -> (tempfile::TempDir, Arc<SqliteRepository>, AppState) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().to_path_buf()),
            public_url: Some("http://localhost:8080".into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let state = AppState::new(config, repository.clone()).unwrap();
    (temp, repository, state)
}

#[tokio::test(flavor = "current_thread")]
async fn trace_correlates_the_response_without_recording_query_secrets() {
    let (_temp, _repository, state) = test_state().await;
    let traces = TraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(traces.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/healthz?token=must-never-be-logged")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request_id = response.headers()["x-request-id"].to_str().unwrap();
    let output = String::from_utf8(traces.0.lock().unwrap().clone()).unwrap();
    let events = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let completed = events
        .iter()
        .find(|event| event["fields"]["event"] == "http.request.completed")
        .expect("request completion event");
    let span = &completed["span"];
    assert_eq!(span["name"], "http.request");
    assert_eq!(span["request_id"], request_id);
    assert_eq!(span["method"], "GET");
    assert_eq!(span["path"], "/healthz");
    assert_eq!(span["status"], 200);
    assert!(span["elapsed_ms"].is_number());
    assert!(!output.contains("must-never-be-logged"));
}

#[tokio::test(flavor = "current_thread")]
async fn internal_failure_has_a_stable_error_code_and_the_same_request_id() {
    let (_temp, repository, state) = test_state().await;
    repository.close().await;
    let traces = TraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(traces.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::HOST, "localhost:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"Internal Server Error");

    let output = String::from_utf8(traces.0.lock().unwrap().clone()).unwrap();
    let events = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let failure = events
        .iter()
        .find(|event| event["fields"]["event"] == "http.request.failed")
        .expect("typed failure event");
    assert_eq!(failure["fields"]["error_code"], "repository.storage");
    assert_eq!(failure["span"]["request_id"], request_id);
    let completed = events
        .iter()
        .find(|event| event["fields"]["event"] == "http.request.completed")
        .expect("completion event");
    assert_eq!(completed["span"]["request_id"], request_id);
    assert_eq!(completed["span"]["status"], 500);
}
