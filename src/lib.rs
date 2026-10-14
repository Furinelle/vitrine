mod admin;
mod image_review;
mod ingest;
pub mod similar;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::{JsCast, JsValue};
use worker::*;

/// Cloudflare Worker entrypoint for Vitrine (Rust / workers-rs).
/// Read-path parity with the existing TypeScript Worker, plus Phase 2 ingest.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let result = match handle_request(req, env).await {
        Ok(res) => res,
        Err(err) => {
            console_error!("vitrine error: {err}");
            let response = if is_d1_daily_limit(&err.to_string()) {
                let mut response = json_response(
                    &json!({ "ok": false, "code": "database_daily_limit", "error": "数据库今日访问额度已用完，每日北京时间 8 点重置。请稍后手动重试。" }),
                    503,
                )?;
                response.headers_mut().set(
                    "Retry-After",
                    &seconds_until_d1_reset((js_sys::Date::now() / 1000.0) as u64).to_string(),
                )?;
                response
            } else {
                json_response(
                    &json!({ "ok": false, "error": "internal server error" }),
                    500,
                )?
            };
            with_cors(response)
        }
    };
    with_security_headers(result)
}

fn is_d1_daily_limit(message: &str) -> bool {
    message.contains("Your account has exceeded D1's free tier daily row read limit")
        || message.contains("Your account has exceeded D1's free tier daily row write limit")
}

fn seconds_until_d1_reset(now_seconds: u64) -> u64 {
    86_400 - now_seconds % 86_400
}

async fn handle_request(req: Request, env: Env) -> Result<Response> {
    let method = req.method();
    let url = req.url()?;
    let path = url.path().to_string();

    let rate_binding = if path == "/api/admin/login" && method == Method::Post {
        Some("LOGIN_RATE_LIMITER")
    } else if method == Method::Get
        && (path == "/api/works"
            || path.starts_with("/api/works/")
            || path == "/api/tags"
            || path == "/api/sources")
    {
        Some("PUBLIC_RATE_LIMITER")
    } else {
        None
    };
    if let Some(binding) = rate_binding {
        let ip = req
            .headers()
            .get("CF-Connecting-IP")?
            .unwrap_or_else(|| "127.0.0.1".into());
        if !env.rate_limiter(binding)?.limit(ip).await?.success {
            let mut response =
                json_response(&json!({"ok": false, "error": "too many requests"}), 429)?;
            response.headers_mut().set("Retry-After", "60")?;
            return Ok(with_cors(response));
        }
    }

    // Reject unauthenticated writes before reading their body. Handler-level
    // checks remain the authority for each route's cookie/Bearer contract.
    if admin::is_mutating_method(req.method()) {
        let rejection = if path.starts_with("/api/catalog/") {
            if path == "/api/catalog/image-review" {
                require_catalog_or_admin_cookie(&req, &env)?
            } else {
                require_catalog_auth(&req, &env)?
            }
        } else if path.starts_with("/api/admin/")
            && path != "/api/admin/login"
            && path != "/api/admin/logout"
        {
            admin::require_admin_cookie(&req, &env)?
        } else {
            None
        };
        if let Some(response) = rejection {
            return Ok(response);
        }
    }

    // Bound every JSON mutation, including handlers in the review modules.
    // Read the stream incrementally so chunked requests cannot bypass the limit.
    let req = if path.starts_with("/api/")
        && path != "/api/ingest"
        && admin::is_mutating_method(req.method())
    {
        let limit = if path == "/api/catalog/similar/result" {
            4 * 1024 * 1024
        } else if path == "/api/admin/login" {
            16 * 1024
        } else {
            1024 * 1024
        };
        match bounded_api_request(req, limit).await? {
            Ok(req) => req,
            Err(response) => return Ok(response),
        }
    } else {
        req
    };

    if method == Method::Options {
        return Ok(with_cors(Response::empty()?.with_status(204)));
    }

    if path == "/api/health" {
        return Ok(with_cors(json_response(
            &json!({ "ok": true, "service": "vitrine" }),
            200,
        )?));
    }

    if (path == "/admin" || path == "/admin/" || path.starts_with("/admin/"))
        && method == Method::Get
    {
        return admin::serve_page(req, env).await;
    }

    if path == "/api/admin/login" && method == Method::Post {
        return admin::handle_login(req, env).await;
    }
    if path == "/api/admin/logout" && method == Method::Post {
        return admin::handle_logout(req, env).await;
    }
    if path == "/api/admin/session" && method == Method::Get {
        return admin::handle_session(req, env).await;
    }
    if path == "/api/admin/works" && method == Method::Get {
        return admin::handle_list(req, env).await;
    }
    if path == "/api/admin/fingerprints/status" && method == Method::Get {
        if let Some(response) = admin::require_admin_cookie(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_status(req, env).await;
    }
    if path == "/api/admin/similar" && method == Method::Get {
        if let Some(response) = admin::require_admin_cookie(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_list(req, env).await;
    }
    if path == "/api/admin/similar/delete" && method == Method::Post {
        if let Some(response) = admin::require_admin_cookie(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_delete(req, env).await;
    }
    if path == "/api/admin/similar/skip" && method == Method::Post {
        if let Some(response) = admin::require_admin_cookie(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_skip(req, env).await;
    }
    if let Some(rest) = path.strip_prefix("/api/admin/works/") {
        if let Some(work_id) = rest.strip_suffix("/hide") {
            if method == Method::Post {
                return admin::handle_hide(req, env, work_id).await;
            }
        } else if let Some(work_id) = rest.strip_suffix("/restore") {
            if method == Method::Post {
                return admin::handle_restore(req, env, work_id).await;
            }
        } else if method == Method::Get {
            return admin::handle_detail(req, env, rest).await;
        } else if method == Method::Patch {
            return admin::handle_patch(req, env, rest).await;
        }
    }

    if path == "/api/ingest" && method == Method::Post {
        return ingest::handle_ingest(req, env).await;
    }

    if path == "/api/works" && method == Method::Get {
        return Ok(with_cors(handle_list_works(&url, &env).await?));
    }

    if let Some(work_id) = path.strip_prefix("/api/works/") {
        if method == Method::Get {
            return Ok(with_cors(handle_work_images(work_id, &env).await?));
        }
    }

    if path == "/api/catalog" && method == Method::Get {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_list_catalog(&url, &env).await?));
    }

    if matches!(
        path.as_str(),
        "/api/catalog/similar/snapshot"
            | "/api/catalog/similar/input"
            | "/api/catalog/similar/result"
    ) {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return match (path.as_str(), method) {
            ("/api/catalog/similar/snapshot", Method::Get) => {
                similar::handle_snapshot(req, env).await
            }
            ("/api/catalog/similar/input", Method::Get) => similar::handle_input(req, env).await,
            ("/api/catalog/similar/result", Method::Post) => {
                similar::handle_publish(req, env).await
            }
            _ => json_response(&json!({"ok":false,"error":"method not allowed"}), 405),
        };
    }

    if path == "/api/catalog/fingerprints" && method == Method::Put {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_put(req, env).await;
    }
    if path == "/api/catalog/clip" && method == Method::Put {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_put_clip(req, env).await;
    }
    if path == "/api/catalog/clip/missing" && method == Method::Get {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return similar::handle_clip_missing(req, env).await;
    }

    if path == "/api/catalog/image-review" && method == Method::Post {
        if let Some(response) = require_catalog_or_admin_cookie(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(image_review::handle(req, &env).await?));
    }

    if path == "/api/catalog/prune" && method == Method::Post {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_catalog_prune(req, &env).await?));
    }

    if path == "/api/catalog/prune-works" && method == Method::Post {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_catalog_work_prune(req, &env).await?));
    }

    if path == "/api/catalog/retract" && method == Method::Post {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_catalog_retract(req, &env).await?));
    }

    if path == "/api/catalog/prune-works/telegram-result" && method == Method::Post {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_catalog_telegram_result(req, &env).await?));
    }

    if path == "/api/catalog/publications" && method == Method::Put {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_catalog_publication(req, &env).await?));
    }

    if path == "/api/tags" && method == Method::Get {
        return Ok(with_cors(handle_list_tags(&url, &env).await?));
    }

    if path == "/api/sources" && method == Method::Get {
        return Ok(with_cors(handle_list_sources(&url, &env).await?));
    }

    if let Some(key) = path.strip_prefix("/media/") {
        if method == Method::Get || method == Method::Head {
            return handle_media(key, &req, &env).await;
        }
    }

    if path.starts_with("/api/") || path.starts_with("/media/") {
        return json_response(&json!({"ok": false, "error": "not found"}), 404);
    }

    // SPA / static assets binding (wrangler assets.directory = ./public)
    if let Ok(assets) = env.assets("ASSETS") {
        return assets.fetch_request(req).await;
    }

    Response::ok("vitrine online")
}

pub(crate) fn with_cors(res: Response) -> Response {
    let headers = res.headers().clone();
    let _ = headers.set("Access-Control-Allow-Origin", "*");
    let _ = headers.set("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS");
    let _ = headers.set("Access-Control-Allow-Headers", "Content-Type");
    res.with_headers(headers)
}

pub(crate) fn with_security_headers(res: Response) -> Result<Response> {
    let headers = res.headers().clone();
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set("Strict-Transport-Security", "max-age=31536000")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
    )?;
    headers.set("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")?;
    Ok(res.with_headers(headers))
}

pub(crate) fn request_length_error(
    length: Option<&str>,
    limit: u64,
) -> Option<(u16, &'static str)> {
    match length.map(str::parse::<u64>) {
        Some(Ok(length)) if length > limit => Some((413, "request body too large")),
        Some(Err(_)) => Some((400, "invalid content length")),
        _ => None,
    }
}

async fn bounded_api_request(
    req: Request,
    limit: usize,
) -> Result<std::result::Result<Request, Response>> {
    if let Some((status, error)) = request_length_error(
        req.headers().get("Content-Length")?.as_deref(),
        limit as u64,
    ) {
        return Ok(Err(json_response(
            &json!({"ok": false, "error": error}),
            status,
        )?));
    }
    let Some(stream) = req.inner().body() else {
        return Ok(Ok(req));
    };
    let reader = stream
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|error| Error::from(JsValue::from(error)))?;
    let mut bytes = Vec::new();
    loop {
        let result = match wasm_bindgen_futures::JsFuture::from(reader.read()).await {
            Ok(result) => result,
            Err(_) => {
                return Ok(Err(json_response(
                    &json!({"ok": false, "error": "invalid request body"}),
                    400,
                )?))
            }
        };
        if js_sys::Reflect::get(&result, &JsValue::from_str("done"))?.as_bool() == Some(true) {
            break;
        }
        let chunk =
            js_sys::Uint8Array::new(&js_sys::Reflect::get(&result, &JsValue::from_str("value"))?);
        if chunk.length() as usize > limit.saturating_sub(bytes.len()) {
            let _ = wasm_bindgen_futures::JsFuture::from(reader.cancel()).await;
            return Ok(Err(json_response(
                &json!({"ok": false, "error": "request body too large"}),
                413,
            )?));
        }
        bytes.extend_from_slice(&chunk.to_vec());
    }
    reader.release_lock();
    let mut init = RequestInit::new();
    init.with_method(req.method())
        .with_headers(req.headers().clone())
        .with_body(Some(js_sys::Uint8Array::from(bytes.as_slice()).into()));
    Ok(Ok(Request::new_with_init(req.url()?.as_str(), &init)?))
}

