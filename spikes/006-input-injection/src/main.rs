//! Spike 006: Input injection over Iroh
//!
//! Host mode:   cargo run -- host
//!   Starts Iroh endpoint, prints addr_json, streams frames, injects input
//!
//! Client mode: cargo run -- client <addr_json>
//!   Connects to host, receives frames, sends input events
//!
//! Input test:  cargo run -- client <addr_json> --test-input
//!   Connects, sends a test mouse move + click, then exits

use anyhow::{Context as _, Result, bail};
use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use iroh::endpoint::Connection;
use iroh::protocol::{ProtocolHandler, Router};
use enigo::{Enigo, Keyboard, Mouse, Settings, Button, Coordinate, Direction, Key};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ALPN: &[u8] = b"keyhome/input-stream/0";

// ─── Input event protocol ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum InputEvent {
    #[serde(rename = "mm")]
    MouseMove { x: i32, y: i32 },
    #[serde(rename = "mc")]
    MouseClick { b: u8 },  // 1=left 2=right 3=middle
    #[serde(rename = "md")]
    MouseDown { b: u8 },
    #[serde(rename = "mu")]
    MouseUp { b: u8 },
    #[serde(rename = "ms")]
    MouseScroll { dx: i32, dy: i32 },
    #[serde(rename = "kd")]
    KeyDown { k: String },
    #[serde(rename = "ku")]
    KeyUp { k: String },
    #[serde(rename = "kt")]
    KeyClick { k: String },
    #[serde(rename = "tx")]
    Text { s: String },
}

impl InputEvent {
    fn apply(&self, enigo: &mut Enigo) -> Result<()> {
        match self {
            InputEvent::MouseMove { x, y } => {
                enigo.move_mouse(*x, *y, Coordinate::Abs)
                    .map_err(|e| anyhow::anyhow!("mouse move: {:?}", e))?;
            }
            InputEvent::MouseClick { b } => {
                let btn = button_from_code(*b);
                enigo.button(btn, Direction::Click)
                    .map_err(|e| anyhow::anyhow!("mouse click: {:?}", e))?;
            }
            InputEvent::MouseDown { b } => {
                let btn = button_from_code(*b);
                enigo.button(btn, Direction::Press)
                    .map_err(|e| anyhow::anyhow!("mouse down: {:?}", e))?;
            }
            InputEvent::MouseUp { b } => {
                let btn = button_from_code(*b);
                enigo.button(btn, Direction::Release)
                    .map_err(|e| anyhow::anyhow!("mouse up: {:?}", e))?;
            }
            InputEvent::MouseScroll { dx, dy } => {
                if *dy != 0 {
                    enigo.scroll(*dy, enigo::Axis::Vertical)
                        .map_err(|e| anyhow::anyhow!("scroll: {:?}", e))?;
                }
                if *dx != 0 {
                    enigo.scroll(*dx, enigo::Axis::Horizontal)
                        .map_err(|e| anyhow::anyhow!("scroll: {:?}", e))?;
                }
            }
            InputEvent::KeyDown { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo.key(key, Direction::Press)
                        .map_err(|e| anyhow::anyhow!("key down: {:?}", e))?;
                }
            }
            InputEvent::KeyUp { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo.key(key, Direction::Release)
                        .map_err(|e| anyhow::anyhow!("key up: {:?}", e))?;
                }
            }
            InputEvent::KeyClick { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo.key(key, Direction::Click)
                        .map_err(|e| anyhow::anyhow!("key click: {:?}", e))?;
                }
            }
            InputEvent::Text { s } => {
                enigo.text(s)
                    .map_err(|e| anyhow::anyhow!("text: {:?}", e))?;
            }
        }
        Ok(())
    }

    fn to_json_line(&self) -> String {
        serde_json::to_string(self).unwrap() + "\n"
    }
}

fn button_from_code(b: u8) -> Button {
    match b {
        2 => Button::Right,
        3 => Button::Middle,
        4 => Button::Back,
        5 => Button::Forward,
        _ => Button::Left,
    }
}

fn key_from_str(s: &str) -> Option<Key> {
    match s {
        "Enter" => Some(Key::Return),
        "Tab" => Some(Key::Tab),
        "Space" => Some(Key::Space),
        "Backspace" => Some(Key::Backspace),
        "Escape" => Some(Key::Escape),
        "Shift" => Some(Key::Shift),
        "Control" => Some(Key::Control),
        "Alt" => Some(Key::Alt),
        "Meta" => Some(Key::Meta),
        "Up" => Some(Key::UpArrow),
        "Down" => Some(Key::DownArrow),
        "Left" => Some(Key::LeftArrow),
        "Right" => Some(Key::RightArrow),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        "Delete" => Some(Key::Delete),
        _ => {
            // Single character → Unicode key
            let c = s.chars().next()?;
            if c.is_ascii() {
                Some(Key::Unicode(c))
            } else {
                None
            }
        }
    }
}

