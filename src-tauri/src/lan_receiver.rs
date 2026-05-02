use std::{
    io::Read,
    net::UdpSocket,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    imageops::FilterType as ResizeFilterType,
    ColorType, DynamicImage, GenericImageView, ImageEncoder,
};
use qrcode::{render::svg, QrCode};
use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use uuid::Uuid;

use crate::{
    clipboard::write_item_to_clipboard_with_profile,
    history::{build_captured_clipboard, history_item_to_dto, store_capture},
    models::{
        AppError, CapturedClipboard, LanReceiverSession, LanReceiverStateDto, LanReceiverStatus,
        SharedState, StoredClipboardItem, HISTORY_UPDATED_EVENT, LAN_RECEIVER_STATUS_EVENT,
    },
    paste_target::TargetProfile,
};

const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const UPLOAD_HARD_LIMIT: usize = 128 * 1024 * 1024;
const MAX_STORED_IMAGE_SIDE: u32 = 1600;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileClipboardPayload {
    kind: String,
    text: Option<String>,
    image_data: Option<String>,
}

fn system_time_ms(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn receiver_state_dto(session: Option<&LanReceiverSession>) -> LanReceiverStateDto {
    let Some(session) = session else {
        return LanReceiverStateDto {
            running: false,
            url: None,
            qr_svg: None,
            expires_at: None,
            last_status: None,
        };
    };

    LanReceiverStateDto {
        running: true,
        url: Some(session.url.clone()),
        qr_svg: Some(session.qr_svg.clone()),
        expires_at: system_time_ms(session.expires_at),
        last_status: session.last_status.clone(),
    }
}

fn local_lan_ip() -> Result<String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.connect(("8.8.8.8", 80))?;
    Ok(socket.local_addr()?.ip().to_string())
}

