use anyhow::Context as _;
use iroh::{endpoint::Connection, protocol::ProtocolHandler};
use openh264::{
    encoder::Encoder,
    formats::{RgbSliceU8, YUVBuffer},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use super::state::{AppState, EncoderConfig};

// ─── Codec helpers ────────────────────────────────────────────────────────────

pub fn codec_to_byte(codec: &str) -> u8 {
    match codec {
        "h265" => 1,
        "av1" => 2,
        _ => 0,
    }
}

pub fn byte_to_codec(b: u8) -> &'static str {
    match b {
        1 => "h265",
        2 => "av1",
        _ => "h264",
    }
}

// ─── Encoder config commands ──────────────────────────────────────────────────

#[tauri::command]
pub fn get_encoder_config(state: State<'_, AppState>) -> EncoderConfig {
    state.encoder_config.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_encoder_config(app: AppHandle, state: State<'_, AppState>, config: EncoderConfig) {
    *state.encoder_config.lock().unwrap() = config.clone();
    if let Ok(data_dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&data_dir);
        let path = data_dir.join("encoder_config.json");
        if let Ok(json) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[tauri::command]
pub fn detect_available_encoders() -> Vec<String> {
    let encoders = std::process::Command::new("ffmpeg")
        .arg("-encoders")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let all = [
        "h264_nvenc", "h264_vaapi", "h264_qsv", "h264_amf", "h264_videotoolbox", "libx264",
        "hevc_nvenc", "hevc_vaapi", "hevc_qsv", "hevc_amf", "hevc_videotoolbox", "libx265",
        "av1_nvenc", "av1_vaapi", "av1_qsv", "av1_amf", "av1_videotoolbox", "libsvtav1",
        "libaom-av1",
    ];

    all.iter()
        .filter(|name| encoders.contains(*name))
        .map(|s| s.to_string())
        .collect()
}

// ─── Frame Stream Handler (host side) ────────────────────────────────────────

#[derive(Debug)]
pub struct FrameStreamHandler {
    pub connections: Arc<std::sync::atomic::AtomicU32>,
    pub app: AppHandle,
}

impl ProtocolHandler for FrameStreamHandler {
    async fn accept(&self, conn: Connection) -> Result<(), iroh::protocol::AcceptError> {
        let count = self.connections.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.app.emit("host-connections", count);
        eprintln!("[host] client connected: {} (total: {})", conn.remote_id(), count);

        let conn_clone = conn.clone();
        let app_clone = self.app.clone();
        let mut stream_task =
            tokio::spawn(async move { stream_frames(conn_clone, &app_clone).await });

        tokio::select! {
            result = &mut stream_task => {
                if let Ok(Err(e)) = result {
                    eprintln!("[host] stream error: {}", e);
                }
            }
            _ = conn.closed() => {
                eprintln!("[host] connection closed by peer");
                stream_task.abort();
            }
        }

        let count = self.connections.fetch_sub(1, Ordering::SeqCst) - 1;
        let _ = self.app.emit("host-connections", count);
        eprintln!("[host] client disconnected (total: {})", count);
        Ok(())
    }
}

async fn stream_frames(conn: Connection, app: &AppHandle) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;

    let mut start_buf = [0u8; 1];
    recv.read_exact(&mut start_buf).await?;
    if start_buf[0] != 1 {
        return Ok(());
    }

    let (w, h) = {
        let monitors = xcap::Monitor::all().context("failed to enumerate monitors")?;
        let mon = monitors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no monitors found"))?;
        let img = mon.capture_image()?;
        (img.width() as usize, img.height() as usize)
    };
    eprintln!("[host] screen resolution: {}x{}", w, h);

    if ffmpeg_available() {
        let config = {
            let app_state = app.state::<AppState>();
            app_state.encoder_config.lock().unwrap().clone()
        };
        eprintln!(
            "[host] using ffmpeg: codec={} backend={}",
            config.codec, config.backend
        );
        match stream_frames_ffmpeg(&mut send, &mut recv, app, w, h, &config).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("[host] ffmpeg failed: {}, falling back to xcap+openh264", e);
            }
        }
    }

    eprintln!("[host] using xcap + openh264");
    stream_frames_xcap(&mut send, &mut recv, app).await
}

