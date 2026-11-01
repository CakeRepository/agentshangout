//! Temporary probe for this pull request only, removed before it is ready. Prints straight to the
//! log, past the test harness, so CI shows what a Linux runner sees before the fix goes in.

use std::{
    collections::BTreeMap,
    error::Error,
    io::Write,
    time::{Duration, Instant},
};

use agenthangout::{app, db::Db, AppState, Config};
use reqwest::{Client, Method};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

fn say(line: String) {
    let mut out = std::io::stdout();
    out.write_all(format!("LINUXPROBE {line}\n").as_bytes()).unwrap();
    out.flush().unwrap();
}

fn root_cause(e: &(dyn Error + 'static)) -> String {
    let mut at = e;
    while let Some(next) = at.source() {
        at = next;
    }
    format!("{at}")
}

async fn serve() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        Db::open(":memory:").unwrap(),
        Config {
            public_url: format!("http://{addr}"),
            create_code: None,
            allow_private_webhooks: true,
            keeper_id: None,
            god_ids: Vec::new(),
        },
    );
    tokio::spawn(async move { axum::serve(listener, app(state)).await });
    addr
}

fn head(path: &str, length: usize) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nhost: probe\r\ncontent-type: application/json\r\n\
         x-forwarded-for: 10.1.2.3\r\ncontent-length: {length}\r\n\r\n"
    )
}

async fn through_reqwest(addr: std::net::SocketAddr, size: usize, rounds: usize) {
    let client = Client::new();
    let routes = [
        "/api/status",
        "/api/activity",
        "/api/worlds/commons/act",
        "/api/passkeys/login/start",
        "/api/public/join",
    ];
    let body = vec![b'z'; size];
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for round in 0..rounds {
        for path in routes {
            let sent = client
                .request(Method::POST, format!("http://{addr}{path}"))
                .header("x-forwarded-for", format!("10.0.{}.{}", round / 256, round % 256))
                .header("content-type", "application/json")
                .body(body.clone())
                .timeout(Duration::from_secs(60))
                .send()
                .await;
            let what = match sent {
                Ok(response) => {
                    let status = response.status().as_u16();
                    match response.text().await {
                        Ok(_) => format!("{status}"),
                        Err(e) => format!("read failed: {}", root_cause(&e)),
                    }
                }
                Err(e) => format!("send failed: {}", root_cause(&e)),
            };
            *seen.entry(what).or_default() += 1;
        }
    }
    say(format!("reqwest {size} bytes, {} requests: {seen:?}", rounds * routes.len()));
}

async fn fast_sender(addr: std::net::SocketAddr, size: usize, rounds: usize) {
    let mut request = head("/api/status", size).into_bytes();
    request.extend_from_slice(&vec![b'z'; size]);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..rounds {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let wrote = stream.write_all(&request).await;
        let mut answer = Vec::new();
        let read = stream.read_to_end(&mut answer).await;
        let line = String::from_utf8_lossy(&answer);
        let first = line.lines().next().unwrap_or("nothing").to_string();
        let what = format!(
            "write {} / read {} / {first}",
            wrote.map_or_else(|e| format!("{:?}", e.raw_os_error()), |_| "ok".to_string()),
            read.map_or_else(|e| format!("{:?}", e.raw_os_error()), |_| "eof".to_string()),
        );
        *seen.entry(what).or_default() += 1;
    }
    say(format!("one write then read, {size} bytes, {rounds} requests: {seen:?}"));
}

async fn slow_sender(addr: std::net::SocketAddr, size: usize) {
    for path in ["/api/status", "/api/passkeys/login/start", "/api/public/join"] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body = vec![b'z'; size];
        let mut first = head(path, size).into_bytes();
        first.extend_from_slice(&body[..1000]);
        let wrote_first = stream.write_all(&first).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let wrote_rest = stream.write_all(&body[1000..]).await;
        let mut answer = vec![0; 4096];
        let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut answer)).await;
        let what = match read {
            Ok(Ok(n)) => format!("read {n}: {:?}", String::from_utf8_lossy(&answer[..n.min(24)])),
            Ok(Err(e)) => format!("read failed: {e}"),
            Err(_) => "read timed out".to_string(),
        };
        say(format!("paused sender {size} bytes {path}: {wrote_first:?} {wrote_rest:?} {what}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_linux_runner_sees() {
    let started = Instant::now();
    let addr = serve().await;
    say(format!("os {}", std::env::consts::OS));
    for size in [5_000, 70_000, 200_000] {
        slow_sender(addr, size).await;
    }
    for size in [70_000, 200_000, 1_000_000, 8_000_000] {
        fast_sender(addr, size, 2000).await;
    }
    for size in [60_000, 70_000, 200_000, 1_000_000] {
        through_reqwest(addr, size, 2000).await;
    }
    say(format!("took {:?}", started.elapsed()));
}

#[tokio::test]
async fn reqwest_on_one_thread_like_the_round_two_test() {
    let addr = serve().await;
    through_reqwest(addr, 70_000, 4000).await;
}