fn build_qr_svg(url: &str) -> Result<String> {
    let code = QrCode::new(url.as_bytes())?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#1d232d"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

pub(crate) fn get_state(state: &Arc<SharedState>) -> LanReceiverStateDto {
    cleanup_expired_session(state);
    let guard = state.lan_receiver.lock().unwrap();
    receiver_state_dto(guard.as_ref())
}

// 启动局域网接收会话，生成带随机令牌的手机访问地址和二维码。
pub(crate) fn start(
    app: AppHandle,
    state: Arc<SharedState>,
) -> Result<LanReceiverStateDto, AppError> {
    cleanup_expired_session(&state);
    if let Some(existing) = state.lan_receiver.lock().unwrap().as_ref() {
        return Ok(receiver_state_dto(Some(existing)));
    }

    let server = Server::http(("0.0.0.0", 0)).map_err(|error| anyhow::anyhow!("{error}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| anyhow::anyhow!("failed to resolve receiver port"))?;
    let ip = local_lan_ip().unwrap_or_else(|_| "127.0.0.1".into());
    let token = Uuid::new_v4().to_string();
    let url = format!("http://{ip}:{port}/?token={token}");
    let qr_svg = build_qr_svg(&url).map_err(anyhow::Error::from)?;
    let expires_at = SystemTime::now() + SESSION_TTL;
    let stop_requested = Arc::new(AtomicBool::new(false));

    {
        let mut guard = state.lan_receiver.lock().unwrap();
        *guard = Some(LanReceiverSession {
            url,
            qr_svg,
            expires_at,
            stop_requested: stop_requested.clone(),
            last_status: None,
        });
    }

    let server_app = app.clone();
    let server_state = state.clone();
    thread::spawn(move || run_server(server_app, server_state, server, token, stop_requested));

    let dto = get_state(&state);
    let _ = app.emit(LAN_RECEIVER_STATUS_EVENT, &dto);
    Ok(dto)
}

// 停止当前局域网接收会话，并让已生成的二维码立即失效。
pub(crate) fn stop(
    app: AppHandle,
    state: Arc<SharedState>,
) -> Result<LanReceiverStateDto, AppError> {
    if let Some(session) = state.lan_receiver.lock().unwrap().take() {
        session.stop_requested.store(true, Ordering::Relaxed);
    }
    let dto = receiver_state_dto(None);
    let _ = app.emit(LAN_RECEIVER_STATUS_EVENT, &dto);
    Ok(dto)
}

fn cleanup_expired_session(state: &Arc<SharedState>) {
    let mut guard = state.lan_receiver.lock().unwrap();
    let expired = guard
        .as_ref()
        .map(|session| session.expires_at <= SystemTime::now())
        .unwrap_or(false);
    if expired {
        if let Some(session) = guard.take() {
            session.stop_requested.store(true, Ordering::Relaxed);
        }
    }
}

fn run_server(
    app: AppHandle,
    state: Arc<SharedState>,
    server: Server,
    token: String,
    stop_requested: Arc<AtomicBool>,
) {
    while !stop_requested.load(Ordering::Relaxed) {
        cleanup_expired_session(&state);
        if state.lan_receiver.lock().unwrap().is_none() {
            break;
        }

        match server.recv_timeout(Duration::from_millis(120)) {
            Ok(Some(request)) => {
                let worker_app = app.clone();
                let worker_state = state.clone();
                let worker_token = token.clone();
                thread::spawn(move || {
                    handle_request(worker_app, worker_state, request, &worker_token);
                });
            }
            Ok(None) => {}
            Err(error) => {
                set_status(
                    &app,
                    &state,
                    LanReceiverStatus {
                        kind: "error".into(),
                        message: format!("listener_error: {error}"),
                        received_kind: None,
                    },
                );
                break;
            }
        }
    }
}

fn handle_request(app: AppHandle, state: Arc<SharedState>, request: Request, token: &str) {
    let response = route_request(app, state, request, token);
    let _ = response;
}

fn route_request(app: AppHandle, state: Arc<SharedState>, mut request: Request, token: &str) -> () {
    let (path, query) = split_target(request.url());
    if request.method() == &Method::Get && path == "/" {
        if !query_has_token(&query, token) {
            respond_text(request, 403, "invalid token");
            return;
        }
        let max_image_bytes = state.settings.lock().unwrap().max_image_bytes;
        respond_html(request, mobile_page(max_image_bytes));
        return;
    }

    if request.method() == &Method::Post && path == "/api/clipboard" {
        if !query_has_token(&query, token) {
            respond_json(request, 403, r#"{"ok":false,"message":"invalid_token"}"#);
            return;
        }

        let body = match read_tiny_request_body(&mut request, UPLOAD_HARD_LIMIT) {
            Ok(body) => body,
            Err(error) => {
                respond_error(request, &app, &state, error);
                return;
            }
        };
        match receive_payload(app.clone(), state.clone(), &body) {
            Ok(kind) => respond_json(
                request,
                200,
                &format!(r#"{{"ok":true,"kind":"{}"}}"#, escape_json(&kind)),
            ),
            Err(error) => respond_error(request, &app, &state, error),
        }
        return;
    }

    if request.method() == &Method::Post && path == "/api/clipboard/text" {
        if !query_has_token(&query, token) {
            respond_json(request, 403, r#"{"ok":false,"message":"invalid_token"}"#);
            return;
        }

        let body = match read_tiny_request_body(&mut request, UPLOAD_HARD_LIMIT) {
            Ok(body) => body,
            Err(error) => {
                respond_error(request, &app, &state, error);
                return;
            }
        };
        match receive_text_payload(app.clone(), state.clone(), &body) {
            Ok(kind) => respond_json(
                request,
                200,
                &format!(r#"{{"ok":true,"kind":"{}"}}"#, escape_json(&kind)),
            ),
            Err(error) => respond_error(request, &app, &state, error),
        }
        return;
    }

    if request.method() == &Method::Post && path == "/api/clipboard/image" {
        if !query_has_token(&query, token) {
            respond_json(request, 403, r#"{"ok":false,"message":"invalid_token"}"#);
            return;
        }

        let body = match read_tiny_request_body(&mut request, UPLOAD_HARD_LIMIT) {
            Ok(body) => body,
            Err(error) => {
                respond_error(request, &app, &state, error);
                return;
            }
        };
        if body.is_empty() {
            respond_error(request, &app, &state, anyhow::anyhow!("empty_payload"));
            return;
        }
        let max_image_bytes = state.settings.lock().unwrap().max_image_bytes;
        if body.len() > max_image_bytes {
            respond_error(request, &app, &state, anyhow::anyhow!("image_too_large"));
            return;
        }

        set_status(
            &app,
            &state,
            LanReceiverStatus {
                kind: "processing".into(),
                message: "processing_image".into(),
                received_kind: Some("image".into()),
            },
        );
        let worker_app = app.clone();
        let worker_state = state.clone();
        let image_bytes = body;
        thread::spawn(move || {
            if let Err(error) =
                receive_image_payload(worker_app.clone(), worker_state.clone(), &image_bytes)
            {
                set_status(
                    &worker_app,
                    &worker_state,
                    LanReceiverStatus {
                        kind: "error".into(),
                        message: error.to_string(),
                        received_kind: None,
                    },
                );
            }
        });

        respond_json(
            request,
            202,
            r#"{"ok":true,"kind":"image","status":"processing"}"#,
        );
        return;
    }

    respond_text(request, 404, "not found");
}

fn read_tiny_request_body(request: &mut Request, max_body: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take((max_body + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > max_body {
        anyhow::bail!("request body too large");
    }
    Ok(body)
}

fn receive_payload(app: AppHandle, state: Arc<SharedState>, body: &[u8]) -> Result<String> {
    let payload: MobileClipboardPayload = serde_json::from_slice(body)?;
    let text = payload.text.unwrap_or_default();
    let text = text.trim().to_string();
    let image_data = payload
        .image_data
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let has_text = !text.is_empty();
    let has_image = image_data.is_some();

    if has_text && has_image {
        anyhow::bail!("text_and_image_are_mutually_exclusive");
    }
    if !has_text && !has_image {
        anyhow::bail!("empty_payload");
    }

    let settings = state.settings.lock().unwrap().clone();
    let capture = if payload.kind == "text" && has_text {
        build_captured_clipboard(&settings, text, None, None, None, None, None)?
    } else if payload.kind == "image" && has_image {
        let image_data = image_data.unwrap();
        let raw = image_data
            .split_once(',')
            .map(|(_, value)| value)
            .unwrap_or(image_data.as_str());
        let bytes = BASE64.decode(raw)?;
        if bytes.len() > settings.max_image_bytes {
            anyhow::bail!("image_too_large");
        }
        let decoded = image::load_from_memory(&bytes)?;
        let (width, height) = decoded.dimensions();
        let png_bytes = encode_png(decoded)?;
        if png_bytes.len() > settings.max_image_bytes {
            anyhow::bail!("image_too_large");
        }
        build_captured_clipboard(
            &settings,
            String::new(),
            None,
            None,
            Some(png_bytes),
            Some(width),
            Some(height),
        )?
    } else {
        anyhow::bail!("invalid_payload_kind");
    }
    .context("unsupported_payload")?;

    let received_kind = capture_kind(&capture).to_string();
    let hash = capture_hash(&capture).to_string();
    let item = {
        let mut store = state.history_store.lock().unwrap();
        let mut history = state.history.lock().unwrap();
        store_capture(
            &mut store,
            &mut history,
            capture,
            Some(("Mobile".into(), None)),
            &settings,
        )?;
        history
            .iter()
            .find(|item| item.hash == hash)
            .cloned()
            .context("stored item not found")?
    };

    let _ = app.emit(HISTORY_UPDATED_EVENT, history_item_to_dto(&item));
    crate::capture::mark_clipboard_suppressed(&state, item.hash.clone());
    write_received_item_to_clipboard(&app, &item)?;
    set_status(
        &app,
        &state,
        LanReceiverStatus {
            kind: "success".into(),
            message: "received".into(),
            received_kind: Some(received_kind.clone()),
        },
    );
    Ok(received_kind)
}

fn receive_text_payload(app: AppHandle, state: Arc<SharedState>, body: &[u8]) -> Result<String> {
    let text = String::from_utf8(body.to_vec())?.trim().to_string();
    receive_capture(app, state, |settings| {
        build_captured_clipboard(settings, text, None, None, None, None, None)
    })
}

fn receive_image_payload(app: AppHandle, state: Arc<SharedState>, body: &[u8]) -> Result<String> {
    if body.is_empty() {
        anyhow::bail!("empty_payload");
    }

    receive_capture(app, state, |settings| {
        if body.len() > settings.max_image_bytes {
            anyhow::bail!("image_too_large");
        }
        let decoded = image::load_from_memory(body)?;
        let (width, height) = decoded.dimensions();
        let png_bytes = encode_png_for_storage(decoded, settings.max_image_bytes)?;
        let original_mime = detect_image_mime(body).map(ToString::to_string);
        if png_bytes.len() > settings.max_image_bytes {
            anyhow::bail!("image_too_large");
        }
        let image_hash = crate::storage::image_hash_from_png_bytes(&png_bytes)?;
        Ok(Some(CapturedClipboard::Image {
            hash: image_hash,
            preview: format!("Image {width}x{height}"),
            png_bytes,
            original_bytes: Some(body.to_vec()),
            original_mime,
            image_width: width,
            image_height: height,
        }))
    })
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn receive_capture<F>(app: AppHandle, state: Arc<SharedState>, build: F) -> Result<String>
where
    F: FnOnce(&crate::models::AppSettings) -> Result<Option<CapturedClipboard>>,
{
    let settings = state.settings.lock().unwrap().clone();
    let capture = build(&settings)?.context("unsupported_payload")?;
    store_and_write_capture(app, state, &settings, capture)
}

fn store_and_write_capture(
    app: AppHandle,
    state: Arc<SharedState>,
    settings: &crate::models::AppSettings,
    capture: CapturedClipboard,
) -> Result<String> {
    let received_kind = capture_kind(&capture).to_string();
    let hash = capture_hash(&capture).to_string();
    let item = {
        let mut store = state.history_store.lock().unwrap();
        let mut history = state.history.lock().unwrap();
        store_capture(
            &mut store,
            &mut history,
            capture,
            Some(("Mobile".into(), None)),
            settings,
        )?;
        history
            .iter()
            .find(|item| item.hash == hash)
            .cloned()
            .context("stored item not found")?
    };

    let _ = app.emit(HISTORY_UPDATED_EVENT, history_item_to_dto(&item));
    crate::capture::mark_clipboard_suppressed(&state, item.hash.clone());
    write_received_item_to_clipboard(&app, &item)?;
    set_status(
        &app,
        &state,
        LanReceiverStatus {
            kind: "success".into(),
            message: "received".into(),
            received_kind: Some(received_kind.clone()),
        },
    );
    Ok(received_kind)
}

fn write_received_item_to_clipboard(app: &AppHandle, item: &StoredClipboardItem) -> Result<()> {
    #[cfg(windows)]
    {
        if item.kind == "image" {
            let png_bytes = item.image_png.as_deref().context("image payload missing")?;
            crate::clipboard::write_image_to_clipboard(png_bytes)?;
            return Ok(());
        }
    }

    write_item_to_clipboard_with_profile(app, item, TargetProfile::Generic).map(|_| ())
}

fn encode_png(image: DynamicImage) -> Result<Vec<u8>> {
    encode_png_bytes(image)
}

fn encode_png_for_storage(image: DynamicImage, max_bytes: usize) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let longest_side = width.max(height);
    if longest_side <= MAX_STORED_IMAGE_SIDE {
        let png_bytes = encode_png_bytes(image.clone())?;
        if png_bytes.len() <= max_bytes {
            return Ok(png_bytes);
        }
    }

    let scale = MAX_STORED_IMAGE_SIDE as f32 / longest_side.max(1) as f32;
    let next_width = ((width as f32 * scale).round() as u32).max(1);
    let next_height = ((height as f32 * scale).round() as u32).max(1);
    let resized = image.resize(next_width, next_height, ResizeFilterType::Triangle);
    let resized_png = encode_png_bytes(resized)?;
    if resized_png.len() <= max_bytes {
        return Ok(resized_png);
    }

    anyhow::bail!("image_too_large_after_png_conversion")
}

fn encode_png_bytes(image: DynamicImage) -> Result<Vec<u8>> {
    let rgba = image.to_rgba8();
    let mut bytes = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::NoFilter);
    encoder.write_image(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        ColorType::Rgba8.into(),
    )?;
    Ok(bytes)
}

fn capture_kind(capture: &CapturedClipboard) -> &'static str {
    match capture {
        CapturedClipboard::Text { .. } => "text",
        CapturedClipboard::Link { .. } => "link",
        CapturedClipboard::Image { .. } => "image",
        CapturedClipboard::Mixed { .. } => "mixed",
    }
}

fn capture_hash(capture: &CapturedClipboard) -> &str {
    match capture {
        CapturedClipboard::Text { hash, .. }
        | CapturedClipboard::Link { hash, .. }
        | CapturedClipboard::Image { hash, .. }
        | CapturedClipboard::Mixed { hash, .. } => hash,
    }
}

fn set_status(app: &AppHandle, state: &Arc<SharedState>, status: LanReceiverStatus) {
    {
        let mut guard = state.lan_receiver.lock().unwrap();
        if let Some(session) = guard.as_mut() {
            session.last_status = Some(status);
        }
    }
    let dto = get_state(state);
    let _ = app.emit(LAN_RECEIVER_STATUS_EVENT, dto);
}

fn split_target(target: &str) -> (String, String) {
    target
        .split_once('?')
        .map(|(path, query)| (path.to_string(), query.to_string()))
        .unwrap_or_else(|| (target.to_string(), String::new()))
}

fn query_has_token(query: &str, expected: &str) -> bool {
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .any(|(key, value)| key == "token" && value == expected)
}

fn respond_html(request: Request, body: String) {
    respond(request, 200, "text/html; charset=utf-8", body);
}

fn respond_json(request: Request, status: u16, body: &str) {
    respond(
        request,
        status,
        "application/json; charset=utf-8",
        body.to_string(),
    );
}

fn respond_text(request: Request, status: u16, body: &str) {
    respond(
        request,
        status,
        "text/plain; charset=utf-8",
        body.to_string(),
    );
}

fn respond_error(
    request: Request,
    app: &AppHandle,
    state: &Arc<SharedState>,
    error: anyhow::Error,
) {
    set_status(
        app,
        state,
        LanReceiverStatus {
            kind: "error".into(),
            message: error.to_string(),
            received_kind: None,
        },
    );
    respond_json(
        request,
        400,
        &format!(
            r#"{{"ok":false,"message":"{}"}}"#,
            escape_json(&error.to_string())
        ),
    );
}

fn respond(request: Request, status: u16, content_type: &'static str, body: String) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes("Cache-Control", "no-store") {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn mobile_page(max_image_bytes: usize) -> String {
    r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Power Paste</title>
  <style>
    :root { color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { margin: 0; min-height: 100vh; background: #f4f0ea; color: #1d232d; }
    main { max-width: 560px; margin: 0 auto; padding: 28px 18px; }
    h1 { margin: 0 0 18px; font-size: 24px; }
    .panel { display: grid; gap: 16px; }
    .tabs { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    button, textarea, input { font: inherit; }
    button { min-height: 44px; border: 0; border-radius: 8px; background: #1f2937; color: white; font-weight: 700; }
    button.secondary { background: #e2ddd5; color: #1d232d; }
    button.active { background: #dd8648; color: #24160d; }
    button:disabled { opacity: .55; }
    textarea { width: 100%; min-height: 220px; box-sizing: border-box; border: 1px solid #cbc4b9; border-radius: 8px; padding: 12px; resize: vertical; background: white; color: #1d232d; }
    input[type="file"] { width: 100%; min-height: 44px; }
    .preview-grid { display: none; grid-template-columns: repeat(3, 1fr); gap: 8px; }
    .preview-grid img { width: 100%; aspect-ratio: 1; border-radius: 8px; object-fit: cover; background: white; }
    .progress { display: none; width: 100%; height: 8px; border-radius: 999px; overflow: hidden; background: #e2ddd5; }
    .progress-bar { width: 0%; height: 100%; background: #dd8648; transition: width 120ms ease; }
    .status { min-height: 24px; color: #5f6672; word-break: break-word; }
    @media (prefers-color-scheme: dark) {
      body { background: #12151a; color: #f4efe8; }
      textarea { background: #1b2028; color: #f4efe8; border-color: #333b48; }
      button.secondary { background: #2b323d; color: #f4efe8; }
      .progress { background: #2b323d; }
      .status { color: #b8b0a5; }
    }
  </style>
</head>
<body>
  <main>
    <h1>Power Paste</h1>
    <section class="panel">
      <div class="tabs">
        <button id="textTab" class="active" type="button">文本</button>
        <button id="imageTab" class="secondary" type="button">图片</button>
      </div>
      <textarea id="textInput" placeholder="输入或粘贴要发送到桌面剪贴板的文本"></textarea>
      <input id="imageInput" type="file" accept="image/*" multiple hidden />
      <div id="previewGrid" class="preview-grid"></div>
      <button id="sendButton" type="button">发送到桌面剪贴板</button>
      <div id="progress" class="progress" aria-hidden="true"><div id="progressBar" class="progress-bar"></div></div>
      <div id="status" class="status"></div>
    </section>
  </main>
  <script>
    const token = new URLSearchParams(location.search).get('token') || '';
    const textTab = document.getElementById('textTab');
    const imageTab = document.getElementById('imageTab');
    const textInput = document.getElementById('textInput');
    const imageInput = document.getElementById('imageInput');
    const previewGrid = document.getElementById('previewGrid');
    const sendButton = document.getElementById('sendButton');
    const progress = document.getElementById('progress');
    const progressBar = document.getElementById('progressBar');
    const statusEl = document.getElementById('status');
    const maxImageBytes = __MAX_IMAGE_BYTES__;
    let mode = 'text';
    let imageFiles = [];

    function setMode(next) {
      mode = next;
      textTab.className = next === 'text' ? 'active' : 'secondary';
      imageTab.className = next === 'image' ? 'active' : 'secondary';
      textInput.hidden = next !== 'text';
      imageInput.hidden = next !== 'image';
      previewGrid.style.display = next === 'image' && imageFiles.length ? 'grid' : 'none';
      statusEl.textContent = '';
      setProgress(0, false);
      updateSendState();
    }

    function setProgress(percent, visible = true) {
      progress.style.display = visible ? 'block' : 'none';
      progressBar.style.width = Math.max(0, Math.min(100, percent)) + '%';
    }

    function updateSendState() {
      const hasPayload = mode === 'text'
        ? textInput.value.trim().length > 0
        : imageFiles.length > 0;
      sendButton.disabled = !hasPayload;
    }

    function sendPayload(path, body, contentType) {
      return new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open('POST', path + '?token=' + encodeURIComponent(token));
        xhr.setRequestHeader('Content-Type', contentType);
        xhr.upload.onprogress = (event) => {
          if (!event.lengthComputable) {
            statusEl.textContent = '上传中...';
            return;
          }
          const percent = Math.round((event.loaded / event.total) * 100);
          setProgress(percent);
          statusEl.textContent = '上传中 ' + percent + '%';
        };
        xhr.onload = () => {
          setProgress(100);
          statusEl.textContent = mode === 'image' ? '上传完成，桌面端正在处理...' : '上传完成，正在写入桌面剪贴板...';
          let result = {};
          try {
            result = JSON.parse(xhr.responseText || '{}');
          } catch (error) {
            reject(new Error(xhr.responseText || '发送失败'));
            return;
          }
          if (xhr.status < 200 || xhr.status >= 300 || !result.ok) {
            reject(new Error(result.message || '发送失败'));
            return;
          }
          resolve(result);
        };
        xhr.onerror = () => reject(new Error('网络连接失败'));
        xhr.ontimeout = () => reject(new Error('发送超时'));
        xhr.timeout = 120000;
        xhr.send(body);
      });
    }

    textTab.addEventListener('click', () => setMode('text'));
    imageTab.addEventListener('click', () => setMode('image'));
    textInput.addEventListener('input', updateSendState);
    imageInput.addEventListener('change', () => {
      const files = Array.from(imageInput.files || []).slice(0, 9);
      imageFiles = [];
      previewGrid.innerHTML = '';
      previewGrid.style.display = 'none';
      updateSendState();
      if (!files.length) return;
      const oversized = files.find((file) => file.size > maxImageBytes);
      if (oversized) {
        statusEl.textContent = '图片超过桌面端大小限制（最大 ' + Math.floor(maxImageBytes / 1000000) + ' MB）';
        updateSendState();
        return;
      }
      imageFiles = files;
      for (const file of files) {
        const image = document.createElement('img');
        image.alt = '';
        image.src = URL.createObjectURL(file);
        previewGrid.appendChild(image);
      }
      previewGrid.style.display = 'grid';
      statusEl.textContent = files.length === 1 ? '图片已选择' : '已选择 ' + files.length + ' 张图片';
      setProgress(0, false);
      updateSendState();
    });

    sendButton.addEventListener('click', async () => {
      if (mode === 'text' && !textInput.value.trim()) {
        updateSendState();
        return;
      }
      if (mode === 'image' && !imageFiles.length) {
        updateSendState();
        return;
      }
      sendButton.disabled = true;
      setProgress(0);
      statusEl.textContent = mode === 'image' ? '准备上传图片...' : '发送中...';
      try {
        if (mode === 'text') {
          await sendPayload('/api/clipboard/text', textInput.value.trim(), 'text/plain; charset=utf-8');
          statusEl.textContent = '已发送到桌面剪贴板';
        } else {
          for (let index = 0; index < imageFiles.length; index += 1) {
            const file = imageFiles[index];
            statusEl.textContent = '正在上传第 ' + (index + 1) + '/' + imageFiles.length + ' 张...';
            await sendPayload('/api/clipboard/image', file, file.type || 'application/octet-stream');
          }
          statusEl.textContent = '上传完成，桌面端正在处理...';
        }
      } catch (error) {
        statusEl.textContent = error.message || String(error);
      } finally {
        window.setTimeout(() => setProgress(0, false), 800);
        updateSendState();
      }
    });
    updateSendState();
  </script>
</body>
</html>"#
        .replace("__MAX_IMAGE_BYTES__", &max_image_bytes.to_string())
}

#[cfg(test)]
mod tests {
    use super::{query_has_token, split_target};

    #[test]
    fn validates_token_query_pair() {
        assert!(query_has_token("token=abc&x=1", "abc"));
        assert!(!query_has_token("token=abc", "def"));
    }

    #[test]
    fn splits_path_and_query() {
        let (path, query) = split_target("/api/clipboard?token=abc");
        assert_eq!(path, "/api/clipboard");
        assert_eq!(query, "token=abc");
    }
}