// ─── ffmpeg subprocess path ───────────────────────────────────────────────────

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn resolve_encoder(codec: &str, backend: &str) -> String {
    let encoders = std::process::Command::new("ffmpeg")
        .arg("-encoders")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let has = |name: &str| encoders.contains(name);

    let hw_backends: &[&str] = if cfg!(target_os = "macos") {
        &["videotoolbox"]
    } else if cfg!(target_os = "windows") {
        &["nvenc", "qsv", "amf"]
    } else {
        &["nvenc", "vaapi"]
    };

    let codec_prefix = match codec {
        "h265" => "hevc",
        "av1" => "av1",
        _ => "h264",
    };

    if backend != "auto" {
        let name = if backend == "software" {
            match codec {
                "h265" => "libx265",
                "av1" => "libsvtav1",
                _ => "libx264",
            }
        } else {
            &format!("{}_{}", codec_prefix, backend)
        };
        if has(name) {
            return name.to_string();
        }
    }

    for hw in hw_backends {
        let name = format!("{}_{}", codec_prefix, hw);
        if has(&name) {
            return name;
        }
    }

    match codec {
        "h265" => "libx265".to_string(),
        "av1" => {
            if has("libsvtav1") { "libsvtav1".to_string() } else { "libaom-av1".to_string() }
        }
        _ => "libx264".to_string(),
    }
}

fn codec_format(codec: &str) -> (&'static str, &'static str) {
    match codec {
        "h265" => ("hevc", "hevc_metadata=aud=insert"),
        "av1" => ("av1", "av1_metadata=td=insert"),
        _ => ("h264", "h264_metadata=aud=insert"),
    }
}

// ─── Frame / keyframe detection ───────────────────────────────────────────────

pub fn find_next_frame_delim(data: &[u8], from: usize, codec: &str) -> Option<usize> {
    let mut i = from;
    while i + 4 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                // 4-byte start code
                if codec == "av1" {
                    if i + 4 < data.len() && (data[i + 4] & 0x38) == 0x10 {
                        return Some(i);
                    }
                } else if codec == "h265" {
                    if i + 4 < data.len() && ((data[i + 4] >> 1) & 0x3f) == 35 {
                        return Some(i);
                    }
                } else if i + 4 < data.len() && (data[i + 4] & 0x1f) == 9 {
                    return Some(i);
                }
                i += 4;
                continue;
            }
            if data[i + 2] == 1 {
                // 3-byte start code
                if codec == "av1" {
                    if i + 3 < data.len() && (data[i + 3] & 0x38) == 0x10 {
                        return Some(i);
                    }
                } else if codec == "h265" {
                    if i + 3 < data.len() && ((data[i + 3] >> 1) & 0x3f) == 35 {
                        return Some(i);
                    }
                } else if i + 3 < data.len() && (data[i + 3] & 0x1f) == 9 {
                    return Some(i);
                }
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    None
}

