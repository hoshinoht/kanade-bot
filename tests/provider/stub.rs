//! Loopback-only HTTP/1.1 stub for the OpenAI-compatible adapter.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub enum Reply {
    Json(u16, Value),
    Raw(Vec<u8>),
    Hang,
    /// A healthy but slow reply.
    Slow(Duration, u16, Value),
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct Stub {
    pub addr: SocketAddr,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl Stub {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        Self::start_with(handler, None).await
    }

    pub async fn start_with(
        handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
        tls: Option<TlsAcceptor>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let log = recorded.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    match tls {
                        Some(acceptor) => {
                            if let Ok(stream) = acceptor.accept(stream).await {
                                serve(stream, handler, log).await;
                            }
                        }
                        None => serve(stream, handler, log).await,
                    }
                });
            }
        });
        Self { addr, recorded }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }

    pub fn chat_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.path.ends_with("/chat/completions"))
            .collect()
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    handler: Arc<Handler>,
    log: Arc<Mutex<Vec<Recorded>>>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let method = start.next().unwrap_or_default().to_owned();
    let path = start.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    let recorded = Recorded {
        method,
        path,
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    };
    log.lock().unwrap().push(recorded.clone());
    let reply = match handler(&recorded) {
        Reply::Slow(delay, status, value) => {
            tokio::time::sleep(delay).await;
            Reply::Json(status, value)
        }
        reply => reply,
    };
    let bytes = match reply {
        Reply::Slow(..) => unreachable!("resolved above"),
        Reply::Json(status, value) => {
            let body = value.to_string();
            format!(
                "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes()
        }
        Reply::Raw(bytes) => bytes,
        Reply::Hang => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
    };
    let _ = stream.write_all(&bytes).await;
    let _ = stream.shutdown().await;
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn completion(model: &str, content: &str) -> Value {
    json!({
        "id": "chatcmpl-synthetic",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2}
    })
}

pub fn kanata_models() -> Value {
    serde_json::from_str(include_str!("../fixtures/provider/models-kanata.json")).unwrap()
}

pub fn ollama_models() -> Value {
    serde_json::from_str(include_str!("../fixtures/provider/models-ollama.json")).unwrap()
}

/// Serves `models` for `GET .../models` and a completion echoing the request model.
pub fn gateway(
    models: Value,
    content: &'static str,
) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| {
        if request.method == "GET" && request.path.ends_with("/models") {
            Reply::Json(200, models.clone())
        } else {
            let model = request.body["model"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Reply::Json(200, completion(&model, content))
        }
    }
}
