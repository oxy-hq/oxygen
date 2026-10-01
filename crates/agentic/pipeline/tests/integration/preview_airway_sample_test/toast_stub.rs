//! A local stand-in for the Toast API, just enough for a windowed `orders`
//! pull: a login that hands out a token, and `ordersBulk` that answers one
//! order once and an empty page after. Nothing leaves the machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Serve on an ephemeral local port; the base URL to point a Toast source at.
pub(super) async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = Arc::new(AtomicBool::new(false));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let served = Arc::clone(&served);
            tokio::spawn(async move {
                let _ = answer(&mut sock, &served).await;
            });
        }
    });
    base
}

async fn answer(sock: &mut TcpStream, served: &AtomicBool) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = route(head.starts_with("POST"), &target, served);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(response.as_bytes()).await?;
    sock.shutdown().await
}

fn route(post: bool, target: &str, served: &AtomicBool) -> String {
    if post {
        return json!({ "token": { "accessToken": "stub", "tokenType": "Bearer", "expiresIn": 3600 } })
            .to_string();
    }
    if target.starts_with("/orders/v2/ordersBulk") && !served.swap(true, Ordering::SeqCst) {
        return json!([{
            "guid": "order-1",
            "businessDate": 20260922,
            "openedDate": "2026-09-22T10:00:00.000+0000",
            "modifiedDate": "2026-09-22T10:05:00.000+0000",
            "checks": [],
        }])
        .to_string();
    }
    "[]".to_string()
}
