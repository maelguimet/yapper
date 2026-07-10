//! Local automation API for Yapper TTS.
//!
//! Owns a same-user HTTP/1.1 listener over a mode-0600 Unix socket and a
//! bounded command queue consumed by the egui thread. It must never expose a
//! TCP listener, spawn a second model, or log request text.

use super::YapperApp;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const SOCKET_NAME: &str = "tts-api.sock";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_TEXT_CHARS: usize = 10_000;
const QUEUE_CAPACITY: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

static COMMAND_RX: OnceLock<Mutex<Receiver<ApiCommand>>> = OnceLock::new();
static SOCKET_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApiCommand {
    Speak { text: String },
    Stop,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeakBody {
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
    content_type: Option<String>,
    content_length: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    reason: &'static str,
    body: String,
}

impl HttpResponse {
    fn json(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            body: body.to_owned(),
        }
    }

    fn bad_request(message: &str, code: &str) -> Self {
        let body = serde_json::json!({"error": message, "code": code}).to_string();
        Self::json(400, "Bad Request", &body)
    }

    fn unsupported_media_type(message: &str) -> Self {
        let body = serde_json::json!({"error": message, "code": "unsupported_media_type"}).to_string();
        Self::json(415, "Unsupported Media Type", &body)
    }
}

/// Start the process-lifetime API listener. Repeated calls return the same path.
pub(crate) fn start() -> Result<PathBuf> {
    if let Some(path) = SOCKET_PATH.get() {
        return Ok(path.clone());
    }

    let path = socket_path()?;
    prepare_socket_parent(&path)?;
    remove_stale_socket(&path)?;
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind Yapper TTS API socket {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", path.display()))?;
    listener
        .set_nonblocking(true)
        .context("set Yapper TTS API listener nonblocking")?;

    let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
    COMMAND_RX
        .set(Mutex::new(rx))
        .map_err(|_| anyhow::anyhow!("Yapper TTS API command receiver already initialized"))?;
    SOCKET_PATH
        .set(path.clone())
        .map_err(|_| anyhow::anyhow!("Yapper TTS API socket path already initialized"))?;

    thread::Builder::new()
        .name("yapper-tts-api".into())
        .spawn(move || listener_loop(listener, tx))
        .context("spawn Yapper TTS API listener")?;
    Ok(path)
}

fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::runtime_dir)
        .context("XDG_RUNTIME_DIR is unavailable; refusing an insecure temp socket")?;
    Ok(runtime.join("yapper").join(SOCKET_NAME))
}

fn prepare_socket_parent(path: &std::path::Path) -> Result<()> {
    let parent = path.parent().context("TTS API socket has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create TTS API runtime dir {}", parent.display()))?;
    let meta = fs::symlink_metadata(parent)
        .with_context(|| format!("stat TTS API runtime dir {}", parent.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        bail!(
            "TTS API runtime path is not a real directory: {}",
            parent.display()
        );
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 0700 {}", parent.display()))?;
    Ok(())
}

fn remove_stale_socket(path: &std::path::Path) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !meta.file_type().is_socket() {
        bail!(
            "refusing to replace non-socket TTS API path: {}",
            path.display()
        );
    }
    fs::remove_file(path).with_context(|| format!("remove stale TTS API socket {}", path.display()))
}

fn listener_loop(listener: UnixListener, tx: SyncSender<ApiCommand>) {
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                let response = match read_request(&mut stream) {
                    Ok(req) => route(req, &tx),
                    Err(error) => HttpResponse::bad_request(&error.to_string(), "bad_request"),
                };
                let _ = write_response(&mut stream, response);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                eprintln!("Yapper TTS API listener stopped: {error}");
                break;
            }
        }
    }
}