pub(crate) fn json_response(data: &Value, status: u16) -> Result<Response> {
    let res = Response::from_json(data)?.with_status(status);
    let headers = res.headers().clone();
    let _ = headers.set("Content-Type", "application/json; charset=utf-8");
    let _ = headers.set("Cache-Control", "no-store");
    Ok(res.with_headers(headers))
}

/// Normalize tag text the same way as the TypeScript Worker.
pub fn normalize_tag(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('#').trim();
    let collapsed: String = trimmed.split_whitespace().collect::<Vec<_>>().join("_");
    collapsed.chars().take(64).collect()
}

/// Parse and clamp `limit`/`offset` query params like the TypeScript Worker.
pub fn parse_limit_offset(url: &Url) -> (u32, u32) {
    let limit = url
        .query_pairs()
        .find(|(k, _)| k == "limit")
        .and_then(|(_, v)| v.parse::<i64>().ok())
        .unwrap_or(40);
    let offset = url
        .query_pairs()
        .find(|(k, _)| k == "offset")
        .and_then(|(_, v)| v.parse::<i64>().ok())
        .unwrap_or(0);
    let limit = limit.clamp(1, 100) as u32;
    let offset = offset.clamp(0, u32::MAX as i64) as u32;
    (limit, offset)
}

fn query_param(url: &Url, key: &str) -> String {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

/// Split GROUP_CONCAT tags the same way as the TypeScript mapping.
pub fn tags_from_group_concat(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build public cover URL path from an optional R2 key.
pub fn cover_url_from_key(cover_key: Option<&str>) -> Option<String> {
    cover_key.filter(|k| !k.is_empty()).map(|key| {
        format!(
            "/media/{}",
            key.split('/')
                .map(|part| urlencoding::encode(part))
                .collect::<Vec<_>>()
                .join("/")
        )
    })
}

#[derive(Debug, Serialize)]
struct WorkOut {
    id: String,
    source: String,
    source_id: String,
    source_url: String,
    title: String,
    author_name: String,
    author_url: String,
    is_r18: bool,
    page_count: i64,
    created_at: String,
    cover_url: Option<String>,
    // Lets the gallery reserve each cover's real shape before the image loads.
    cover_width: Option<i64>,
    cover_height: Option<i64>,
    tags: Vec<String>,
}

const WORK_SELECT: &str = r#"
    SELECT w.*,
      (SELECT i.r2_key FROM images i WHERE i.work_id = w.id ORDER BY i.page_index LIMIT 1) AS cover_key,
      cover.width AS cover_width, cover.height AS cover_height,
      (SELECT GROUP_CONCAT(wt.tag, ',') FROM work_tags wt WHERE wt.work_id = w.id) AS tags
    FROM works w
    LEFT JOIN image_fingerprints cover ON cover.r2_key =
      (SELECT i.r2_key FROM images i WHERE i.work_id = w.id ORDER BY i.page_index LIMIT 1)
"#;

fn work_cursor(value: &str) -> std::result::Result<Option<(String, String)>, ()> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 1024 {
        return Err(());
    }
    let (created_at, id): (String, String) = serde_json::from_str(value).map_err(|_| ())?;
    if created_at.is_empty()
        || created_at.len() > 40
        || created_at.chars().any(char::is_control)
        || validate_work_id_value(&id).is_err()
    {
        return Err(());
    }
    Ok(Some((created_at, id)))
}

fn like_pattern(value: &str) -> String {
    format!(
        "%{}%",
        value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// Upper bound on tags in one `tag=a,b,c` filter; keeps the statement well
/// under D1's 100 bound-parameter limit alongside the other filters.
const MAX_FILTER_TAGS: usize = 64;

/// Splits a `tag` query value into normalized, de-duplicated tags. Commas
/// separate tags, matching the `GROUP_CONCAT(wt.tag, ',')` output format.
pub fn tag_filter(raw: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in raw.split(',').map(normalize_tag) {
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// FROM clause for the works list. One tag keeps the original indexed join;
/// several tags match works carrying any of them, each work once.
fn works_from_clause(tag_count: usize) -> String {
    match tag_count {
        0 => "FROM works w".to_string(),
        1 => "FROM work_tags filter CROSS JOIN works w ON w.id=filter.work_id".to_string(),
        n => format!(
            "FROM (SELECT DISTINCT work_id FROM work_tags WHERE tag IN ({})) filter CROSS JOIN works w ON w.id=filter.work_id",
            vec!["?"; n].join(", ")
        ),
    }
}

async fn handle_list_works(url: &Url, env: &Env) -> Result<Response> {
    let tag = query_param(url, "tag");
    let tags = tag_filter(&tag);
    let source = query_param(url, "source").to_lowercase();
    let q = query_param(url, "q");
    let (limit, offset) = parse_limit_offset(url);
    if q.len() > 512 || tag.len() > 4096 || tags.len() > MAX_FILTER_TAGS || source.len() > 64 {
        return json_response(&json!({"ok": false, "error": "query too long"}), 400);
    }
    let Ok(cursor) = work_cursor(&query_param(url, "cursor")) else {
        return json_response(&json!({"ok": false, "error": "invalid cursor"}), 400);
    };
    let offset = if cursor.is_some() { 0 } else { offset };

    let mut where_parts: Vec<&str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if !source.is_empty() {
        where_parts.push("w.source = ?");
        binds.push(source);
    }
    if tags.len() == 1 {
        where_parts.push("filter.tag = ?");
        binds.push(tags[0].clone());
    }
    if !q.is_empty() {
        where_parts.push("(w.title LIKE ? ESCAPE '\\' OR w.author_name LIKE ? ESCAPE '\\' OR w.source_id LIKE ? ESCAPE '\\')");
        let like = like_pattern(&q);
        binds.push(like.clone());
        binds.push(like.clone());
        binds.push(like);
    }

    // Hide soft-deleted works when deleted_at is present (migration 0002).
    where_parts.push("(w.deleted_at IS NULL)");
    if let Some((created_at, id)) = cursor {
        where_parts.push("(w.created_at, w.id) < (?, ?)");
        binds.extend([created_at, id]);
    }

    let where_sql = format!("WHERE {}", where_parts.join(" AND "));
    let work_select = WORK_SELECT.replace("FROM works w", &works_from_clause(tags.len()));

    let sql = format!(
        r#"
        {work_select}
        {where_sql}
        ORDER BY w.created_at DESC, w.id DESC
        LIMIT ? OFFSET ?
        "#
    );

    // The multi-tag placeholders sit in the FROM clause, ahead of the WHERE binds.
    let from_binds: &[String] = if tags.len() > 1 { &tags } else { &[] };
    let mut values: Vec<JsValue> = from_binds
        .iter()
        .chain(binds.iter())
        .map(|s| JsValue::from_str(s))
        .collect();
    values.push(JsValue::from_f64(f64::from(limit)));
    values.push(JsValue::from_f64(f64::from(offset)));

    let db = env.d1("DB")?;
    let rows = db.prepare(&sql).bind(&values)?.all().await?;
    let results = rows.results::<WorkRow>()?;

    let works: Vec<WorkOut> = results.into_iter().map(public_work).collect();
    let next_cursor = works
        .last()
        .filter(|_| works.len() == limit as usize)
        .map(|work| json!([work.created_at, work.id]).to_string());

    json_response(
        &json!({
            "ok": true,
            "works": works,
            "limit": limit,
            "offset": offset,
            "next_cursor": next_cursor,
        }),
        200,
    )
}

fn public_work(row: WorkRow) -> WorkOut {
    WorkOut {
        id: row.id,
        source: row.source,
        source_id: row.source_id,
        source_url: row.source_url,
        title: row.title,
        author_name: row.author_name,
        author_url: row.author_url,
        is_r18: row.is_r18 != 0,
        page_count: row.page_count,
        created_at: row.created_at,
        cover_url: cover_url_from_key(row.cover_key.as_deref()),
        cover_width: row.cover_width,
        cover_height: row.cover_height,
        tags: tags_from_group_concat(row.tags.as_deref()),
    }
}

#[derive(Debug, serde::Deserialize)]
struct WorkRow {
    id: String,
    source: String,
    source_id: String,
    source_url: String,
    title: String,
    author_name: String,
    author_url: String,
    is_r18: i64,
    page_count: i64,
    created_at: String,
    cover_key: Option<String>,
    cover_width: Option<i64>,
    cover_height: Option<i64>,
    tags: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct PublicImage {
    r2_key: String,
    content_type: String,
    page_index: i64,
    byte_size: i64,
}

async fn handle_work_images(work_id: &str, env: &Env) -> Result<Response> {
    let Ok(work_id) = urlencoding::decode(work_id) else {
        return json_response(&json!({"ok":false,"error":"invalid work id"}), 400);
    };
    if validate_work_id_value(&work_id).is_err() {
        return json_response(&json!({"ok":false,"error":"invalid work id"}), 400);
    }
    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare(format!(
            "{WORK_SELECT} WHERE w.id=? AND w.deleted_at IS NULL"
        ))
        .bind(&[JsValue::from_str(&work_id)])?
        .first::<WorkRow>(None)
        .await?
    else {
        return json_response(&json!({"ok": false, "error": "work not found"}), 404);
    };
    let images = env.d1("DB")?.prepare(
        "SELECT i.r2_key,i.content_type,i.page_index,i.byte_size FROM images i JOIN works w ON w.id=i.work_id WHERE w.id=? AND w.deleted_at IS NULL ORDER BY i.page_index"
    ).bind(&[JsValue::from_str(&work_id)])?.all().await?.results::<PublicImage>()?;
    if images.is_empty() {
        return json_response(&json!({"ok":false,"error":"work not found"}), 404);
    }
    json_response(
        &json!({"ok":true,"work":public_work(work),"images":images}),
        200,
    )
}

#[derive(Debug, serde::Deserialize, Serialize)]
struct TagRow {
    name: String,
    cnt: i64,
}

#[derive(Debug, serde::Deserialize, Serialize)]
struct SourceRow {
    source: String,
    cnt: i64,
}

/// Tag list query: counts only work_tags attached to non-deleted works.
pub fn tags_list_sql() -> &'static str {
    r#"
            SELECT t.name, COUNT(w.id) AS cnt
            FROM tags t
            LEFT JOIN work_tags wt ON wt.tag = t.name
            LEFT JOIN works w ON w.id = wt.work_id AND w.deleted_at IS NULL
            GROUP BY t.name
            HAVING cnt > 0
            ORDER BY cnt DESC, t.name ASC
            "#
}

pub fn catalog_list_sql() -> &'static str {
    r#"
        SELECT i.work_id,w.source,w.source_id,w.source_url,w.title,
               i.page_index,i.r2_key,i.byte_size,i.content_type,i.sha256
        FROM images i
        JOIN works w ON w.id = i.work_id
        WHERE w.deleted_at IS NULL
        ORDER BY i.id
        LIMIT ? OFFSET ?
    "#
}

pub fn catalog_work_list_sql() -> &'static str {
    r#"
        SELECT i.work_id,w.source,w.source_id,w.source_url,w.title,
               i.page_index,i.r2_key,i.byte_size,i.content_type,i.sha256
        FROM images i
        JOIN works w ON w.id = i.work_id
        WHERE w.deleted_at IS NULL AND i.work_id=?
        ORDER BY i.page_index
        LIMIT ? OFFSET ?
    "#
}

fn catalog_cursor_sql(fingerprints: bool) -> String {
    let (metadata, join) = if fingerprints {
        (
            r#"CASE WHEN f.r2_key IS NOT NULL AND json_valid(f.regions_json) THEN json_object(
          'r2_key',f.r2_key,'solid_color',json(CASE WHEN f.solid_color=0 THEN 'false' ELSE 'true' END),
          'content_sha256',f.content_sha256,'strict_key',f.strict_key,
          'average_hash',f.average_hash,'difference_hash',f.difference_hash,
          'color_key',f.color_key,'detail_key',f.detail_key,'width',f.width,'height',f.height,
          'bytes',f.bytes,'format',f.format,'regions',json(f.regions_json)) END"#,
            "LEFT JOIN image_fingerprints f ON f.r2_key=i.r2_key",
        )
    } else {
        ("NULL", "")
    };
    format!(
        r#"
        SELECT i.id AS catalog_id,i.work_id,w.source,w.source_id,w.source_url,w.title,
               i.page_index,i.r2_key,i.byte_size,i.content_type,i.sha256,
               {metadata} AS fingerprint_json
        FROM images i CROSS JOIN works w ON w.id=i.work_id {join}
        WHERE i.id > ? AND w.deleted_at IS NULL ORDER BY i.id LIMIT ?
    "#
    )
}

fn fingerprint_digest(value: &Value) -> Result<String> {
    use sha2::Digest;
    let bytes = serde_json::to_vec(value).map_err(|error| Error::RustError(error.to_string()))?;
    Ok(hex::encode(sha2::Sha256::digest(bytes)))
}

#[derive(Debug, serde::Deserialize, Serialize)]
struct CatalogImageOut {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    catalog_id: String,
    #[serde(skip_serializing)]
    fingerprint_json: Option<String>,
    fingerprint_digest: Option<String>,
    work_id: String,
    source: String,
    source_id: String,
    source_url: String,
    title: String,
    page_index: i64,
    r2_key: String,
    byte_size: i64,
    content_type: String,
    sha256: String,
}

fn require_catalog_auth(req: &Request, env: &Env) -> Result<Option<Response>> {
    catalog_auth(req, env, false)
}

fn require_catalog_or_admin_cookie(req: &Request, env: &Env) -> Result<Option<Response>> {
    catalog_auth(req, env, true)
}

fn catalog_auth(req: &Request, env: &Env, allow_admin_cookie: bool) -> Result<Option<Response>> {
    let token = admin::env_ingest_token(env);
    match ingest::check_bearer_auth(req.headers().get("Authorization")?.as_deref(), &token) {
        ingest::AuthCheck::Ok => return Ok(None),
        ingest::AuthCheck::NotConfigured => {
            return Ok(Some(with_cors(json_response(
                &json!({ "ok": false, "error": "INGEST_TOKEN not configured" }),
                500,
            )?)));
        }
        ingest::AuthCheck::Unauthorized => {}
    }
    if allow_admin_cookie && admin::request_has_valid_cookie(req, &token)? {
        if admin::is_mutating_method(req.method()) && !admin::origin_matches_request(req)? {
            return Ok(Some(with_cors(json_response(
                &json!({ "ok": false, "error": "invalid origin" }),
                403,
            )?)));
        }
        return Ok(None);
    }
    Ok(Some(with_cors(json_response(
        &json!({ "ok": false, "error": "unauthorized" }),
        401,
    )?)))
}

async fn handle_list_catalog(url: &Url, env: &Env) -> Result<Response> {
    let (limit, offset) = parse_limit_offset(url);
    let work_id = query_param(url, "work_id");
    if !work_id.is_empty() && validate_work_id_value(&work_id).is_err() {
        return json_response(&json!({ "ok": false, "error": "invalid work id" }), 400);
    }
    let db = env.d1("DB")?;
    if url.query_pairs().any(|(key, _)| key == "after") {
        let after = query_param(url, "after");
        if !work_id.is_empty()
            || offset != 0
            || after.len() > 2048
            || after.chars().any(char::is_control)
        {
            return json_response(
                &json!({"ok": false, "error": "invalid catalog cursor"}),
                400,
            );
        }
        let fingerprints = query_param(url, "fingerprints") == "1";
        let mut images = db
            .prepare(catalog_cursor_sql(fingerprints))
            .bind(&[
                JsValue::from_str(&after),
                JsValue::from_f64(f64::from(limit)),
            ])?
            .all()
            .await?
            .results::<CatalogImageOut>()?;
        for image in &mut images {
            image.fingerprint_digest = match image.fingerprint_json.take() {
                Some(raw) => Some(fingerprint_digest(
                    &serde_json::from_str::<Value>(&raw)
                        .map_err(|error| Error::RustError(error.to_string()))?,
                )?),
                None => None,
            };
        }
        let next_cursor = (images.len() == limit as usize)
            .then(|| images.last().map(|image| image.catalog_id.clone()))
            .flatten();
        return json_response(
            &json!({"ok": true, "images": images, "next_cursor": next_cursor, "limit": limit}),
            200,
        );
    }
    let rows = if work_id.is_empty() {
        let values = [
            JsValue::from_f64(f64::from(limit)),
            JsValue::from_f64(f64::from(offset)),
        ];
        db.prepare(catalog_list_sql())
            .bind(&values)?
            .all()
            .await?
            .results::<CatalogImageOut>()?
    } else {
        let values = [
            JsValue::from_str(&work_id),
            JsValue::from_f64(f64::from(limit)),
            JsValue::from_f64(f64::from(offset)),
        ];
        db.prepare(catalog_work_list_sql())
            .bind(&values)?
            .all()
            .await?
            .results::<CatalogImageOut>()?
    };
    json_response(
        &json!({
            "ok": true,
            "images": rows,
            "limit": limit,
            "offset": offset,
        }),
        200,
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogPruneRequest {
    decision_id: String,
    keep_r2_key: String,
    remove_r2_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogPruneImageRow {
    id: String,
    work_id: String,
    r2_key: String,
}

#[derive(Debug, Deserialize)]
struct CatalogPruneReceiptRow {
    keep_r2_key: String,
    removed_json: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogPublicationRequest {
    work_id: String,
    chat_id: i64,
    message_ids: Vec<i64>,
    publish_state: String,
}

#[derive(Debug, Deserialize)]
struct CatalogPublicationRow {
    id: String,
    work_id: String,
    chat_id: i64,
    message_ids_json: String,
    publish_state: String,
    deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActiveWorkRow {
    id: String,
}

#[derive(Deserialize)]
struct WorkVersionRow {
    review_version: i64,
}

#[derive(Debug, Deserialize)]
struct RetractWorkRow {
    id: String,
    deleted_at: Option<String>,
    review_version: i64,
    image_count: i64,
}

#[derive(Debug, Deserialize)]
struct RetractBackupRow {
    original_r2_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogWorkPruneRequest {
    decision_id: String,
    keep_work_id: String,
    remove_work_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogRetractRequest {
    decision_id: String,
    work_id: String,
}

#[derive(Debug, Deserialize)]
struct CatalogWorkPruneReceiptRow {
    keep_work_id: String,
    remove_work_ids_json: String,
    removed_r2_keys_json: String,
    telegram_targets_json: String,
}

struct ParsedWorkPruneReceipt {
    remove_work_ids: Vec<String>,
    removed_r2_keys: Vec<String>,
    telegram_targets: Vec<TelegramPruneTarget>,
}

#[derive(Debug, Deserialize)]
struct WorkPublicationRow {
    id: String,
    work_id: String,
    chat_id: i64,
    message_ids_json: String,
    publish_state: String,
}

#[derive(Debug, Deserialize)]
struct WorkTagRow {
    tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramPruneTarget {
    publication_id: String,
    work_id: String,
    chat_id: i64,
    message_ids: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogTelegramResult {
    decision_id: String,
    complete: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramResultReceiptRow {
    telegram_state: String,
    remove_work_ids_json: String,
    telegram_targets_json: String,
}

fn validate_catalog_publication(
    request: &CatalogPublicationRequest,
) -> std::result::Result<(), &'static str> {
    if request.work_id.trim().is_empty() || request.work_id.len() > 320 {
        return Err("invalid work id");
    }
    let Some((source, source_id)) = request.work_id.split_once(':') else {
        return Err("invalid work id");
    };
    if !ingest::is_safe_source(source) || !ingest::is_safe_source_id(source_id) {
        return Err("invalid work id");
    }
    if request.chat_id == 0 {
        return Err("invalid chat id");
    }
    if request.message_ids.is_empty() || request.message_ids.len() > 40 {
        return Err("message ids must contain 1 to 40 entries");
    }
    let mut unique = std::collections::HashSet::new();
    for id in &request.message_ids {
        if *id <= 0 {
            return Err("invalid message id");
        }
        if !unique.insert(*id) {
            return Err("duplicate message id");
        }
    }
    if request.publish_state != "full" && request.publish_state != "partial" {
        return Err("invalid publish state");
    }
    Ok(())
}

fn same_catalog_publication(
    stored: &CatalogPublicationRow,
    request: &CatalogPublicationRequest,
) -> bool {
    let Ok(stored_ids) = serde_json::from_str::<Vec<i64>>(&stored.message_ids_json) else {
        return false;
    };
    stored.work_id == request.work_id
        && stored.chat_id == request.chat_id
        && stored_ids == request.message_ids
        && stored.publish_state == request.publish_state
}

async fn handle_catalog_publication(mut req: Request, env: &Env) -> Result<Response> {
    let request: CatalogPublicationRequest = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({ "ok": false, "error": "invalid json" }), 400),
    };
    if let Err(message) = validate_catalog_publication(&request) {
        return json_response(&json!({ "ok": false, "error": message }), 400);
    }

    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare("SELECT id FROM works WHERE id=? AND deleted_at IS NULL")
        .bind(&[JsValue::from_str(&request.work_id)])?
        .first::<ActiveWorkRow>(None)
        .await?
    else {
        return json_response(&json!({ "ok": false, "error": "work not active" }), 409);
    };
    if work.id != request.work_id {
        return json_response(&json!({ "ok": false, "error": "work not active" }), 409);
    }

    let anchor = request.message_ids[0];
    let publication_id = ingest::telegram_publication_id(&request.work_id, request.chat_id, anchor);
    if let Some(stored) = db
        .prepare(
            r#"SELECT id, work_id, chat_id, message_ids_json, publish_state, deleted_at
               FROM telegram_publications WHERE (work_id=? AND chat_id=? AND anchor_message_id=?) OR id=?
               ORDER BY (anchor_message_id=?) DESC LIMIT 1"#,
        )
        .bind(&[JsValue::from_str(&request.work_id), JsValue::from_f64(request.chat_id as f64), JsValue::from_f64(anchor as f64), JsValue::from_str(&publication_id), JsValue::from_f64(anchor as f64)])?
        .first::<CatalogPublicationRow>(None)
        .await?
    {
        if stored.deleted_at.is_none() && same_catalog_publication(&stored, &request) {
            return json_response(
                &json!({
                    "ok": true,
                    "publication_id": stored.id,
                    "idempotent": true,
                }),
                200,
            );
        }
        return json_response(
            &json!({ "ok": false, "error": "publication conflict" }),
            409,
        );
    }

    let now = js_iso_now();
    let message_ids_json = serde_json::to_string(&request.message_ids)
        .map_err(|error| Error::RustError(error.to_string()))?;
    let inserted = db.prepare(
        r#"INSERT INTO telegram_publications
           (id, work_id, chat_id, anchor_message_id, message_ids_json, publish_state, created_at, deleted_at)
           SELECT ?, ?, ?, ?, ?, ?, ?, NULL
           WHERE EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL)
             AND (? <> 'full' OR (SELECT COUNT(*) FROM images WHERE work_id=?)=?)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&[
        JsValue::from_str(&publication_id),
        JsValue::from_str(&request.work_id),
        JsValue::from_f64(request.chat_id as f64),
        JsValue::from_f64(anchor as f64),
        JsValue::from_str(&message_ids_json),
        JsValue::from_str(&request.publish_state),
        JsValue::from_str(&now),
        JsValue::from_str(&request.work_id),
        JsValue::from_str(&request.publish_state),
        JsValue::from_str(&request.work_id),
        JsValue::from_f64(request.message_ids.len() as f64),
    ])?
    .run()
    .await?;

    if inserted.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        return json_response(
            &json!({"ok":false,"error":"publication or work changed"}),
            409,
        );
    }

    json_response(
        &json!({
            "ok": true,
            "publication_id": publication_id,
            "idempotent": false,
        }),
        200,
    )
}

fn validate_catalog_prune(request: &CatalogPruneRequest) -> std::result::Result<(), &'static str> {
    if request.decision_id.trim().is_empty() || request.decision_id.len() > 160 {
        return Err("invalid decision id");
    }
    if request.keep_r2_key.trim().is_empty() || request.keep_r2_key.len() > 1024 {
        return Err("invalid keep key");
    }
    if request.remove_r2_keys.is_empty() || request.remove_r2_keys.len() > 20 {
        return Err("remove keys must contain 1 to 20 entries");
    }
    let mut unique = std::collections::HashSet::new();
    for key in &request.remove_r2_keys {
        if key.trim().is_empty() || key.len() > 1024 {
            return Err("invalid remove key");
        }
        if key == &request.keep_r2_key {
            return Err("keep key cannot be removed");
        }
        if !unique.insert(key) {
            return Err("duplicate remove key");
        }
    }
    Ok(())
}

pub(crate) fn js_iso_now() -> String {
    js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into())
}

fn catalog_backup_key(decision_id: &str, original: &str) -> String {
    let safe_decision: String = decision_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("review-trash/{safe_decision}/{original}")
}

async fn copy_catalog_object(bucket: &Bucket, original: &str, backup: &str) -> Result<()> {
    let object = bucket
        .get(original)
        .execute()
        .await?
        .ok_or_else(|| Error::RustError(format!("R2 object not found: {original}")))?;
    let http_metadata = object.http_metadata();
    let custom_metadata = object.custom_metadata()?;
    let body = object
        .body()
        .ok_or_else(|| Error::RustError(format!("R2 object has no body: {original}")))?;
    let ResponseBody::Stream(stream) = body.response_body()? else {
        return Err(Error::RustError("unexpected R2 response body".into()));
    };
    bucket
        .put(backup, stream)
        .http_metadata(http_metadata)
        .custom_metadata(custom_metadata)
        .execute()
        .await?;
    Ok(())
}

async fn handle_catalog_prune(mut req: Request, env: &Env) -> Result<Response> {
    let request: CatalogPruneRequest = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({ "ok": false, "error": "invalid json" }), 400),
    };
    if let Err(message) = validate_catalog_prune(&request) {
        return json_response(&json!({ "ok": false, "error": message }), 400);
    }

    let db = env.d1("DB")?;
    if let Some(receipt) = db
        .prepare(
            "SELECT keep_r2_key, removed_json FROM catalog_prune_receipts WHERE decision_id = ?",
        )
        .bind(&[JsValue::from_str(&request.decision_id)])?
        .first::<CatalogPruneReceiptRow>(None)
        .await?
    {
        let removed: Vec<String> = match serde_json::from_str(&receipt.removed_json) {
            Ok(removed) => removed,
            Err(_) => {
                return json_response(
                    &json!({ "ok": false, "error": "invalid prune receipt" }),
                    500,
                );
            }
        };
        if !same_prune_plan(
            &request.keep_r2_key,
            &request.remove_r2_keys,
            &receipt.keep_r2_key,
            &removed,
        ) {
            return json_response(
                &json!({ "ok": false, "error": "prune decision conflict" }),
                409,
            );
        }
        env.bucket("MEDIA")?
            .delete_multiple(removed.clone())
            .await?;
        return json_response(
            &json!({ "ok": true, "removed": removed.len(), "replayed": true }),
            200,
        );
    }

    let active_sql = r#"
        SELECT i.id,i.work_id,i.r2_key
        FROM images i JOIN works w ON w.id=i.work_id
        WHERE i.r2_key=? AND w.deleted_at IS NULL
    "#;
    if db
        .prepare(active_sql)
        .bind(&[JsValue::from_str(&request.keep_r2_key)])?
        .first::<CatalogPruneImageRow>(None)
        .await?
        .is_none()
    {
        return json_response(&json!({ "ok": false, "error": "keep key not active" }), 409);
    }

    let mut rows = Vec::with_capacity(request.remove_r2_keys.len());
    for key in &request.remove_r2_keys {
        let Some(row) = db
            .prepare(active_sql)
            .bind(&[JsValue::from_str(key)])?
            .first::<CatalogPruneImageRow>(None)
            .await?
        else {
            return json_response(
                &json!({ "ok": false, "error": format!("remove key not active: {key}") }),
                409,
            );
        };
        rows.push(row);
    }

    let bucket = env.bucket("MEDIA")?;
    let backup_keys: Vec<String> = rows
        .iter()
        .map(|row| catalog_backup_key(&request.decision_id, &row.r2_key))
        .collect();
    for (row, backup) in rows.iter().zip(&backup_keys) {
        copy_catalog_object(&bucket, &row.r2_key, backup).await?;
    }

    let now = js_iso_now();
    let removed_json = serde_json::to_string(&request.remove_r2_keys)
        .map_err(|error| Error::RustError(error.to_string()))?;
    let mut statements = Vec::new();
    let mut work_ids = std::collections::HashSet::new();
    for (row, backup) in rows.iter().zip(&backup_keys) {
        work_ids.insert(row.work_id.clone());
        statements.push(
            db.prepare(
                r#"INSERT INTO catalog_prune_backups
                   (original_r2_key,backup_r2_key,decision_id,work_id,created_at)
                   VALUES (?,?,?,?,?)"#,
            )
            .bind(&[
                JsValue::from_str(&row.r2_key),
                JsValue::from_str(backup),
                JsValue::from_str(&request.decision_id),
                JsValue::from_str(&row.work_id),
                JsValue::from_str(&now),
            ])?,
        );
        statements.push(
            db.prepare("DELETE FROM images WHERE id=? AND r2_key=?")
                .bind(&[JsValue::from_str(&row.id), JsValue::from_str(&row.r2_key)])?,
        );
    }
    for work_id in work_ids {
        statements.push(
            db.prepare("UPDATE works SET page_count=(SELECT COUNT(*) FROM images WHERE work_id=?),review_version=review_version+1 WHERE id=?")
                .bind(&[JsValue::from_str(&work_id), JsValue::from_str(&work_id)])?,
        );
        statements.push(
            db.prepare("UPDATE works SET deleted_at=? WHERE id=? AND NOT EXISTS (SELECT 1 FROM images WHERE work_id=?)")
                .bind(&[
                    JsValue::from_str(&now),
                    JsValue::from_str(&work_id),
                    JsValue::from_str(&work_id),
                ])?,
        );
    }
    statements.push(
        db.prepare("INSERT INTO catalog_prune_receipts (decision_id,keep_r2_key,removed_json,created_at) VALUES (?,?,?,?)")
            .bind(&[
                JsValue::from_str(&request.decision_id),
                JsValue::from_str(&request.keep_r2_key),
                JsValue::from_str(&removed_json),
                JsValue::from_str(&now),
            ])?,
    );
    db.batch(statements).await?;
    bucket
        .delete_multiple(request.remove_r2_keys.clone())
        .await?;

    json_response(
        &json!({
            "ok": true,
            "removed": request.remove_r2_keys.len(),
            "replayed": false,
        }),
        200,
    )
}

pub(crate) fn validate_work_id_value(work_id: &str) -> std::result::Result<(), &'static str> {
    if work_id.trim().is_empty() || work_id.len() > 320 {
        return Err("invalid work id");
    }
    let Some((source, source_id)) = work_id.split_once(':') else {
        return Err("invalid work id");
    };
    if !ingest::is_safe_source(source) || !ingest::is_safe_source_id(source_id) {
        return Err("invalid work id");
    }
    Ok(())
}

fn validate_catalog_work_prune(
    request: &CatalogWorkPruneRequest,
) -> std::result::Result<(), &'static str> {
    if request.decision_id.trim().is_empty() || request.decision_id.len() > 160 {
        return Err("invalid decision id");
    }
    validate_work_id_value(&request.keep_work_id)?;
    if request.remove_work_ids.is_empty() || request.remove_work_ids.len() > 20 {
        return Err("remove works must contain 1 to 20 entries");
    }
    let mut unique = std::collections::HashSet::new();
    for work_id in &request.remove_work_ids {
        validate_work_id_value(work_id)?;
        if work_id == &request.keep_work_id {
            return Err("keep work cannot be removed");
        }
        if !unique.insert(work_id) {
            return Err("duplicate remove work id");
        }
    }
    Ok(())
}

fn same_prune_plan(
    keep: &str,
    remove: &[String],
    stored_keep: &str,
    stored_remove: &[String],
) -> bool {
    if keep != stored_keep {
        return false;
    }
    let mut left = remove.to_vec();
    let mut right = stored_remove.to_vec();
    left.sort();
    right.sort();
    left == right
}

fn parse_publication_message_ids(raw: &str) -> std::result::Result<Vec<i64>, &'static str> {
    let ids: Vec<i64> =
        serde_json::from_str(raw).map_err(|_| "invalid telegram publication mapping")?;
    if ids.is_empty() || ids.len() > 40 {
        return Err("invalid telegram publication mapping");
    }
    let mut unique = std::collections::HashSet::new();
    for id in &ids {
        if *id <= 0 || !unique.insert(*id) {
            return Err("invalid telegram publication mapping");
        }
    }
    Ok(ids)
}