pub fn frame_is_keyframe(data: &[u8], codec: &str) -> bool {
    match codec {
        "h265" => {
            let mut i = 0;
            while i + 3 < data.len() {
                if data[i] == 0 && data[i + 1] == 0 {
                    let (sc_len, nal_off) =
                        if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                            (4, 4)
                        } else if data[i + 2] == 1 {
                            (3, 3)
                        } else {
                            i += 1;
                            continue;
                        };
                    if i + nal_off < data.len() {
                        let nal_type = (data[i + nal_off] >> 1) & 0x3f;
                        if nal_type == 19 || nal_type == 20 || nal_type == 32 || nal_type == 33 {
                            return true;
                        }
                    }
                    i += sc_len;
                } else {
                    i += 1;
                }
            }
            false
        }
        "av1" => {
            // AV1: scan OBUs for OBU_SEQUENCE_HEADER (type 1), which only appears in keyframes.
            // OBU header: forbidden(1) | obu_type(4) | extension_flag(1) | has_size(1) | reserved(1)
            let mut i = 0;
            while i < data.len() {
                let header_byte = data[i];
                let obu_type = (header_byte >> 3) & 0x0F;
                let has_extension = (header_byte >> 2) & 1 == 1;
                let has_size = (header_byte >> 1) & 1 == 1;
                i += 1;
                if has_extension {
                    if i >= data.len() {
                        break;
                    }
                    i += 1;
                }
                let obu_size = if has_size {
                    let mut size: usize = 0;
                    let mut shift = 0usize;
                    loop {
                        if i >= data.len() {
                            break;
                        }
                        let b = data[i];
                        i += 1;
                        size |= ((b & 0x7F) as usize) << shift;
                        shift += 7;
                        if (b & 0x80) == 0 {
                            break;
                        }
                    }
                    size
                } else {
                    data.len().saturating_sub(i)
                };
                if obu_type == 1 {
                    // OBU_SEQUENCE_HEADER — this temporal unit is a keyframe
                    return true;
                }
                i = i.saturating_add(obu_size);
            }
            false
        }
        _ => {
            // H.264: IDR (type 5) or SPS (type 7)
            let mut i = 0;
            while i + 3 < data.len() {
                if data[i] == 0 && data[i + 1] == 0 {
                    let (sc_len, nal_off) =
                        if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                            (4, 4)
                        } else if data[i + 2] == 1 {
                            (3, 3)
                        } else {
                            i += 1;
                            continue;
                        };
                    if i + nal_off < data.len() {
                        let nal_type = data[i + nal_off] & 0x1f;
                        if nal_type == 5 || nal_type == 7 {
                            return true;
                        }
                    }
                    i += sc_len;
                } else {
                    i += 1;
                }
            }
            false
        }
    }
}

// ─── ffmpeg streaming ─────────────────────────────────────────────────────────