fn read_request(stream: &mut UnixStream) -> Result<HttpRequest> {
    let mut bytes = Vec::with_capacity(1024);
    let header_end = loop {
        if bytes.len() >= MAX_HEADER_BYTES {
            bail!("request headers exceed {MAX_HEADER_BYTES} bytes");
        }
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).context("read request")?;
        if count == 0 {
            bail!("connection closed before complete headers");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = find_header_end(&bytes) {
            break index;
        }
    };

    let header = std::str::from_utf8(&bytes[..header_end]).context("headers are not UTF-8")?;
    let mut lines = header.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("missing method")?.to_owned();
    let path = parts.next().context("missing path")?.to_owned();
    let version = parts.next().context("missing HTTP version")?;
    if parts.next().is_some() || version != "HTTP/1.1" {
        bail!("expected an HTTP/1.1 request line");
    }

    let mut content_length = 0_usize;
    let mut content_length_headers = 0_u8;
    let mut content_type: Option<String> = None;
    let mut transfer_encoding = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            bail!("malformed request header");
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length_headers += 1;
            if content_length_headers > 1 {
                bail!("duplicate Content-Length");
            }
            content_length = value.parse::<usize>().context("invalid Content-Length")?;
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_ascii_lowercase());
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        }
    }
    if transfer_encoding {
        bail!("Transfer-Encoding is not supported");
    }
    if content_length > MAX_BODY_BYTES {
        bail!("request body exceeds {MAX_BODY_BYTES} bytes");
    }

    let body_start = header_end + 4;
    while bytes.len().saturating_sub(body_start) < content_length {
        let remaining = content_length - bytes.len().saturating_sub(body_start);
        let mut chunk = vec![0_u8; remaining.min(4096)];
        let count = stream.read(&mut chunk).context("read request body")?;
        if count == 0 {
            bail!("connection closed before complete request body");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = bytes[body_start..body_start + content_length].to_vec();
    Ok(HttpRequest {
        method,
        path,
        body,
        content_type,
        content_length,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn sanitize_speak_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.trim().chars() {
        if matches!(c, '\n' | '\r' | '\t') {
            out.push(c);
            continue;
        }
        if c.is_control() {
            continue;
        }
        if matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}') {
            continue;
        }
        if ('\u{202a}'..='\u{202e}').contains(&c) || ('\u{2066}'..='\u{2069}').contains(&c) {
            continue;
        }
        out.push(c);
    }
    out
}

fn route(request: HttpRequest, tx: &SyncSender<ApiCommand>) -> HttpResponse {
    if request.method == "GET" && request.content_length > 0 {
        return HttpResponse::bad_request("unexpected request body", "bad_request");
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => HttpResponse::json(200, "OK", r#"{"status":"ok"}"#),
        ("POST", "/v1/speak") => {
            let ct = request
                .content_type
                .as_deref()
                .unwrap_or("");
            if !ct.starts_with("application/json") {
                return HttpResponse::unsupported_media_type(
                    "Content-Type must be application/json",
                );
            }
            if request.content_length == 0 {
                return HttpResponse::bad_request("Content-Length required", "bad_request");
            }
            let body: SpeakBody = match serde_json::from_slice(&request.body) {
                Ok(body) => body,
                Err(_) => {
                    return HttpResponse::bad_request(
                        "body must be JSON object with a text field",
                        "invalid_json",
                    )
                }
            };
            let text = sanitize_speak_text(&body.text);
            if text.is_empty() {
                return HttpResponse::bad_request("text must not be empty", "empty_text");
            }
            if text.chars().count() > MAX_TEXT_CHARS {
                return HttpResponse::bad_request("text exceeds 10000 characters", "text_too_long");
            }
            enqueue(tx, ApiCommand::Speak { text })
        }
        ("POST", "/v1/stop") => {
            if request.content_length > 0 {
                return HttpResponse::bad_request("unexpected request body", "bad_request");
            }
            enqueue(tx, ApiCommand::Stop)
        }
        _ => HttpResponse::json(
            404,
            "Not Found",
            r#"{"error":"not found","code":"not_found"}"#,
        ),
    }
}

fn enqueue(tx: &SyncSender<ApiCommand>, command: ApiCommand) -> HttpResponse {
    match tx.try_send(command) {
        Ok(()) => HttpResponse::json(202, "Accepted", r#"{"status":"accepted"}"#),
        Err(TrySendError::Full(_)) => HttpResponse::json(
            429,
            "Too Many Requests",
            r#"{"error":"command queue full"}"#,
        ),
        Err(TrySendError::Disconnected(_)) => HttpResponse::json(
            503,
            "Service Unavailable",
            r#"{"error":"Yapper command loop unavailable"}"#,
        ),
    }
}

fn write_response(stream: &mut UnixStream, response: HttpResponse) -> Result<()> {
    let body = format!("{}\n", response.body);
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        response.reason,
        body.len(),
        body
    )
    .context("write TTS API response")
}

fn drain_commands() -> Vec<ApiCommand> {
    let Some(rx) = COMMAND_RX.get() else {
        return Vec::new();
    };
    let Ok(rx) = rx.lock() else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(command) => commands.push(command),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    commands
}

impl YapperApp {
    /// Apply API commands on the UI thread so they reuse the normal pipeline.
    pub(crate) fn poll_tts_api(&mut self) {
        for command in drain_commands() {
            match command {
                ApiCommand::Speak { text } => {
                    self.tts_text = text.clone();
                    self.do_speak(&text);
                }
                ApiCommand::Stop => {
                    self.cancel_tts_pipeline();
                    self.status = "playback stopped by API".into();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str, body: &[u8]) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            body: body.to_vec(),
            content_type: if body.is_empty() {
                None
            } else {
                Some("application/json".into())
            },
            content_length: body.len(),
        }
    }

    #[test]
    fn health_is_ok_without_queueing() {
        let (tx, rx) = mpsc::sync_channel(1);
        let response = route(request("GET", "/health", b""), &tx);
        assert_eq!(response.status, 200);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn speak_queues_trimmed_text() {
        let (tx, rx) = mpsc::sync_channel(1);
        let response = route(
            request("POST", "/v1/speak", br#"{"text":"  hello  "}"#),
            &tx,
        );
        assert_eq!(response.status, 202);
        assert_eq!(
            rx.try_recv().unwrap(),
            ApiCommand::Speak {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn empty_and_oversized_text_are_rejected() {
        let (tx, rx) = mpsc::sync_channel(2);
        assert_eq!(
            route(request("POST", "/v1/speak", br#"{"text":"  "}"#), &tx).status,
            400
        );
        let body = serde_json::json!({"text": "x".repeat(MAX_TEXT_CHARS + 1)}).to_string();
        assert_eq!(
            route(request("POST", "/v1/speak", body.as_bytes()), &tx).status,
            400
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn rejects_unknown_json_fields() {
        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            route(
                request("POST", "/v1/speak", br#"{"text":"hi","tone":"calm"}"#),
                &tx
            )
            .status,
            400
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sanitizes_control_chars_in_speak_text() {
        let (tx, rx) = mpsc::sync_channel(1);
        let body = br#"{"text":"  hi\u0007there  "}"#;
        let response = route(request("POST", "/v1/speak", body), &tx);
        assert_eq!(response.status, 202);
        match rx.try_recv().unwrap() {
            ApiCommand::Speak { text } => assert_eq!(text, "hithere"),
            _ => panic!("expected speak"),
        }
    }

    #[test]
    fn stop_is_queued_and_queue_full_is_bounded() {
        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(route(request("POST", "/v1/stop", b""), &tx).status, 202);
        assert_eq!(route(request("POST", "/v1/stop", b""), &tx).status, 429);
        assert_eq!(rx.try_recv().unwrap(), ApiCommand::Stop);
    }

    #[test]
    fn unknown_route_is_404() {
        let (tx, _rx) = mpsc::sync_channel(1);
        assert_eq!(route(request("GET", "/nope", b""), &tx).status, 404);
    }

    #[test]
    fn finds_complete_http_headers() {
        assert_eq!(find_header_end(b"GET /health HTTP/1.1\r\n\r\n"), Some(20));
        assert_eq!(find_header_end(b"GET /health HTTP/1.1\r\n"), None);
    }
}