fn parse_work_prune_receipt(
    receipt: &CatalogWorkPruneReceiptRow,
) -> std::result::Result<ParsedWorkPruneReceipt, &'static str> {
    let remove_work_ids: Vec<String> =
        serde_json::from_str(&receipt.remove_work_ids_json).map_err(|_| "corrupt prune receipt")?;
    if remove_work_ids.is_empty() || remove_work_ids.len() > 20 {
        return Err("corrupt prune receipt");
    }
    let mut unique_work_ids = std::collections::HashSet::new();
    if remove_work_ids
        .iter()
        .any(|work_id| validate_work_id_value(work_id).is_err() || !unique_work_ids.insert(work_id))
    {
        return Err("corrupt prune receipt");
    }

    let removed_r2_keys: Vec<String> =
        serde_json::from_str(&receipt.removed_r2_keys_json).map_err(|_| "corrupt prune receipt")?;
    let telegram_targets: Vec<TelegramPruneTarget> =
        serde_json::from_str(&receipt.telegram_targets_json)
            .map_err(|_| "corrupt prune receipt")?;
    validate_stored_telegram_targets(&telegram_targets, &remove_work_ids)?;
    Ok(ParsedWorkPruneReceipt {
        remove_work_ids,
        removed_r2_keys,
        telegram_targets,
    })
}