// ─── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("host") => host().await,
        Some("client") => {
            let addr = args.get(1).context("missing addr arg")?;
            let test_input = args.iter().any(|a| a == "--test-input");
            client(addr, test_input).await
        }
        _ => {
            bail!("usage: cargo run -- host | client <addr_json> [--test-input]")
        }
    }
}

// ─── Host ────────────────────────────────────────────────────────────────────

async fn host() -> Result<()> {
    println!("[host] starting...");

    let secret = SecretKey::generate();
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await?;

    let _ = tokio::time::timeout(Duration::from_secs(5), endpoint.online()).await;

    let addr = endpoint.addr();
    let addr_json = serde_json::to_string(&addr)?;
    let node_id = endpoint.id();

    println!("[host] node_id={}", node_id);
    println!("{}", addr_json);
    eprintln!("[host] waiting for connections on ALPN {:?}...", ALPN);

    let handler = Arc::new(InputStreamHandler);
    let _router = Router::builder(endpoint.clone())
        .accept(ALPN, handler)
        .spawn();

    tokio::signal::ctrl_c().await?;
    endpoint.close().await;
    Ok(())
}

#[derive(Debug)]
struct InputStreamHandler;

impl ProtocolHandler for InputStreamHandler {
    async fn accept(&self, conn: Connection) -> Result<(), iroh::protocol::AcceptError> {
        if let Err(e) = handle_connection(conn).await {
            eprintln!("[host] error: {}", e);
        }
        Ok(())
    }
}

async fn handle_connection(conn: Connection) -> Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    eprintln!("[host] client connected: {}", conn.remote_id());

    // Wait for client start signal
    let mut start_buf = [0u8; 1];
    recv.read_exact(&mut start_buf).await?;
    if start_buf[0] != 1 {
        return Ok(());
    }
    eprintln!("[host] received start signal");

    // Read newline-delimited JSON input events, inject via enigo
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("enigo init: {:?}", e))?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    loop {
        let n = match recv.read(&mut chunk).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                eprintln!("[host] client disconnected");
                break;
            }
            Err(e) => {
                eprintln!("[host] read error: {}", e);
                break;
            }
        };
        if n == 0 {
            eprintln!("[host] client disconnected");
            break;
        }

        buf.extend_from_slice(&chunk[..n]);

        // Process complete lines
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line[..line.len()-1]);

            if line_str.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<InputEvent>(&line_str) {
                Ok(event) => {
                    eprintln!("[host] input: {:?}", event);
                    if let Err(e) = event.apply(&mut enigo) {
                        eprintln!("[host] inject error: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("[host] parse error: {} (line: {})", e, line_str);
                }
            }
        }
    }

    // Send ack
    let _ = send.write_all(b"bye\n").await;
    Ok(())
}

// ─── Client ──────────────────────────────────────────────────────────────────

async fn client(addr_str: &str, test_input: bool) -> Result<()> {
    println!("[client] connecting...");

    let addr: EndpointAddr = serde_json::from_str(addr_str)
        .context("invalid EndpointAddr JSON")?;

    let secret = SecretKey::generate();
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await?;

    let _ = tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await;

    let conn = endpoint.connect(addr, ALPN).await
        .context("failed to connect")?;
    println!("[client] connected!");

    let (mut send, mut recv) = conn.open_bi().await?;

    // Send start signal
    send.write_all(&[1u8]).await?;
    println!("[client] sent start signal");

    if test_input {
        // Send a sequence of test input events
        let events = vec![
            InputEvent::MouseMove { x: 500, y: 400 },
            InputEvent::MouseClick { b: 1 },
            InputEvent::Text { s: "Hello from keyhome!".to_string() },
            InputEvent::KeyClick { k: "Enter".to_string() },
        ];

        for event in &events {
            let line = event.to_json_line();
            println!("[client] sending: {}", line.trim());
            send.write_all(line.as_bytes()).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        println!("[client] test input sent, waiting for ack...");
        let mut ack = [0u8; 4];
        let _ = recv.read(&mut ack).await;
        println!("[client] done");
    } else {
        // Interactive mode: read input events from stdin
        println!("[client] interactive mode — type JSON input events, Ctrl+D to quit");
        println!("[client] examples:");
        println!("  {{\"t\":\"mm\",\"x\":100,\"y\":100}}");
        println!("  {{\"t\":\"mc\",\"b\":1}}");
        println!("  {{\"t\":\"tx\",\"s\":\"hello\"}}");
        println!("  {{\"t\":\"kt\",\"k\":\"Enter\"}}");

        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(tokio::io::stdin());
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await
                .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
            if n == 0 {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            // Validate JSON
            match serde_json::from_str::<InputEvent>(&line) {
                Ok(event) => {
                    let json = event.to_json_line();
                    send.write_all(json.as_bytes()).await?;
                    println!("[client] sent: {}", json.trim());
                }
                Err(e) => {
                    println!("[client] invalid input: {} (must be valid InputEvent JSON)", e);
                }
            }
        }
    }

    let _ = send.finish();
    let _ = endpoint.close().await;
    Ok(())
}
