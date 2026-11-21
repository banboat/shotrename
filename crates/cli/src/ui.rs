use crate::apply::{apply_plan, undo_last};
use crate::locations;
use crate::planning::{build_plan, collect_files, load_template, parse_tz};
use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use shotrename_core::Plan;
use shotrename_meta::ProbeOptions;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::process::Command;
use tiny_http::{Header, Method, Request, Response, Server};

const DEFAULT_JOURNAL: &str = ".shotrename/journal.jsonl";
const UI_HTML: &str = include_str!("ui.html");

#[derive(Debug, Parser)]
pub struct UiArgs {
    /// Port to listen on. 0 picks a free port.
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Do not open the browser.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Debug, Deserialize)]
struct PlanRequest {
    path: String,
    template: String,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct ApplyRequest {
    plan: Plan,
    #[serde(default)]
    journal: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UndoRequest {
    #[serde(default)]
    journal: Option<String>,
}

type ApiResponse = Response<Cursor<Vec<u8>>>;
type ApiResult = std::result::Result<serde_json::Value, (u16, String)>;

pub fn run(args: UiArgs) -> Result<()> {
    let server = Server::http(("127.0.0.1", args.port))
        .map_err(|error| anyhow!("cannot listen on 127.0.0.1:{}: {error}", args.port))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|address| address.port())
        .unwrap_or(args.port);
    let token = random_token();
    let url = format!("http://127.0.0.1:{port}/?t={token}");
    println!("ShotRename UI: {url}");
    if !args.no_open {
        open_browser(&url);
    }

    for mut request in server.incoming_requests() {
        let response = route(&mut request, &token);
        let _ = request.respond(response);
    }
    Ok(())
}

fn route(request: &mut Request, token: &str) -> ApiResponse {
    if query_param(request.url(), "t").as_deref() != Some(token) {
        return text_response(403, "missing or invalid token");
    }
    let path = request.url().split('?').next().unwrap_or("/").to_string();
    match (request.method(), path.as_str()) {
        (Method::Get, "/") => Response::from_string(UI_HTML)
            .with_header(header("Content-Type", "text/html; charset=utf-8")),
        (Method::Get, "/api/state") => json_response(200, state()),
        (Method::Get, "/api/locations") => json_response(
            200,
            serde_json::to_value(locations::list()).unwrap_or_default(),
        ),
        (Method::Post, "/api/pick-folder") => match locations::pick_folder() {
            Ok(Some(path)) => json_response(200, json!({ "path": path.display().to_string() })),
            Ok(None) => json_response(200, json!({ "cancelled": true })),
            Err(error) => json_response(500, json!({ "error": format!("{error:#}") })),
        },
        (Method::Post, "/api/plan") => match read_json::<PlanRequest>(request) {
            Ok(body) => match plan_endpoint(body) {
                Ok(value) => json_response(200, value),
                Err((status, message)) => json_response(status, json!({ "error": message })),
            },
            Err(message) => json_response(400, json!({ "error": message })),
        },
        (Method::Post, "/api/apply") => match read_json::<ApplyRequest>(request) {
            Ok(body) => {
                let journal = journal_path(body.journal.as_deref());
                match apply_plan(&body.plan, &journal, false) {
                    Ok(report) => json_response(
                        200,
                        json!({ "moved": report.moved, "journal": journal.display().to_string() }),
                    ),
                    Err(error) => json_response(400, json!({ "error": format!("{error:#}") })),
                }
            }
            Err(message) => json_response(400, json!({ "error": message })),
        },
        (Method::Post, "/api/undo") => match read_json::<UndoRequest>(request) {
            Ok(body) => {
                let journal = journal_path(body.journal.as_deref());
                match undo_last(&journal, false) {
                    Ok(report) => json_response(
                        200,
                        json!({ "batch_id": report.batch_id, "restored": report.restored }),
                    ),
                    Err(error) => json_response(400, json!({ "error": format!("{error:#}") })),
                }
            }
            Err(message) => json_response(400, json!({ "error": message })),
        },
        _ => json_response(404, json!({ "error": "not found" })),
    }
}

fn state() -> serde_json::Value {
    let templates = ["dji", "insta360", "gopro"]
        .iter()
        .filter_map(|key| load_template(key).ok().map(|template| (*key, template)))
        .map(|(key, template)| {
            json!({
                "key": key,
                "name": template.name,
                "filename": template.filename,
                "directory": template.directory,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "templates": templates,
        "journal": DEFAULT_JOURNAL,
        "fields": ["date", "time", "camera", "lens", "location", "location_zh",
                   "country", "gps", "resolution", "fps", "duration", "codec",
                   "original", "family", "width", "height", "altitude", "seq"],
    })
}

fn plan_endpoint(body: PlanRequest) -> ApiResult {
    let path = expand(&body.path);
    let template = load_template(&body.template).map_err(|error| (400, format!("{error:#}")))?;
    let options = ProbeOptions {
        tz_offset_secs: body
            .tz
            .as_deref()
            .map(parse_tz)
            .transpose()
            .map_err(|error| (400, format!("{error:#}")))?,
    };
    let files = collect_files(&[path]).map_err(|error| (400, format!("{error:#}")))?;
    if files.is_empty() {
        return Err((400, "目录里没有支持的视频文件".to_string()));
    }
    let output = body.output.as_deref().map(expand);
    let plan = build_plan(&files, &template, &options, output.as_deref(), body.force)
        .map_err(|error| (500, format!("{error:#}")))?;
    Ok(json!({
        "plan": plan,
        "counts": {
            "ready": plan.ready_count(),
            "blocked": plan.blocked_count(),
            "unchanged": plan.unchanged_count(),
        },
        "journal": DEFAULT_JOURNAL,
    }))
}

fn read_json<T: for<'de> Deserialize<'de>>(
    request: &mut Request,
) -> std::result::Result<T, String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|error| format!("read request: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("invalid JSON: {error}"))
}

fn journal_path(value: Option<&str>) -> PathBuf {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_JOURNAL))
}

fn expand(value: &str) -> PathBuf {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

fn json_response(status: u16, value: serde_json::Value) -> ApiResponse {
    Response::from_string(value.to_string())
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
}

fn text_response(status: u16, body: &str) -> ApiResponse {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        if file.read_exact(&mut bytes).is_ok() {
            return hex(&bytes);
        }
    }
    format!(
        "{:x}{:x}",
        std::process::id(),
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    )
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "cmd";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    #[cfg(target_os = "windows")]
    let status = Command::new(program).args(["/C", "start", "", url]).spawn();
    #[cfg(not(target_os = "windows"))]
    let status = Command::new(program).arg(url).spawn();
    if let Err(error) = status {
        eprintln!("note: could not open the browser automatically: {error}");
    }
}