fn validate_stored_telegram_targets(
    targets: &[TelegramPruneTarget],
    remove_work_ids: &[String],
) -> std::result::Result<(), &'static str> {
    if targets.is_empty() {
        return Err("corrupt prune receipt");
    }
    let mut covered_work_ids = std::collections::HashSet::new();
    let mut publication_ids = std::collections::HashSet::new();
    for target in targets {
        if target.publication_id.trim().is_empty()
            || target.chat_id == 0
            || !remove_work_ids.contains(&target.work_id)
            || !publication_ids.insert(target.publication_id.as_str())
        {
            return Err("corrupt prune receipt");
        }
        let encoded =
            serde_json::to_string(&target.message_ids).map_err(|_| "corrupt prune receipt")?;
        parse_publication_message_ids(&encoded).map_err(|_| "corrupt prune receipt")?;
        covered_work_ids.insert(target.work_id.as_str());
    }
    if remove_work_ids
        .iter()
        .any(|work_id| !covered_work_ids.contains(work_id.as_str()))
    {
        return Err("corrupt prune receipt");
    }
    Ok(())
}

async fn handle_catalog_work_prune(mut req: Request, env: &Env) -> Result<Response> {
    let request: CatalogWorkPruneRequest = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({ "ok": false, "error": "invalid json" }), 400),
    };
    if let Err(message) = validate_catalog_work_prune(&request) {
        return json_response(&json!({ "ok": false, "error": message }), 400);
    }

    let db = env.d1("DB")?;
    if let Some(receipt) = db
        .prepare(
            r#"SELECT keep_work_id, remove_work_ids_json, removed_r2_keys_json, telegram_targets_json
               FROM catalog_work_prune_receipts WHERE decision_id=?"#,
        )
        .bind(&[JsValue::from_str(&request.decision_id)])?
        .first::<CatalogWorkPruneReceiptRow>(None)
        .await?
    {
        let parsed_receipt = match parse_work_prune_receipt(&receipt) {
            Ok(parsed) => parsed,
            Err(message) => {
                return json_response(&json!({ "ok": false, "error": message }), 500);
            }
        };
        let stored_remove = parsed_receipt.remove_work_ids;
        let removed_r2_keys = parsed_receipt.removed_r2_keys;
        let telegram_targets = parsed_receipt.telegram_targets;
        if !same_prune_plan(
            &request.keep_work_id,
            &request.remove_work_ids,
            &receipt.keep_work_id,
            &stored_remove,
        ) {
            return json_response(
                &json!({ "ok": false, "error": "prune decision conflict" }),
                409,
            );
        }
        if !removed_r2_keys.is_empty() {
            env.bucket("MEDIA")?
                .delete_multiple(removed_r2_keys.clone())
                .await?;
        }
        return json_response(
            &json!({
                "ok": true,
                "removed_works": stored_remove,
                "removed_r2_keys": removed_r2_keys,
                "telegram_targets": telegram_targets,
                "replayed": true,
            }),
            200,
        );
    }

    if db
        .prepare("SELECT id FROM works WHERE id=? AND deleted_at IS NULL")
        .bind(&[JsValue::from_str(&request.keep_work_id)])?
        .first::<ActiveWorkRow>(None)
        .await?
        .map(|row| row.id)
        .as_deref()
        != Some(request.keep_work_id.as_str())
    {
        return json_response(
            &json!({ "ok": false, "error": "keep work not active" }),
            409,
        );
    }

    let mut images = Vec::new();
    let mut affected_tags = std::collections::BTreeSet::new();
    let mut telegram_targets = Vec::new();
    let mut work_versions = Vec::new();
    for work_id in &request.remove_work_ids {
        let Some(work) = db
            .prepare("SELECT review_version FROM works WHERE id=? AND deleted_at IS NULL")
            .bind(&[JsValue::from_str(work_id)])?
            .first::<WorkVersionRow>(None)
            .await?
        else {
            return json_response(
                &json!({ "ok": false, "error": format!("remove work not active: {work_id}") }),
                409,
            );
        };
        work_versions.push((work_id, work.review_version));
        let work_images = db
            .prepare(
                r#"SELECT i.id,i.work_id,i.r2_key
                   FROM images i JOIN works w ON w.id=i.work_id
                   WHERE i.work_id=? AND w.deleted_at IS NULL
                   ORDER BY i.page_index"#,
            )
            .bind(&[JsValue::from_str(work_id)])?
            .all()
            .await?
            .results::<CatalogPruneImageRow>()?;
        let image_count = work_images.len();
        images.extend(work_images);

        let tags = db
            .prepare("SELECT tag FROM work_tags WHERE work_id=?")
            .bind(&[JsValue::from_str(work_id)])?
            .all()
            .await?
            .results::<WorkTagRow>()?;
        for row in tags {
            if !row.tag.is_empty() {
                affected_tags.insert(row.tag);
            }
        }

        let publications = db
            .prepare(
                r#"SELECT id, work_id, chat_id, message_ids_json, publish_state
                   FROM telegram_publications
                   WHERE work_id=? AND deleted_at IS NULL
                   ORDER BY created_at, id"#,
            )
            .bind(&[JsValue::from_str(work_id)])?
            .all()
            .await?
            .results::<WorkPublicationRow>()?;
        if publications.is_empty() {
            return json_response(
                &json!({
                    "ok": false,
                    "error": format!("telegram publication mapping missing: {work_id}")
                }),
                409,
            );
        }
        for publication in publications {
            let message_ids = match parse_publication_message_ids(&publication.message_ids_json) {
                Ok(ids) => ids,
                Err(message) => {
                    return json_response(
                        &json!({
                            "ok": false,
                            "error": format!("{message}: {work_id}")
                        }),
                        409,
                    );
                }
            };
            if publication.publish_state != "full" || message_ids.len() != image_count {
                return json_response(
                    &json!({"ok":false,"error":"telegram publication mapping is incomplete"}),
                    409,
                );
            }
            telegram_targets.push(TelegramPruneTarget {
                publication_id: publication.id,
                work_id: publication.work_id,
                chat_id: publication.chat_id,
                message_ids,
            });
        }
    }

    let bucket = env.bucket("MEDIA")?;
    let backup_keys: Vec<String> = images
        .iter()
        .map(|row| catalog_backup_key(&request.decision_id, &row.r2_key))
        .collect();
    for (row, backup) in images.iter().zip(&backup_keys) {
        copy_catalog_object(&bucket, &row.r2_key, backup).await?;
    }

    let now = js_iso_now();
    let removed_r2_keys: Vec<String> = images.iter().map(|row| row.r2_key.clone()).collect();
    let remove_work_ids_json = serde_json::to_string(&request.remove_work_ids)
        .map_err(|error| Error::RustError(error.to_string()))?;
    let removed_r2_keys_json = serde_json::to_string(&removed_r2_keys)
        .map_err(|error| Error::RustError(error.to_string()))?;
    let telegram_targets_json = serde_json::to_string(&telegram_targets)
        .map_err(|error| Error::RustError(error.to_string()))?;

    let mut statements = Vec::new();
    for (row, backup) in images.iter().zip(&backup_keys) {
        statements.push(
            db.prepare(
                r#"INSERT INTO catalog_prune_backups
                   (original_r2_key,backup_r2_key,decision_id,work_id,created_at)
                   VALUES (?,?,?,?,?)"#,
            )
            .bind(&[
                JsValue::from_str(&row.r2_key),
                JsValue::from_str(backup),
                JsValue::from_str(&request.decision_id),
                JsValue::from_str(&row.work_id),
                JsValue::from_str(&now),
            ])?,
        );
    }
    for work_id in &request.remove_work_ids {
        statements.push(
            db.prepare("DELETE FROM images WHERE work_id=?")
                .bind(&[JsValue::from_str(work_id)])?,
        );
        statements.push(
            db.prepare("DELETE FROM work_tags WHERE work_id=?")
                .bind(&[JsValue::from_str(work_id)])?,
        );
        statements.push(
            db.prepare("UPDATE works SET deleted_at=?,review_version=review_version+1 WHERE id=?")
                .bind(&[JsValue::from_str(&now), JsValue::from_str(work_id)])?,
        );
    }
    for tag in &affected_tags {
        statements.push(
            db.prepare(
                r#"UPDATE tags SET use_count=(
                     SELECT COUNT(*) FROM work_tags WHERE tag=?
                   ) WHERE name=?"#,
            )
            .bind(&[JsValue::from_str(tag), JsValue::from_str(tag)])?,
        );
    }
    let mut guarded = vec![db
        .prepare(
            r#"INSERT INTO catalog_work_prune_receipts (
                 decision_id, keep_work_id, remove_work_ids_json, removed_r2_keys_json,
                 telegram_targets_json, telegram_state, telegram_error, created_at,
                 telegram_completed_at
               ) VALUES (?, ?, ?, ?, ?, 'pending', NULL, ?, NULL)"#,
        )
        .bind(&[
            JsValue::from_str(&request.decision_id),
            JsValue::from_str(&request.keep_work_id),
            JsValue::from_str(&remove_work_ids_json),
            JsValue::from_str(&removed_r2_keys_json),
            JsValue::from_str(&telegram_targets_json),
            JsValue::from_str(&now),
        ])?];
    // R2 backup awaits permit an image restore or a new publication. Validate the
    // complete captured set in the same transaction that removes the work.
    guarded.push(db.prepare("UPDATE catalog_work_prune_receipts SET telegram_state=CASE WHEN EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL) THEN telegram_state ELSE 'conflict' END WHERE decision_id=?")
        .bind(&[JsValue::from_str(&request.keep_work_id), JsValue::from_str(&request.decision_id)])?);
    for (work_id, version) in work_versions {
        let target_count = telegram_targets
            .iter()
            .filter(|t| &t.work_id == work_id)
            .count();
        guarded.push( db.prepare("UPDATE catalog_work_prune_receipts SET telegram_state=CASE WHEN EXISTS(SELECT 1 FROM works WHERE id=? AND review_version=? AND deleted_at IS NULL) AND (SELECT COUNT(*) FROM telegram_publications WHERE work_id=? AND deleted_at IS NULL)=? THEN telegram_state ELSE 'conflict' END WHERE decision_id=?")
            .bind(&[JsValue::from_str(work_id), JsValue::from_f64(version as f64), JsValue::from_str(work_id), JsValue::from_f64(target_count as f64), JsValue::from_str(&request.decision_id)])?);
    }
    for target in &telegram_targets {
        guarded.push( db.prepare("UPDATE catalog_work_prune_receipts SET telegram_state=CASE WHEN EXISTS(SELECT 1 FROM telegram_publications WHERE id=? AND work_id=? AND chat_id=? AND message_ids_json=? AND publish_state='full' AND deleted_at IS NULL) THEN telegram_state ELSE 'conflict' END WHERE decision_id=?")
            .bind(&[JsValue::from_str(&target.publication_id), JsValue::from_str(&target.work_id), JsValue::from_f64(target.chat_id as f64), JsValue::from_str(&serde_json::to_string(&target.message_ids).unwrap()), JsValue::from_str(&request.decision_id)])?);
    }
    guarded.extend(statements);
    if let Err(error) = db.batch(guarded).await {
        if error.to_string().contains("CHECK constraint failed") {
            return json_response(
                &json!({"ok":false,"error":"work or publication changed concurrently; retry prune"}),
                409,
            );
        }
        return Err(error);
    }
    if !removed_r2_keys.is_empty() {
        bucket.delete_multiple(removed_r2_keys.clone()).await?;
    }

    json_response(
        &json!({
            "ok": true,
            "removed_works": request.remove_work_ids,
            "removed_r2_keys": removed_r2_keys,
            "telegram_targets": telegram_targets,
            "replayed": false,
        }),
        200,
    )
}

