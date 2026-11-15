//! Backends that write down every call they receive, so a test can say what a
//! preview run sent and — the point — what it did not.
//!
//! * [`FakeClickHouse`] speaks the ClickHouse HTTP interface the connector
//!   uses (the SQL is the POST body) and answers every query with one row.
//! * [`FakeEgress`] is an HTTPS proxy. `http_request` refuses loopback and
//!   plain-HTTP targets before it builds a request (its SSRF guard), so no
//!   local server can stand in for the far end of a POST; the process's
//!   HTTPS egress can be routed through one, though, and a request that was
//!   sent shows up here as the `CONNECT` that opens its tunnel. The tunnel is
//!   always refused: nothing leaves the machine.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::fixture::EnvGuard;

type Log = Arc<Mutex<Vec<String>>>;

/// One `UInt64` column `n`, one row: enough for the connector's count, sample
/// and per-column stats queries alike.
const ONE_ROW: &str = r#"{"meta":[{"name":"n","type":"UInt64"}],"data":[["3"]],"rows":1}"#;

pub(crate) struct FakeClickHouse {
    pub url: String,
    log: Log,
    server: JoinHandle<()>,
}

impl FakeClickHouse {
    pub(crate) async fn start() -> Self {
        let log = Log::default();
        let app = axum::Router::new()
            .fallback(record_statement)
            .with_state(log.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, log, server }
    }

    /// Every SQL body received, in order.
    pub(crate) fn statements(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FakeClickHouse {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn record_statement(State(log): State<Log>, sql: String) -> impl IntoResponse {
    log.lock().unwrap().push(sql);
    ([(CONTENT_TYPE, "application/json")], ONE_ROW)
}

pub(crate) struct FakeEgress {
    url: String,
    log: Log,
    server: JoinHandle<()>,
}

impl FakeEgress {
    pub(crate) async fn start() -> Self {
        let log = Log::default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let tunnels = log.clone();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(refuse_tunnel(stream, tunnels.clone()));
            }
        });
        Self { url, log, server }
    }

    /// Send this process's HTTPS egress through the proxy until the guard
    /// drops. reqwest reads the proxy variables whenever a client is built,
    /// and `http_request` builds one per step, so this takes effect for every
    /// step from here on. Plain HTTP (the fake warehouse on loopback) is left
    /// direct. Before `worker::start` (see [`EnvGuard`]).
    pub(super) fn route_https_egress_here(&self) -> EnvGuard {
        let url = Some(self.url.as_str());
        let local = Some("127.0.0.1,localhost");
        EnvGuard::set(&[
            ("HTTPS_PROXY", url),
            ("https_proxy", url),
            ("HTTP_PROXY", None),
            ("http_proxy", None),
            ("ALL_PROXY", None),
            ("all_proxy", None),
            ("NO_PROXY", local),
            ("no_proxy", local),
        ])
    }

    /// The request line of every tunnel asked for, e.g.
    /// `CONNECT hooks.example.test:443 HTTP/1.1`.
    pub(crate) fn tunnels(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FakeEgress {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Read the proxy request's head, write down its first line, refuse it.
async fn refuse_tunnel(mut stream: tokio::net::TcpStream, log: Log) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    if let Some(line) = text.lines().next().filter(|l| !l.is_empty()) {
        log.lock().unwrap().push(line.to_string());
    }
    let _ = stream
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        .await;
    let _ = stream.shutdown().await;
}