async fn stream_frames_ffmpeg(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    app: &AppHandle,
    width: usize,
    height: usize,
    config: &EncoderConfig,
) -> anyhow::Result<()> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    use tokio::process::Command as TokioCommand;

    let encoder = resolve_encoder(&config.codec, &config.backend);
    let (fmt, bsf) = codec_format(&config.codec);
    eprintln!(
        "[host] ffmpeg: encoder={} format={} bsf={} bitrate={} fps={} gop={}",
        encoder, fmt, bsf, config.bitrate, config.framerate, config.gop
    );

    let _ = app.emit("host-codec", &config.codec);

    let mut cmd = TokioCommand::new("ffmpeg");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if cfg!(target_os = "macos") {
        cmd.arg("-f").arg("avfoundation")
            .arg("-framerate").arg(config.framerate.to_string())
            .arg("-capture_cursor").arg("1")
            .arg("-i").arg("1:");
    } else if cfg!(target_os = "windows") {
        cmd.arg("-f").arg("gdigrab")
            .arg("-framerate").arg(config.framerate.to_string())
            .arg("-i").arg("desktop");
    } else {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        cmd.arg("-f").arg("x11grab")
            .arg("-framerate").arg(config.framerate.to_string())
            .arg("-video_size").arg(format!("{}x{}", width, height))
            .arg("-i").arg(&display);
    }

    match encoder.as_str() {
        e if e.ends_with("_nvenc") => {
            cmd.arg("-c:v").arg(&encoder)
                .arg("-preset").arg("p1")
                .arg("-tune").arg("ll")
                .arg("-rc").arg("cbr")
                .arg("-b:v").arg(&config.bitrate);
        }
        e if e.ends_with("_vaapi") => {
            cmd.arg("-c:v").arg(&encoder)
                .arg("-rc_mode").arg("CBR")
                .arg("-b:v").arg(&config.bitrate);
        }
        e if e.ends_with("_videotoolbox") => {
            cmd.arg("-c:v").arg(&encoder)
                .arg("-realtime").arg("1")
                .arg("-b:v").arg(&config.bitrate);
        }
        e if e.ends_with("_qsv") => {
            cmd.arg("-c:v").arg(&encoder)
                .arg("-preset").arg("veryfast")
                .arg("-b:v").arg(&config.bitrate);
        }
        e if e.ends_with("_amf") => {
            cmd.arg("-c:v").arg(&encoder)
                .arg("-usage").arg("ultralowlatency")
                .arg("-b:v").arg(&config.bitrate);
        }
        "libx264" => {
            cmd.arg("-c:v").arg("libx264")
                .arg("-preset").arg("ultrafast")
                .arg("-tune").arg("zerolatency")
                .arg("-b:v").arg(&config.bitrate);
        }
        "libx265" => {
            cmd.arg("-c:v").arg("libx265")
                .arg("-preset").arg("ultrafast")
                .arg("-tune").arg("zerolatency")
                .arg("-x265-params").arg("keyint=30:min-keyint=30")
                .arg("-b:v").arg(&config.bitrate);
        }
        "libsvtav1" => {
            cmd.arg("-c:v").arg("libsvtav1")
                .arg("-preset").arg("8")
                .arg("-crf").arg("35")
                .arg("-g").arg(config.gop.to_string());
        }
        "libaom-av1" => {
            cmd.arg("-c:v").arg("libaom-av1")
                .arg("-cpu-used").arg("8")
                .arg("-crf").arg("35")
                .arg("-b:v").arg(&config.bitrate);
        }
        _ => {
            return Err(anyhow::anyhow!("unsupported encoder: {}", encoder));
        }
    }

    if encoder != "libx265" && encoder != "libsvtav1" {
        cmd.arg("-g").arg(config.gop.to_string());
    }
    cmd.arg("-bf").arg("0")
        .arg("-pix_fmt").arg("yuv420p")
        .arg("-bsf:v").arg(bsf)
        .arg("-f").arg(fmt)
        .arg("-");

    let mut child = cmd.spawn().context("failed to spawn ffmpeg")?;
    let mut stdout = child.stdout.take().context("no stdout from ffmpeg")?;
    let mut stderr = child.stderr.take();
    let mut stderr_buf = String::new();

    let mut buf: Vec<u8> = Vec::with_capacity(65536);
    let mut frame_start: usize = 0;
    let mut first_delim_seen = false;
    let mut frame_count = 0u32;
    let start = Instant::now();
    let mut last_frame_time = Instant::now();

    loop {
        let mut tmp = [0u8; 16384];
        let n = stdout.read(&mut tmp).await?;
        if n == 0 {
            if let Some(ref mut stderr) = stderr {
                let _ = stderr.read_to_string(&mut stderr_buf).await;
            }
            eprintln!("[host] ffmpeg stdout closed");
            if !stderr_buf.is_empty() {
                eprintln!("[host] ffmpeg stderr: {}", stderr_buf);
            }
            break;
        }
        buf.extend_from_slice(&tmp[..n]);

        loop {
            let search_from = if first_delim_seen { frame_start + 6 } else { 0 };
            match find_next_frame_delim(&buf, search_from, &config.codec) {
                Some(delim_pos) => {
                    if first_delim_seen {
                        let frame_data = &buf[frame_start..delim_pos];
                        let is_keyframe = frame_is_keyframe(frame_data, &config.codec);
                        let frame_size = frame_data.len();

                        let now = Instant::now();
                        let frame_ms =
                            now.duration_since(last_frame_time).as_secs_f64() * 1000.0;
                        last_frame_time = now;

                        let header = [
                            (width as u32).to_be_bytes(),
                            (height as u32).to_be_bytes(),
                            (frame_size as u32).to_be_bytes(),
                        ]
                        .concat();
                        let kf_byte = if is_keyframe { 1u8 } else { 0u8 };
                        let codec_byte = codec_to_byte(&config.codec);

                        send.write_all(&header).await?;
                        send.write_all(&[kf_byte, codec_byte]).await?;
                        send.write_all(frame_data).await?;

                        frame_count += 1;
                        let elapsed = start.elapsed();
                        let fps = frame_count as f64 / elapsed.as_secs_f64().max(0.001);

                        let _ = app.emit(
                            "host-encode-stats",
                            serde_json::json!({
                                "frame": frame_count,
                                "encode_ms": (frame_ms * 10.0).round() / 10.0,
                                "capture_ms": 0.0,
                                "size_bytes": frame_size,
                                "fps": (fps * 10.0).round() / 10.0,
                                "keyframe": is_keyframe,
                                "encoder": encoder.clone(),
                            }),
                        );

                        eprintln!(
                            "[host] frame={} {}x{} {}={}B kf={} fps={:.1} ftime={:.1}ms",
                            frame_count, width, height, config.codec, frame_size,
                            is_keyframe, fps, frame_ms
                        );
                    }
                    frame_start = delim_pos;
                    first_delim_seen = true;
                }
                None => break,
            }
        }

        if frame_start > 0 && frame_start >= buf.len() / 2 {
            buf.drain(..frame_start);
            frame_start = 0;
        }

        match tokio::time::timeout(Duration::from_millis(1), recv.read(&mut [0u8; 1])).await {
            Ok(Ok(Some(_))) | Ok(Err(_)) => {
                eprintln!("[host] client disconnected");
                break;
            }
            _ => {}
        }
    }

    let _ = child.kill().await;
    Ok(())
}