fn validate_catalog_retract(
    request: &CatalogRetractRequest,
) -> std::result::Result<(), &'static str> {
    if request.decision_id.trim().is_empty() || request.decision_id.len() > 160 {
        return Err("invalid decision id");
    }
    validate_work_id_value(&request.work_id)
}

fn is_missing_r2_object_message(message: &str) -> bool {
    message.contains("R2 object not found")
}

fn is_missing_r2_object(error: &Error) -> bool {
    is_missing_r2_object_message(&error.to_string())
}

async fn delete_retract_originals(
    db: &D1Database,
    bucket: &Bucket,
    work_id: &str,
    decision_id: &str,
) -> Result<()> {
    let keys = db
        .prepare(
            "SELECT original_r2_key FROM catalog_prune_backups WHERE work_id=? AND decision_id=?",
        )
        .bind(&[JsValue::from_str(work_id), JsValue::from_str(decision_id)])?
        .all()
        .await?
        .results::<RetractBackupRow>()?
        .into_iter()
        .map(|row| row.original_r2_key)
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    if !keys.is_empty() {
        bucket.delete_multiple(keys).await?;
    }
    Ok(())
}

async fn handle_catalog_retract(mut req: Request, env: &Env) -> Result<Response> {
    let request: CatalogRetractRequest = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({ "ok": false, "error": "invalid json" }), 400),
    };
    if let Err(message) = validate_catalog_retract(&request) {
        return json_response(&json!({ "ok": false, "error": message }), 400);
    }

    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare("SELECT id, deleted_at, review_version, (SELECT COUNT(*) FROM images WHERE work_id=works.id) AS image_count FROM works WHERE id=?")
        .bind(&[JsValue::from_str(&request.work_id)])?
        .first::<RetractWorkRow>(None)
        .await?
    else {
        return json_response(&json!({ "ok": false, "error": "work not active" }), 409);
    };
    if work.id != request.work_id {
        return json_response(&json!({ "ok": false, "error": "work not active" }), 409);
    }
    let bucket = env.bucket("MEDIA")?;
    if work.deleted_at.is_some() && work.image_count == 0 {
        let updated = db.prepare("UPDATE works SET review_version=review_version+1 WHERE id=? AND review_version=? AND deleted_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM images WHERE work_id=works.id)")
            .bind(&[JsValue::from_str(&request.work_id), JsValue::from_f64(work.review_version as f64)])?
            .run()
            .await?;
        if updated.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
            return json_response(
                &json!({"ok":false,"error":"work was restored concurrently; retry retract"}),
                409,
            );
        }
        delete_retract_originals(&db, &bucket, &request.work_id, &request.decision_id).await?;
        return json_response(
            &json!({
                "ok": true,
                "work_id": request.work_id,
                "replayed": true,
            }),
            200,
        );
    }

    let images = db
        .prepare(
            r#"SELECT i.id,i.work_id,i.r2_key
               FROM images i JOIN works w ON w.id=i.work_id
               WHERE i.work_id=?
               ORDER BY i.page_index"#,
        )
        .bind(&[JsValue::from_str(&request.work_id)])?
        .all()
        .await?
        .results::<CatalogPruneImageRow>()?;
    let tags = db
        .prepare("SELECT tag FROM work_tags WHERE work_id=?")
        .bind(&[JsValue::from_str(&request.work_id)])?
        .all()
        .await?
        .results::<WorkTagRow>()?;

    let mut copied: Vec<(&CatalogPruneImageRow, String)> = Vec::new();
    for row in &images {
        let backup = catalog_backup_key(&request.decision_id, &row.r2_key);
        match copy_catalog_object(&bucket, &row.r2_key, &backup).await {
            Ok(()) => copied.push((row, backup)),
            Err(error) if is_missing_r2_object(&error) => {}
            Err(error) => return Err(error),
        }
    }

    let now = js_iso_now();
    let removed_r2_keys: Vec<String> = images.iter().map(|row| row.r2_key.clone()).collect();
    // R2 copies await outside the transaction. A concurrent ingest, hide, restore,
    // or image deletion must abort the whole D1 batch before any metadata is lost.
    let mut statements = vec![db.prepare(
        "UPDATE works SET review_version=CASE WHEN review_version=? AND deleted_at IS ? AND (SELECT COUNT(*) FROM images WHERE work_id=works.id)=? AND (SELECT COUNT(*) FROM images WHERE work_id=works.id AND r2_key IN (SELECT value FROM json_each(?)))=? THEN review_version+1 ELSE NULL END WHERE id=?",
    ).bind(&[
        JsValue::from_f64(work.review_version as f64),
        work.deleted_at.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL),
        JsValue::from_f64(work.image_count as f64),
        JsValue::from_str(&json!(removed_r2_keys).to_string()),
        JsValue::from_f64(work.image_count as f64),
        JsValue::from_str(&request.work_id),
    ])?];
    for (row, backup) in &copied {
        statements.push(
            db.prepare(
                r#"INSERT INTO catalog_prune_backups
                   (original_r2_key,backup_r2_key,decision_id,work_id,created_at)
                   VALUES (?,?,?,?,?)"#,
            )
            .bind(&[
                JsValue::from_str(&row.r2_key),
                JsValue::from_str(backup),
                JsValue::from_str(&request.decision_id),
                JsValue::from_str(&row.work_id),
                JsValue::from_str(&now),
            ])?,
        );
    }
    statements.push(
        db.prepare("DELETE FROM images WHERE work_id=?")
            .bind(&[JsValue::from_str(&request.work_id)])?,
    );
    statements.push(
        db.prepare("DELETE FROM work_tags WHERE work_id=?")
            .bind(&[JsValue::from_str(&request.work_id)])?,
    );
    statements.push(
        db.prepare("UPDATE works SET deleted_at=? WHERE id=?")
            .bind(&[JsValue::from_str(&now), JsValue::from_str(&request.work_id)])?,
    );
    statements.push(
        db.prepare(
            "UPDATE telegram_publications SET deleted_at=? WHERE work_id=? AND deleted_at IS NULL",
        )
        .bind(&[JsValue::from_str(&now), JsValue::from_str(&request.work_id)])?,
    );
    for tag in tags {
        if tag.tag.is_empty() {
            continue;
        }
        statements.push(
            db.prepare(
                r#"UPDATE tags SET use_count=(
                     SELECT COUNT(*) FROM work_tags WHERE tag=?
                   ) WHERE name=?"#,
            )
            .bind(&[JsValue::from_str(&tag.tag), JsValue::from_str(&tag.tag)])?,
        );
    }
    if let Err(error) = db.batch(statements).await {
        if error
            .to_string()
            .contains("NOT NULL constraint failed: works.review_version")
        {
            return json_response(
                &json!({"ok":false,"error":"work changed concurrently; retry retract"}),
                409,
            );
        }
        return Err(error);
    }
    if !removed_r2_keys.is_empty() {
        bucket.delete_multiple(removed_r2_keys).await?;
    }

    json_response(
        &json!({
            "ok": true,
            "work_id": request.work_id,
            "replayed": false,
        }),
        200,
    )
}

