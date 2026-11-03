//! Go `GinLogrusRecovery` (internal/logging/gin_logger.go, on every route in
//! internal/api/server.go): a panic in a handler, a translator or a layer answers 500 and
//! is logged, instead of unwinding the hyper connection task, where the client saw a
//! reset and, on HTTP/2, every stream on the connection died with it.
//!
//! gin's `CustomRecovery` ends with `c.AbortWithStatus(500)`: the bare status, no body,
//! so the answer here is the same empty 500. Go logs the panic value, the stack and the
//! request path; here the panic payload text alone, never request bytes.
// ponytail: a panic before the response head is caught here. A panic while a streaming
// body is polled happens in hyper, after this future resolved, and still ends the
// connection, as it does in Go once the headers are flushed.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt;

/// The outermost layer of the served app: `axum::middleware::from_fn(recovery::recover)`.
pub async fn recover(req: Request, next: Next) -> Response {
    match std::panic::AssertUnwindSafe(next.run(req)).catch_unwind().await {
        Ok(response) => response,
        Err(payload) => {
            tracing::error!(panic = %panic_text(payload.as_ref()), "recovered from panic");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The message `panic!` carried; a placeholder for any other payload type.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One HTTP/1.1 response: the head, then as many body bytes as `Content-Length` says.
    async fn read_response(stream: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let end = loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed after {:?}", String::from_utf8_lossy(&buf));
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
        let length: usize = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while buf.len() < end + length {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "body cut short");
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8_lossy(&buf[..end + length]).into_owned()
    }

    /// Go `GinLogrusRecovery`: the panicking request gets gin's empty 500 and the next
    /// request on the same connection is served.
    #[tokio::test]
    async fn a_panicking_handler_answers_500_and_the_connection_survives() {
        let app = Router::new()
            .route(
                "/panic",
                get(|| async {
                    if std::hint::black_box(true) {
                        panic!("handler panicked");
                    }
                    "unreachable"
                }),
            )
            .route("/ok", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(recover));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /panic HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let first = read_response(&mut stream).await;
        assert!(first.starts_with("HTTP/1.1 500 Internal Server Error\r\n"), "{first}");
        assert!(
            first.ends_with("\r\n\r\n"),
            "no body, as gin's AbortWithStatus writes none: {first}"
        );
        stream.write_all(b"GET /ok HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let second = read_response(&mut stream).await;
        assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
        assert!(second.ends_with("\r\n\r\nok"), "{second}");
    }

    #[test]
    fn payload_text_is_the_panic_message() {
        let text: Box<dyn std::any::Any + Send> = Box::new("as str");
        assert_eq!(panic_text(text.as_ref()), "as str");
        let owned: Box<dyn std::any::Any + Send> = Box::new(String::from("as string"));
        assert_eq!(panic_text(owned.as_ref()), "as string");
        let other: Box<dyn std::any::Any + Send> = Box::new(7u8);
        assert_eq!(panic_text(other.as_ref()), "non-string panic payload");
    }
}
