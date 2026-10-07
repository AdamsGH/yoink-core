use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, Notify},
};
use tokio_util::sync::CancellationToken;
use yoink_insight::{
    Error, FutureResult,
    provider::{Completion, DeltaSink, HttpCompletion, Route, Shape},
};

#[derive(Default)]
struct Sink {
    text: Mutex<String>,
    first: Notify,
    fail: bool,
}
impl DeltaSink for Sink {
    fn delta<'a>(&'a self, text: &'a str) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.text.lock().await.push_str(text);
            self.first.notify_one();
            if self.fail {
                Err(Error::Code("delivery_failed"))
            } else {
                Ok(())
            }
        })
    }
}
fn route(base: String, shape: Shape) -> Route {
    Route {
        base,
        shape,
        key: "fixture-only".into(),
        model: "test".into(),
        via: "gateway",
    }
}
// Raw HTTP separates first delta from terminal bytes. The server will not send
// the terminal until the consumer acknowledges the first actual network delta.
async fn server(
    shape: Shape,
    first: &'static str,
    last: &'static str,
    sink: Arc<Sink>,
) -> (Route, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let r = route(format!("http://{}", listener.local_addr().unwrap()), shape);
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let size: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + size {
                    let body: serde_json::Value =
                        serde_json::from_slice(&request[end + 4..]).unwrap();
                    if shape == Shape::Gemini {
                        assert!(headers.contains(":streamGenerateContent?alt=sse"));
                    } else {
                        assert_eq!(body["stream"], true);
                    }
                    break;
                }
            }
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        // Splitting every byte also exercises fragmented UTF-8 and SSE framing.
        for byte in first.as_bytes() {
            socket.write_all(&[*byte]).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), sink.first.notified())
            .await
            .unwrap();
        let _ = socket.write_all(last.as_bytes()).await;
    });
    (r, task)
}
#[tokio::test]
async fn deltas_arrive_before_terminal_for_all_shapes() {
    for (shape, first, last) in [
        (
            Shape::OpenAi,
            "data: {\"choices\":[{\"delta\":{\"content\":\"hé\"}}]}\r\n\r\n",
            "data: [DONE]\n\n",
        ),
        (
            Shape::Anthropic,
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hé\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ),
        (
            Shape::Gemini,
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hé\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
        ),
    ] {
        let sink = Arc::new(Sink::default());
        let (r, task) = server(shape, first, last, sink.clone()).await;
        let client = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
        assert_eq!(
            client
                .stream(&[r], "hello", &*sink, CancellationToken::new())
                .await
                .unwrap(),
            "hé"
        );
        assert_eq!(*sink.text.lock().await, "hé");
        task.await.unwrap();
    }
}
#[tokio::test]
async fn partial_stream_failure_never_falls_back() {
    for (last, code) in [
        ("", "stream_incomplete"),
        ("data: not-json\n\n", "parse_failed"),
        (
            "data: {\"error\":{\"message\":\"secret\"}}\n\n",
            "llm_error",
        ),
    ] {
        let sink = Arc::new(Sink::default());
        let (r, task) = server(
            Shape::OpenAi,
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            last,
            sink.clone(),
        )
        .await;
        let fallback = route("http://127.0.0.1:1".into(), Shape::OpenAi);
        let client = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
        assert!(
            matches!(client.stream(&[r, fallback], "hello", &*sink, CancellationToken::new()).await, Err(Error::Code(c)) if c == code)
        );
        task.await.unwrap();
    }
}
#[tokio::test]
async fn delivery_failure_and_cancellation_abort_generation() {
    let sink = Arc::new(Sink {
        fail: true,
        ..Default::default()
    });
    let (r, task) = server(
        Shape::OpenAi,
        "data: {\"choices\":[{\"delta\":{\"content\":\"draft\"}}]}\n\n",
        "data: [DONE]\n\n",
        sink.clone(),
    )
    .await;
    let client = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
    assert!(matches!(
        client
            .stream(&[r], "hello", &*sink, CancellationToken::new())
            .await,
        Err(Error::Code("delivery_failed"))
    ));
    task.await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        client
            .stream(
                &[route("http://127.0.0.1:1".into(), Shape::OpenAi)],
                "hello",
                &*sink,
                cancel
            )
            .await,
        Err(Error::Code("cancelled"))
    ));
}

#[tokio::test]
async fn fallback_before_emission_but_not_for_byok() {
    use axum::{Router, http::StatusCode, routing::post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/chat/completions",
                post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            ),
        )
        .await
        .unwrap();
    });
    let sink = Arc::new(Sink::default());
    let (backup, server_task) = server(
        Shape::OpenAi,
        "data: {\"choices\":[{\"delta\":{\"content\":\"backup\"}}]}\n\n",
        "data: [DONE]\n\n",
        sink.clone(),
    )
    .await;
    let client = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
    assert_eq!(
        client
            .stream(
                &[route(base, Shape::OpenAi), backup],
                "hello",
                &*sink,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        "backup"
    );
    server_task.await.unwrap();
    task.abort();
    // BYOK DNS confinement is a pre-emission failure; it must not route the
    // user's private prompt to a coordinator-configured fallback provider.
    let mut byok = route("https://localhost".into(), Shape::OpenAi);
    byok.via = "byok";
    assert!(matches!(
        client
            .stream(
                &[byok, route("http://127.0.0.1:1".into(), Shape::OpenAi)],
                "private",
                &*sink,
                CancellationToken::new()
            )
            .await,
        Err(Error::Code("unsafe_endpoint"))
    ));
}

#[tokio::test]
async fn cancellation_during_awaited_delivery_interrupts_stream() {
    struct Blocking {
        first: Arc<Notify>,
    }
    impl DeltaSink for Blocking {
        fn delta<'a>(&'a self, _: &'a str) -> FutureResult<'a, ()> {
            Box::pin(async move {
                self.first.notify_one();
                std::future::pending().await
            })
        }
    }
    use axum::{Router, routing::post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let r = route(
        format!("http://{}", listener.local_addr().unwrap()),
        Shape::OpenAi,
    );
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/chat/completions",
                post(|| async { "data: {\"choices\":[{\"delta\":{\"content\":\"draft\"}}]}\n\n" }),
            ),
        )
        .await
        .unwrap();
    });
    let first = Arc::new(Notify::new());
    let sink = Blocking {
        first: first.clone(),
    };
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let cancelling = tokio::spawn(async move {
        first.notified().await;
        child.cancel();
    });
    let client = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
    assert!(matches!(
        client.stream(&[r], "hello", &sink, cancel).await,
        Err(Error::Code("cancelled"))
    ));
    cancelling.await.unwrap();
    task.abort();
}