fn validate_telegram_result(
    value: &CatalogTelegramResult,
) -> std::result::Result<(), &'static str> {
    if value.decision_id.trim().is_empty() || value.decision_id.len() > 160 {
        return Err("invalid decision id");
    }
    if value.complete
        && value
            .error
            .as_deref()
            .map(str::trim)
            .is_some_and(|error| !error.is_empty())
    {
        return Err("successful result cannot include error");
    }
    Ok(())
}

fn sanitize_telegram_error(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|value| !value.is_empty())?;
    let filtered: String = raw.chars().filter(|ch| !ch.is_control()).collect();
    let truncated: String = filtered.chars().take(500).collect();
    if truncated.is_empty() {
        return None;
    }
    let lower = truncated.to_ascii_lowercase();
    if lower.contains("bearer ")
        || lower.contains("authorization:")
        || lower.contains("cookie:")
        || lower.contains("set-cookie:")
    {
        return Some("[redacted]".into());
    }
    Some(truncated)
}

async fn handle_catalog_telegram_result(mut req: Request, env: &Env) -> Result<Response> {
    let request: CatalogTelegramResult = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({ "ok": false, "error": "invalid json" }), 400),
    };
    if let Err(message) = validate_telegram_result(&request) {
        return json_response(&json!({ "ok": false, "error": message }), 400);
    }

    let db = env.d1("DB")?;
    let Some(receipt) = db
        .prepare(
            r#"SELECT telegram_state, remove_work_ids_json, telegram_targets_json
               FROM catalog_work_prune_receipts WHERE decision_id=?"#,
        )
        .bind(&[JsValue::from_str(&request.decision_id)])?
        .first::<TelegramResultReceiptRow>(None)
        .await?
    else {
        return json_response(
            &json!({ "ok": false, "error": "prune receipt not found" }),
            409,
        );
    };

    if request.complete {
        if receipt.telegram_state == "complete" {
            return json_response(
                &json!({
                    "ok": true,
                    "telegram_state": "complete",
                    "idempotent": true,
                }),
                200,
            );
        }
        let remove_work_ids: Vec<String> = serde_json::from_str(&receipt.remove_work_ids_json)
            .map_err(|_| Error::RustError("corrupt prune receipt".into()))?;
        let targets: Vec<TelegramPruneTarget> =
            serde_json::from_str(&receipt.telegram_targets_json)
                .map_err(|_| Error::RustError("corrupt prune receipt".into()))?;
        validate_stored_telegram_targets(&targets, &remove_work_ids)
            .map_err(|message| Error::RustError(message.into()))?;
        let now = js_iso_now();
        let mut statements = Vec::new();
        for target in &targets {
            statements.push(
                db.prepare("UPDATE telegram_publications SET deleted_at=? WHERE id=?")
                    .bind(&[
                        JsValue::from_str(&now),
                        JsValue::from_str(&target.publication_id),
                    ])?,
            );
        }
        statements.push(
            db.prepare(
                r#"UPDATE catalog_work_prune_receipts
                   SET telegram_state='complete', telegram_error=NULL, telegram_completed_at=?
                   WHERE decision_id=?"#,
            )
            .bind(&[
                JsValue::from_str(&now),
                JsValue::from_str(&request.decision_id),
            ])?,
        );
        db.batch(statements).await?;
        return json_response(
            &json!({
                "ok": true,
                "telegram_state": "complete",
                "idempotent": false,
            }),
            200,
        );
    }

    if receipt.telegram_state == "complete" {
        return json_response(
            &json!({ "ok": false, "error": "telegram already complete" }),
            409,
        );
    }
    let error_text = sanitize_telegram_error(request.error.as_deref());
    db.prepare(
        r#"UPDATE catalog_work_prune_receipts
           SET telegram_error=?
           WHERE decision_id=? AND telegram_state='pending'"#,
    )
    .bind(&[
        match error_text.as_deref() {
            Some(text) => JsValue::from_str(text),
            None => JsValue::NULL,
        },
        JsValue::from_str(&request.decision_id),
    ])?
    .run()
    .await?;
    json_response(
        &json!({
            "ok": true,
            "telegram_state": "pending",
            "idempotent": false,
        }),
        200,
    )
}

#[derive(Deserialize, Serialize)]
struct VersionedRows<T> {
    generation: u64,
    rows: Vec<T>,
}

async fn cached_filter_rows<T: serde::de::DeserializeOwned + Serialize>(
    url: &Url,
    env: &Env,
    kind: &str,
    sql: &str,
) -> Result<Option<(Vec<T>, &'static str)>> {
    let db = env.d1("DB")?;
    let generation = similar::current_generation(&db).await?;
    let cache = Cache::default();
    let key = format!(
        "{}/__filters/v1/{kind}/{generation}",
        url.origin().ascii_serialization()
    );
    let cached = match cache.get(key.clone(), false).await {
        Ok(Some(mut response)) => response.json::<VersionedRows<T>>().await.ok(),
        _ => None,
    };
    let (rows, source) = if let Some(cached) = cached.filter(|c| c.generation == generation) {
        (cached.rows, "HIT")
    } else {
        (db.prepare(sql).all().await?.results::<T>()?, "MISS")
    };
    if similar::current_generation(&db).await? != generation {
        return Ok(None);
    }
    let value = VersionedRows { generation, rows };
    if source == "MISS" {
        let mut response = Response::from_json(&value)?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=86400")?;
        let _ = cache.put(key, response).await;
    }
    Ok(Some((value.rows, source)))
}

