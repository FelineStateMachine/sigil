use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use iroh::{endpoint::Connection, protocol::ProtocolHandler};
use serde::{Deserialize, Serialize};

// ─── Input Event Protocol ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t")]
pub enum InputEvent {
    #[serde(rename = "mm")]
    MouseMove { x: i32, y: i32 },
    #[serde(rename = "mc")]
    MouseClick { b: u8 },
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
    pub fn apply(&self, enigo: &mut Enigo) -> anyhow::Result<()> {
        match self {
            InputEvent::MouseMove { x, y } => {
                enigo
                    .move_mouse(*x, *y, Coordinate::Abs)
                    .map_err(|e| anyhow::anyhow!("mouse move: {:?}", e))?;
            }
            InputEvent::MouseClick { b } => {
                enigo
                    .button(button_from_code(*b), Direction::Click)
                    .map_err(|e| anyhow::anyhow!("mouse click: {:?}", e))?;
            }
            InputEvent::MouseDown { b } => {
                enigo
                    .button(button_from_code(*b), Direction::Press)
                    .map_err(|e| anyhow::anyhow!("mouse down: {:?}", e))?;
            }
            InputEvent::MouseUp { b } => {
                enigo
                    .button(button_from_code(*b), Direction::Release)
                    .map_err(|e| anyhow::anyhow!("mouse up: {:?}", e))?;
            }
            InputEvent::MouseScroll { dx, dy } => {
                if *dy != 0 {
                    enigo
                        .scroll(*dy, enigo::Axis::Vertical)
                        .map_err(|e| anyhow::anyhow!("scroll: {:?}", e))?;
                }
                if *dx != 0 {
                    enigo
                        .scroll(*dx, enigo::Axis::Horizontal)
                        .map_err(|e| anyhow::anyhow!("scroll: {:?}", e))?;
                }
            }
            InputEvent::KeyDown { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo
                        .key(key, Direction::Press)
                        .map_err(|e| anyhow::anyhow!("key down: {:?}", e))?;
                }
            }
            InputEvent::KeyUp { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo
                        .key(key, Direction::Release)
                        .map_err(|e| anyhow::anyhow!("key up: {:?}", e))?;
                }
            }
            InputEvent::KeyClick { k } => {
                if let Some(key) = key_from_str(k) {
                    enigo
                        .key(key, Direction::Click)
                        .map_err(|e| anyhow::anyhow!("key click: {:?}", e))?;
                }
            }
            InputEvent::Text { s } => {
                enigo
                    .text(s)
                    .map_err(|e| anyhow::anyhow!("text: {:?}", e))?;
            }
        }
        Ok(())
    }
}

fn button_from_code(b: u8) -> Button {
    match b {
        2 => Button::Right,
        3 => Button::Middle,
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
            let c = s.chars().next()?;
            if c.is_ascii() {
                Some(Key::Unicode(c))
            } else {
                None
            }
        }
    }
}

// ─── Input Stream Handler (host side) ─────────────────────────────────────────

#[derive(Debug)]
pub struct InputStreamHandler;

impl ProtocolHandler for InputStreamHandler {
    async fn accept(&self, conn: Connection) -> Result<(), iroh::protocol::AcceptError> {
        if let Err(e) = handle_input(conn).await {
            eprintln!("[host] input error: {}", e);
        }
        Ok(())
    }
}

pub async fn handle_input(conn: Connection) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    eprintln!("[host] input client connected: {}", conn.remote_id());

    let mut start_buf = [0u8; 1];
    recv.read_exact(&mut start_buf).await?;
    if start_buf[0] != 1 {
        return Ok(());
    }

    // Bounded channel: limits enigo backlog without blocking the async reader.
    // try_send on full channel drops the event rather than stalling the stream.
    let (tx, rx) = std::sync::mpsc::sync_channel::<InputEvent>(64);
    std::thread::spawn(move || {
        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[host] enigo init: {:?}", e);
                return;
            }
        };
        for event in rx {
            eprintln!("[host] input: {:?}", event);
            if let Err(e) = event.apply(&mut enigo) {
                eprintln!("[host] inject error: {}", e);
            }
        }
    });

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    loop {
        let n = match recv.read(&mut chunk).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                eprintln!("[host] input client disconnected");
                break;
            }
            Err(e) => {
                eprintln!("[host] input read error: {}", e);
                break;
            }
        };
        if n == 0 {
            break;
        }

        buf.extend_from_slice(&chunk[..n]);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line[..line.len() - 1]);

            if line_str.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<InputEvent>(&line_str) {
                Ok(event) => {
                    if tx.try_send(event).is_err() {
                        eprintln!("[host] input dropped (enigo backlogged)");
                    }
                }
                Err(e) => {
                    eprintln!("[host] parse error: {} (line: {})", e, line_str);
                }
            }
        }
    }

    drop(tx);
    let _ = send.write_all(b"bye\n").await;
    Ok(())
}
