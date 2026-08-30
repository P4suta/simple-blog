use std::{any::Any, time::Instant};

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tracing::{Instrument, field};
use uuid::Uuid;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

pub(super) async fn request_trace(mut request: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let request_id_header = HeaderValue::from_str(&request_id).expect("UUID is a valid header");
    request
        .headers_mut()
        .insert(REQUEST_ID, request_id_header.clone());
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let span = tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %method,
        path = %path,
        status = field::Empty,
        elapsed_ms = field::Empty,
    );
    let started = Instant::now();
    let mut response = next.run(request).instrument(span.clone()).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    span.record("status", response.status().as_u16());
    span.record("elapsed_ms", elapsed_ms);
    span.in_scope(|| {
        if response.status().is_server_error() {
            tracing::error!(event = "http.request.completed", "request completed");
        } else {
            tracing::info!(event = "http.request.completed", "request completed");
        }
    });
    response.headers_mut().insert(REQUEST_ID, request_id_header);
    response
}

pub(super) fn panic_response(_panic: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!(event = "http.request.panicked", "request panicked");
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use tower_http::catch_panic::CatchPanicLayer;

    use super::{panic_response, request_trace};

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

    #[tokio::test(flavor = "current_thread")]
    async fn panic_is_correlated_safely_and_becomes_a_recoverable_500() {
        async fn panic_handler() -> &'static str {
            panic!("panic-payload-must-not-enter-traces")
        }

        let traces = TraceBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(traces.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let app = Router::new()
            .route("/", get(panic_handler))
            .layer(CatchPanicLayer::custom(panic_response))
            .layer(middleware::from_fn(request_trace));

        let response = app
            .oneshot(Request::new(Body::empty()))
            .await
            .expect("panic layer response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let request_id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(uuid::Uuid::parse_str(&request_id).is_ok());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"Internal Server Error");

        let output = String::from_utf8(traces.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("event=\"http.request.panicked\""));
        assert!(output.contains(&request_id));
        assert!(output.contains("status=500"));
        assert!(!output.contains("panic-payload-must-not-enter-traces"));
    }
}