fn filter_tags(
    mut tags: Vec<TagRow>,
    q: &str,
    by_name: bool,
    limit: u32,
    offset: u32,
) -> (Vec<TagRow>, Option<u64>) {
    // SQLite LIKE folds ASCII only; the API treats %, _ and backslash literally.
    let needle = q.trim_start_matches('#').to_ascii_lowercase();
    tags.retain(|tag| tag.name.to_ascii_lowercase().contains(&needle));
    if by_name {
        tags.sort_by(|a, b| a.name.cmp(&b.name));
    }
    let next_offset = (tags.len() as u64 > u64::from(offset) + u64::from(limit))
        .then_some(u64::from(offset) + u64::from(limit));
    (
        tags.into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect(),
        next_offset,
    )
}

async fn handle_list_tags(url: &Url, env: &Env) -> Result<Response> {
    let q = query_param(url, "q");
    if q.len() > 256 {
        return json_response(&json!({"ok": false, "error": "query too long"}), 400);
    }
    let limit = query_param(url, "limit")
        .parse::<u32>()
        .unwrap_or(200)
        .clamp(1, 200);
    let (_, offset) = parse_limit_offset(url);
    let Some((tags, source)) =
        cached_filter_rows::<TagRow>(url, env, "tags", tags_list_sql()).await?
    else {
        return json_response(
            &json!({"ok": false, "error": "catalog changed; retry"}),
            409,
        );
    };
    let (tags, next_offset) =
        filter_tags(tags, &q, query_param(url, "order") == "name", limit, offset);
    let mut response = json_response(
        &json!({ "ok": true, "tags": tags, "next_offset": next_offset }),
        200,
    )?;
    response
        .headers_mut()
        .set("X-Catalog-Stats-Cache", source)?;
    Ok(response)
}

async fn handle_list_sources(url: &Url, env: &Env) -> Result<Response> {
    let Some((sources, source)) = cached_filter_rows::<SourceRow>(
        url,
        env,
        "sources",
        r#"
        SELECT source, COUNT(*) AS cnt FROM works WHERE deleted_at IS NULL
        GROUP BY source ORDER BY cnt DESC, source ASC
    "#,
    )
    .await?
    else {
        return json_response(
            &json!({"ok": false, "error": "catalog changed; retry"}),
            409,
        );
    };
    let mut response = json_response(&json!({"ok": true, "sources": sources}), 200)?;
    response
        .headers_mut()
        .set("X-Catalog-Stats-Cache", source)?;
    Ok(response)
}

const PUBLIC_MEDIA_CACHE: &str = "private, max-age=3600";
// Cloudflare's resizer may keep originals at the edge; viewers still reach them
// only through this Worker's visibility check.
const RESIZER_MEDIA_CACHE: &str = "public, max-age=604800";

/// Build media response header pairs from R2 HTTP metadata.
/// `Headers::clone()` is a deep copy in worker 0.8, so we must not write into a clone.
pub fn media_header_pairs(
    content_type: Option<&str>,
    content_language: Option<&str>,
    content_disposition: Option<&str>,
    content_encoding: Option<&str>,
    cache_control: Option<&str>,
    etag: &str,
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if let Some(v) = content_type.filter(|s| !s.is_empty()) {
        pairs.push(("Content-Type".into(), v.to_string()));
    }
    if let Some(v) = content_language.filter(|s| !s.is_empty()) {
        pairs.push(("Content-Language".into(), v.to_string()));
    }
    if let Some(v) = content_disposition.filter(|s| !s.is_empty()) {
        pairs.push(("Content-Disposition".into(), v.to_string()));
    }
    if let Some(v) = content_encoding.filter(|s| !s.is_empty()) {
        pairs.push(("Content-Encoding".into(), v.to_string()));
    }
    // Browsers may reuse public bytes for an hour after a catalog deletion; every
    // new request still passes the visibility check in `handle_media`.
    let _ = cache_control;
    pairs.push(("Cache-Control".into(), PUBLIC_MEDIA_CACHE.into()));
    // Prevent browsers from MIME-sniffing image responses into executable contexts.
    pairs.push(("X-Content-Type-Options".into(), "nosniff".into()));
    if !etag.is_empty() {
        pairs.push(("etag".into(), etag.to_string()));
    }
    pairs
}

async fn handle_media(key: &str, req: &Request, env: &Env) -> Result<Response> {
    let Ok(decoded) = urlencoding::decode(key) else {
        return Response::error("invalid media key", 400);
    };
    if decoded.is_empty() || decoded.len() > 1024 || decoded.chars().any(char::is_control) {
        return Response::error("invalid media key", 400);
    }
    let url = req.url()?;
    let width = match media_width(
        url.query_pairs()
            .find(|(key, _)| key == "width")
            .as_ref()
            .map(|(_, value)| value.as_ref()),
    ) {
        Ok(width) => width,
        Err(_) => return Response::error("invalid image width", 400),
    };

    #[derive(Deserialize)]
    struct MediaVisibility {
        deleted_at: Option<String>,
    }
    let visible = env.d1("DB")?.prepare("SELECT w.deleted_at FROM images i JOIN works w ON w.id=i.work_id WHERE i.r2_key=? LIMIT 1")
        .bind(&[JsValue::from_str(&decoded)])?.first::<MediaVisibility>(None).await?;
    let private = visible
        .as_ref()
        .is_some_and(|work| work.deleted_at.is_some());
    if visible.is_none()
        || (private && !admin::request_has_valid_cookie(req, &admin::env_ingest_token(env))?)
    {
        let mut response = Response::error("not found", 404)?;
        response.headers_mut().set("Cache-Control", "no-store")?;
        return Ok(response);
    }

    let mut resize_fallback = None;
    // Only these three variants may be generated. Check visibility above even
    // when Cloudflare already has a resized image in its internal cache.
    // Skip transformation for loopback development URLs.
    if let Some(width) = width.filter(|_| {
        !private
            && !matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
            )
    }) {
        let mut source = url.clone();
        source.set_query(None);
        let accept = req.headers().get("Accept")?.unwrap_or_default();
        let format = if accept.contains("image/avif") {
            Some(ResizeFormat::Avif)
        } else if accept.contains("image/webp") {
            Some(ResizeFormat::Webp)
        } else {
            None
        };
        let mut cf = CfProperties::new();
        cf.image = Some(ResizeConfig {
            width: Some(width),
            fit: Some(ResizeFit::ScaleDown),
            format,
            ..ResizeConfig::default()
        });
        let mut init = RequestInit::new();
        init.with_cf_properties(cf);
        let resized = Fetch::Request(Request::new_with_init(source.as_str(), &init)?)
            .send()
            .await;
        if let Ok(mut response) = resized {
            if response.status_code() == 200
                && resized_mime_allowed(
                    response
                        .headers()
                        .get("Content-Type")?
                        .as_deref()
                        .unwrap_or_default(),
                )
            {
                let headers = response.headers().clone();
                headers.set("Cache-Control", PUBLIC_MEDIA_CACHE)?;
                headers.set("Vary", "Accept")?;
                if let Some(etag) = headers.get("ETag")? {
                    if media_not_modified(req.headers().get("If-None-Match")?.as_deref(), &etag) {
                        return Ok(Response::empty()?.with_status(304).with_headers(headers));
                    }
                }
                if req.method() == Method::Head {
                    return Ok(Response::empty()?.with_headers(headers));
                }
                response = response.with_headers(headers);
                return Ok(response);
            }
            resize_fallback = Some(format!(
                "{} {} {}",
                response.status_code(),
                response.headers().get("Content-Type")?.unwrap_or_default(),
                response.headers().get("cf-resized")?.unwrap_or_default()
            ));
        } else {
            resize_fallback = Some("fetch failed".to_string());
        }
        console_warn!("image resize unavailable; serving original");
        // Unsupported/failed resizing keeps the original image usable.
    }

    let bucket = env.bucket("MEDIA")?;
    let Some(obj) = bucket.get(decoded.as_ref()).execute().await? else {
        return Response::error("not found", 404);
    };

    let body = obj
        .body()
        .ok_or_else(|| Error::RustError("empty object body".into()))?;
    let response_body = body.response_body()?;

    let meta = obj.http_metadata();
    let Ok(content_type) = ingest::normalize_allowed_image_mime(
        meta.content_type.as_deref().unwrap_or_default(),
        &decoded,
    ) else {
        return Response::error("not found", 404);
    };
    let pairs = media_header_pairs(
        Some(content_type),
        meta.content_language.as_deref(),
        meta.content_disposition.as_deref(),
        meta.content_encoding.as_deref(),
        meta.cache_control.as_deref(),
        &obj.http_etag(),
    );

    let headers = Headers::new();
    for (name, value) in pairs {
        let _ = headers.set(&name, &value);
    }
    // Says why a requested thumbnail fell back to the original bytes.
    if let Some(reason) = &resize_fallback {
        headers.set("X-Resize-Fallback", reason)?;
    }
    if private {
        headers.set("Cache-Control", "private, no-store")?;
        headers.set("Cross-Origin-Resource-Policy", "same-origin")?;
    } else if req
        .headers()
        .get("Via")?
        .is_some_and(|via| via.contains("image-resizing"))
    {
        headers.set("Cache-Control", RESIZER_MEDIA_CACHE)?;
    }

    if !private
        && media_not_modified(
            req.headers().get("If-None-Match")?.as_deref(),
            &obj.http_etag(),
        )
    {
        return Ok(Response::empty()?.with_status(304).with_headers(headers));
    }

    if req.method() == Method::Head {
        return Ok(Response::empty()?.with_headers(headers));
    }

    Ok(Response::from_body(response_body)?.with_headers(headers))
}

/// Resized output may be AVIF, which uploads never are; anything else must
/// still pass the ingest allow-list.
fn resized_mime_allowed(content_type: &str) -> bool {
    content_type.trim().eq_ignore_ascii_case("image/avif")
        || ingest::normalize_allowed_image_mime(content_type, "").is_ok()
}

fn media_not_modified(value: Option<&str>, etag: &str) -> bool {
    value.is_some_and(|value| {
        value
            .split(',')
            .any(|tag| tag.trim().trim_start_matches("W/") == etag || tag.trim() == "*")
    })
}