// ─── xcap + openh264 fallback path ───────────────────────────────────────────

async fn stream_frames_xcap(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    app: &AppHandle,
) -> anyhow::Result<()> {
    let monitors = xcap::Monitor::all().context("failed to enumerate monitors")?;
    let monitor = monitors
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no monitors found"))?;

    let mut encoder = Encoder::new().context("failed to create H.264 encoder")?;
    let mut frame_count = 0u32;
    let start = Instant::now();

    loop {
        let capture_start = Instant::now();
        let image = monitor.capture_image()?;
        let capture_ms = capture_start.elapsed().as_secs_f64() * 1000.0;

        let rgb_image = image::DynamicImage::ImageRgba8(image).to_rgb8();
        let (w, h) = (rgb_image.width() as usize, rgb_image.height() as usize);

        let encode_start = Instant::now();
        let rgb_source = RgbSliceU8::new(rgb_image.as_raw(), (w, h));
        let yuv = YUVBuffer::from_rgb8_source(rgb_source);
        let (h264_data, is_keyframe) = {
            let bitstream = encoder.encode(&yuv).context("H.264 encode failed")?;
            let kf = matches!(
                bitstream.frame_type(),
                openh264::encoder::FrameType::I | openh264::encoder::FrameType::IDR
            );
            (bitstream.to_vec(), kf)
        };
        let encode_ms = encode_start.elapsed().as_secs_f64() * 1000.0;
        let h264_size = h264_data.len();

        let header = [
            (w as u32).to_be_bytes(),
            (h as u32).to_be_bytes(),
            (h264_size as u32).to_be_bytes(),
        ]
        .concat();
        let kf_byte = if is_keyframe { 1u8 } else { 0u8 };

        send.write_all(&header).await?;
        send.write_all(&[kf_byte, 0u8]).await?;
        send.write_all(&h264_data).await?;

        frame_count += 1;
        let elapsed = start.elapsed();
        let fps = frame_count as f64 / elapsed.as_secs_f64().max(0.001);
        eprintln!(
            "[host] frame={} {}x{} h264={}B kf={} fps={:.1} enc={:.1}ms cap={:.1}ms",
            frame_count, w, h, h264_size, is_keyframe, fps, encode_ms, capture_ms
        );

        let _ = app.emit(
            "host-encode-stats",
            serde_json::json!({
                "frame": frame_count,
                "encode_ms": (encode_ms * 10.0).round() / 10.0,
                "capture_ms": (capture_ms * 10.0).round() / 10.0,
                "size_bytes": h264_size,
                "fps": (fps * 10.0).round() / 10.0,
                "keyframe": is_keyframe,
            }),
        );

        match tokio::time::timeout(Duration::from_millis(1), recv.read(&mut [0u8; 1])).await {
            Ok(Ok(Some(_))) | Ok(Err(_)) => {
                eprintln!("[host] client disconnected");
                break;
            }
            _ => {}
        }

        tokio::task::yield_now().await;
    }

    Ok(())
}