fn media_width(value: Option<&str>) -> std::result::Result<Option<usize>, ()> {
    match value {
        None => Ok(None),
        Some("160") => Ok(Some(160)),
        Some("640") => Ok(Some(640)),
        Some("1280") => Ok(Some(1280)),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn d1_daily_limits_are_specific_and_reset_at_utc_midnight() {
        for kind in ["read", "write"] {
            assert!(is_d1_daily_limit(&format!(
                "D1_ERROR: Your account has exceeded D1's free tier daily row {kind} limit."
            )));
        }
        assert!(!is_d1_daily_limit("D1_ERROR: query exceeded memory limit"));
        assert!(!is_d1_daily_limit("too many requests"));
        assert_eq!(seconds_until_d1_reset(0), 86_400);
        assert_eq!(seconds_until_d1_reset(86_399), 1);
        assert_eq!(seconds_until_d1_reset(86_400), 86_400);
        assert_eq!(seconds_until_d1_reset(86_400 + 43_200), 43_200);
    }

    #[test]
    fn normalize_tag_trims_hash_and_spaces() {
        assert_eq!(normalize_tag("  #foo bar  "), "foo_bar");
        assert_eq!(normalize_tag("#already_ok"), "already_ok");
        assert_eq!(normalize_tag(""), "");
    }

    #[test]
    fn normalize_tag_caps_at_64_chars() {
        let long = "a".repeat(80);
        assert_eq!(normalize_tag(&long).len(), 64);
    }

    #[test]
    fn parse_limit_offset_clamps() {
        let url = Url::parse("https://example.test/api/works?limit=999&offset=-5").unwrap();
        assert_eq!(parse_limit_offset(&url), (100, 0));

        let url = Url::parse("https://example.test/api/works?limit=0&offset=10").unwrap();
        assert_eq!(parse_limit_offset(&url), (1, 10));

        let url = Url::parse("https://example.test/api/works").unwrap();
        assert_eq!(parse_limit_offset(&url), (40, 0));
        let url = Url::parse("https://example.test/api/works?offset=9223372036854775807").unwrap();
        assert_eq!(parse_limit_offset(&url), (40, u32::MAX));
    }

    #[test]
    fn tag_filter_splits_normalizes_and_dedupes() {
        assert!(tag_filter("").is_empty());
        assert!(tag_filter(" , ,").is_empty());
        assert_eq!(tag_filter("#foo bar"), ["foo_bar"]);
        assert_eq!(tag_filter("a, #b ,a,,c"), ["a", "b", "c"]);
    }

    #[test]
    fn works_from_clause_matches_tag_count() {
        assert_eq!(works_from_clause(0), "FROM works w");
        assert!(works_from_clause(1).starts_with("FROM work_tags filter CROSS JOIN"));
        let many = works_from_clause(3);
        assert!(many.contains("SELECT DISTINCT work_id FROM work_tags WHERE tag IN (?, ?, ?)"));
        assert_eq!(many.matches('?').count(), 3);
    }

    #[test]
    fn request_lengths_and_media_variants_are_bounded() {
        assert_eq!(request_length_error(None, 10), None);
        assert_eq!(request_length_error(Some("10"), 10), None);
        assert_eq!(
            request_length_error(Some("11"), 10),
            Some((413, "request body too large"))
        );
        assert_eq!(
            request_length_error(Some("-1"), 10),
            Some((400, "invalid content length"))
        );
        assert_eq!(media_width(None), Ok(None));
        for width in ["160", "640", "1280"] {
            assert!(media_width(Some(width)).is_ok());
        }
        assert!(resized_mime_allowed("image/avif"));
        assert!(resized_mime_allowed("image/webp"));
        assert!(!resized_mime_allowed("text/html"));
        assert!(!resized_mime_allowed(""));
        for width in ["", "0", "00160", "999999", "640,1280"] {
            assert!(media_width(Some(width)).is_err());
        }
        assert!(media_not_modified(Some("W/\"abc\", \"def\""), "\"abc\""));
        assert!(media_not_modified(Some("*"), "\"abc\""));
        assert!(!media_not_modified(Some("\"def\""), "\"abc\""));
    }

    #[test]
    fn health_payload_shape() {
        let v = json!({ "ok": true, "service": "vitrine" });
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "vitrine");
    }

    #[test]
    fn work_out_serializes_like_ts() {
        let work = WorkOut {
            id: "pixiv:1".into(),
            source: "pixiv".into(),
            source_id: "1".into(),
            source_url: "https://example".into(),
            title: "t".into(),
            author_name: "a".into(),
            author_url: "".into(),
            is_r18: false,
            page_count: 1,
            created_at: "2026-01-01T00:00:00Z".into(),
            cover_url: Some("/media/pixiv/1/00.jpg".into()),
            cover_width: Some(1200),
            cover_height: None,
            tags: vec!["foo".into(), "bar".into()],
        };
        let v = serde_json::to_value(&work).unwrap();
        assert_eq!(v["id"], "pixiv:1");
        assert_eq!(v["cover_url"], "/media/pixiv/1/00.jpg");
        assert_eq!(v["tags"][0], "foo");
        assert_eq!(v["is_r18"], false);
        assert_eq!(v["cover_width"], 1200);
        assert!(v["cover_height"].is_null());
        assert!(v.get("origin").is_none());
    }

    #[test]
    fn tags_from_group_concat_splits() {
        assert_eq!(tags_from_group_concat(Some("a,b,c")), vec!["a", "b", "c"]);
        assert!(tags_from_group_concat(None).is_empty());
        assert!(tags_from_group_concat(Some("")).is_empty());
    }

    #[test]
    fn cover_url_from_key_builds_media_path() {
        assert_eq!(
            cover_url_from_key(Some("pixiv/1/00.jpg")).as_deref(),
            Some("/media/pixiv/1/00.jpg")
        );
        assert_eq!(cover_url_from_key(Some("")), None);
        assert_eq!(cover_url_from_key(None), None);
        assert_eq!(
            cover_url_from_key(Some("pixiv/a?#/00.jpg")).as_deref(),
            Some("/media/pixiv/a%3F%23/00.jpg")
        );
    }

    #[test]
    fn media_headers_copy_content_type_and_override_cache() {
        let pairs = media_header_pairs(
            Some("image/png"),
            Some("en"),
            None,
            None,
            Some("no-cache"),
            "\"abc\"",
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert_eq!(
            map.get("Content-Type").map(String::as_str),
            Some("image/png")
        );
        assert_eq!(map.get("Content-Language").map(String::as_str), Some("en"));
        assert_eq!(
            map.get("Cache-Control").map(String::as_str),
            Some("private, max-age=3600")
        );
        assert_eq!(
            map.get("X-Content-Type-Options").map(String::as_str),
            Some("nosniff")
        );
        assert_eq!(map.get("etag").map(String::as_str), Some("\"abc\""));
    }

    #[test]
    fn fingerprint_digest_matches_hanabi_contract() {
        let value: Value = serde_json::from_str(r#"{"average_hash":"0000000000000001","bytes":1234,"color_key":"color","content_sha256":"sha-a","detail_key":"detail","difference_hash":"0000000000000002","format":"JPEG","height":200,"r2_key":"pixiv/42/batch/00.jpg","regions":[{"average_hash":"0000000000000003","color_key":"region","difference_hash":"0000000000000004","height":200,"width":50}],"solid_color":false,"strict_key":"strict","width":100}"#).unwrap();
        assert_eq!(
            fingerprint_digest(&value).unwrap(),
            "7d474f4b43cce9d9d5f182bbaf5176672905e82d8ffbe389feb6a528630052c2"
        );
    }

    #[test]
    fn tags_list_sql_excludes_soft_deleted_works() {
        let sql = tags_list_sql();
        assert!(sql.contains("deleted_at IS NULL"));
        assert!(
            sql.contains("JOIN works w")
                || sql.contains("join works w")
                || sql.contains("works w ON")
        );
        assert!(sql.contains("COUNT(w.id)"));
    }

    #[test]
    fn catalog_list_sql_exposes_every_active_image_in_stable_order() {
        let sql = catalog_list_sql();
        assert!(sql.contains("FROM images i"));
        assert!(sql.contains("JOIN works w ON w.id = i.work_id"));
        assert!(sql.contains("w.deleted_at IS NULL"));
        assert!(sql.contains("ORDER BY i.id"));
        assert!(sql.contains("i.sha256"));
        assert!(sql.contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn catalog_work_list_sql_returns_only_one_active_work_in_page_order() {
        let sql = catalog_work_list_sql();
        assert!(sql.contains("i.work_id=?"));
        assert!(sql.contains("w.deleted_at IS NULL"));
        assert!(sql.contains("ORDER BY i.page_index"));
    }

    #[test]
    fn catalog_prune_request_requires_one_distinct_keep_and_bounded_removals() {
        let valid = CatalogPruneRequest {
            decision_id: "review-1".into(),
            keep_r2_key: "x/1/00.jpg".into(),
            remove_r2_keys: vec!["x/2/00.jpg".into()],
        };
        assert!(validate_catalog_prune(&valid).is_ok());

        let mut invalid = valid.clone();
        invalid.remove_r2_keys.push(invalid.keep_r2_key.clone());
        assert_eq!(
            validate_catalog_prune(&invalid),
            Err("keep key cannot be removed")
        );

        let mut duplicate = valid;
        duplicate.remove_r2_keys.push("x/2/00.jpg".into());
        assert_eq!(
            validate_catalog_prune(&duplicate),
            Err("duplicate remove key")
        );
    }

    #[test]
    fn publication_upsert_requires_complete_ids_shape() {
        let valid = CatalogPublicationRequest {
            work_id: "pixiv:1".into(),
            chat_id: -100123,
            message_ids: vec![41, 42],
            publish_state: "full".into(),
        };
        assert!(validate_catalog_publication(&valid).is_ok());
        let duplicate = CatalogPublicationRequest {
            message_ids: vec![41, 41],
            ..valid
        };
        assert_eq!(
            validate_catalog_publication(&duplicate),
            Err("duplicate message id")
        );
    }

    #[test]
    fn missing_r2_object_is_skippable_for_retract() {
        assert!(is_missing_r2_object_message(
            "R2 object not found: douyin/1/00.jpg"
        ));
        assert!(!is_missing_r2_object_message("R2 put failed"));
    }

    #[test]
    fn catalog_retract_requires_work_id() {
        let valid = CatalogRetractRequest {
            decision_id: "hanabi-undo-1".into(),
            work_id: "douyin:7672448675021161593".into(),
        };
        assert!(validate_catalog_retract(&valid).is_ok());
        let invalid = CatalogRetractRequest {
            work_id: "bad".into(),
            ..valid
        };
        assert_eq!(validate_catalog_retract(&invalid), Err("invalid work id"));
    }

    #[test]
    fn whole_work_prune_is_bounded_and_distinct() {
        let valid = CatalogWorkPruneRequest {
            decision_id: "hanabi-similar-91".into(),
            keep_work_id: "pixiv:2".into(),
            remove_work_ids: vec!["douyin:1".into()],
        };
        assert!(validate_catalog_work_prune(&valid).is_ok());
        let invalid = CatalogWorkPruneRequest {
            remove_work_ids: vec!["pixiv:2".into()],
            ..valid
        };
        assert_eq!(
            validate_catalog_work_prune(&invalid),
            Err("keep work cannot be removed")
        );
    }

    #[test]
    fn receipt_replay_requires_identical_plan() {
        assert!(same_prune_plan(
            "pixiv:2",
            &["douyin:1".into()],
            "pixiv:2",
            &["douyin:1".into()]
        ));
        assert!(!same_prune_plan(
            "pixiv:2",
            &["douyin:1".into()],
            "pixiv:3",
            &["douyin:1".into()]
        ));
    }

    #[test]
    fn corrupt_work_prune_receipt_is_rejected() {
        let receipt = CatalogWorkPruneReceiptRow {
            keep_work_id: "pixiv:2".into(),
            remove_work_ids_json: r#"["douyin:1"]"#.into(),
            removed_r2_keys_json: r#"["douyin/1/00.jpg"]"#.into(),
            telegram_targets_json: "[]".into(),
        };
        assert!(matches!(
            parse_work_prune_receipt(&receipt),
            Err("corrupt prune receipt")
        ));
    }

    #[test]
    fn successful_telegram_result_rejects_error_text() {
        let value = CatalogTelegramResult {
            decision_id: "d1".into(),
            complete: true,
            error: Some("unexpected".into()),
        };
        assert_eq!(
            validate_telegram_result(&value),
            Err("successful result cannot include error")
        );
    }
}
