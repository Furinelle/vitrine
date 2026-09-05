mod ingest;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::JsValue;
use worker::*;

/// Cloudflare Worker entrypoint for Vitrine (Rust / workers-rs).
/// Read-path parity with the existing TypeScript Worker, plus Phase 2 ingest.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    match handle_request(req, env).await {
        Ok(res) => Ok(res),
        Err(err) => {
            console_error!("vitrine error: {err}");
            Ok(with_cors(json_response(
                &json!({ "ok": false, "error": err.to_string() }),
                500,
            )?))
        }
    }
}

async fn handle_request(req: Request, env: Env) -> Result<Response> {
    let method = req.method();
    let url = req.url()?;
    let path = url.path().to_string();

    if method == Method::Options {
        return Ok(with_cors(Response::empty()?.with_status(204)));
    }

    if path == "/api/health" {
        return Ok(with_cors(json_response(
            &json!({ "ok": true, "service": "vitrine" }),
            200,
        )?));
    }

    if path == "/api/ingest" && method == Method::Post {
        return ingest::handle_ingest(req, env).await;
    }

    if path == "/api/works" && method == Method::Get {
        return Ok(with_cors(handle_list_works(&url, &env).await?));
    }

    if path == "/api/catalog" && method == Method::Get {
        if let Some(response) = require_catalog_auth(&req, &env)? {
            return Ok(response);
        }
        return Ok(with_cors(handle_list_catalog(&url, &env).await?));
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
        return Ok(with_cors(handle_list_tags(&env).await?));
    }

    if path == "/api/sources" && method == Method::Get {
        return Ok(with_cors(handle_list_sources(&env).await?));
    }

    if let Some(key) = path.strip_prefix("/media/") {
        if method == Method::Get {
            return handle_media(key, &env).await;
        }
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
    let _ = headers.set("Access-Control-Allow-Methods", "GET, POST, PUT, OPTIONS");
    let _ = headers.set(
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type, Idempotency-Key",
    );
    res.with_headers(headers)
}

pub(crate) fn json_response(data: &Value, status: u16) -> Result<Response> {
    let res = Response::from_json(data)?.with_status(status);
    let headers = res.headers().clone();
    let _ = headers.set("Content-Type", "application/json; charset=utf-8");
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
    let offset = offset.max(0) as u32;
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
    cover_key
        .filter(|k| !k.is_empty())
        .map(|k| format!("/media/{k}"))
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
    origin: String,
    created_at: String,
    cover_url: Option<String>,
    tags: Vec<String>,
}

async fn handle_list_works(url: &Url, env: &Env) -> Result<Response> {
    let tag = query_param(url, "tag");
    let source = query_param(url, "source").to_lowercase();
    let q = query_param(url, "q");
    let (limit, offset) = parse_limit_offset(url);

    let mut where_parts: Vec<&str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if !source.is_empty() {
        where_parts.push("w.source = ?");
        binds.push(source);
    }
    if !tag.is_empty() {
        where_parts
            .push("EXISTS (SELECT 1 FROM work_tags wt WHERE wt.work_id = w.id AND wt.tag = ?)");
        binds.push(normalize_tag(&tag));
    }
    if !q.is_empty() {
        where_parts.push("(w.title LIKE ? OR w.author_name LIKE ? OR w.source_id LIKE ?)");
        let like = format!("%{q}%");
        binds.push(like.clone());
        binds.push(like.clone());
        binds.push(like);
    }

    // Hide soft-deleted works when deleted_at is present (migration 0002).
    where_parts.push("(w.deleted_at IS NULL)");

    let where_sql = format!("WHERE {}", where_parts.join(" AND "));

    let sql = format!(
        r#"
        SELECT w.*,
          (SELECT i.r2_key FROM images i WHERE i.work_id = w.id ORDER BY i.page_index LIMIT 1) AS cover_key,
          (SELECT GROUP_CONCAT(wt.tag, ',') FROM work_tags wt WHERE wt.work_id = w.id) AS tags
        FROM works w
        {where_sql}
        ORDER BY w.created_at DESC
        LIMIT ? OFFSET ?
        "#
    );

    let mut values: Vec<JsValue> = binds.iter().map(|s| JsValue::from_str(s)).collect();
    values.push(JsValue::from_f64(f64::from(limit)));
    values.push(JsValue::from_f64(f64::from(offset)));

    let db = env.d1("DB")?;
    let rows = db.prepare(&sql).bind(&values)?.all().await?;
    let results = rows.results::<WorkRow>()?;

    let works: Vec<WorkOut> = results
        .into_iter()
        .map(|row| WorkOut {
            id: row.id,
            source: row.source,
            source_id: row.source_id,
            source_url: row.source_url,
            title: row.title,
            author_name: row.author_name,
            author_url: row.author_url,
            is_r18: row.is_r18 != 0,
            page_count: row.page_count,
            origin: row.origin,
            created_at: row.created_at,
            cover_url: cover_url_from_key(row.cover_key.as_deref()),
            tags: tags_from_group_concat(row.tags.as_deref()),
        })
        .collect();

    json_response(
        &json!({
            "ok": true,
            "works": works,
            "limit": limit,
            "offset": offset,
        }),
        200,
    )
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
    origin: String,
    created_at: String,
    cover_key: Option<String>,
    tags: Option<String>,
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
            LIMIT 200
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

#[derive(Debug, serde::Deserialize, Serialize)]
struct CatalogImageOut {
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
    let token = match env.secret("INGEST_TOKEN") {
        Ok(secret) => secret.to_string(),
        Err(_) => env
            .var("INGEST_TOKEN")
            .map(|value| value.to_string())
            .unwrap_or_default(),
    };
    let auth = req.headers().get("Authorization")?;
    let response = match ingest::check_bearer_auth(auth.as_deref(), &token) {
        ingest::AuthCheck::Ok => return Ok(None),
        ingest::AuthCheck::Unauthorized => {
            json_response(&json!({ "ok": false, "error": "unauthorized" }), 401)?
        }
        ingest::AuthCheck::NotConfigured => json_response(
            &json!({ "ok": false, "error": "INGEST_TOKEN not configured" }),
            500,
        )?,
    };
    Ok(Some(with_cors(response)))
}

async fn handle_list_catalog(url: &Url, env: &Env) -> Result<Response> {
    let (limit, offset) = parse_limit_offset(url);
    let values = [
        JsValue::from_f64(f64::from(limit)),
        JsValue::from_f64(f64::from(offset)),
    ];
    let rows = env
        .d1("DB")?
        .prepare(catalog_list_sql())
        .bind(&values)?
        .all()
        .await?
        .results::<CatalogImageOut>()?;
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
    work_id: String,
    chat_id: i64,
    message_ids_json: String,
    publish_state: String,
}

#[derive(Debug, Deserialize)]
struct ActiveWorkRow {
    id: String,
}

#[derive(Debug, Deserialize)]
struct RetractWorkRow {
    id: String,
    deleted_at: Option<String>,
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
            r#"SELECT work_id, chat_id, message_ids_json, publish_state
               FROM telegram_publications WHERE id=?"#,
        )
        .bind(&[JsValue::from_str(&publication_id)])?
        .first::<CatalogPublicationRow>(None)
        .await?
    {
        if same_catalog_publication(&stored, &request) {
            return json_response(
                &json!({
                    "ok": true,
                    "publication_id": publication_id,
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
    db.prepare(
        r#"INSERT INTO telegram_publications
           (id, work_id, chat_id, anchor_message_id, message_ids_json, publish_state, created_at, deleted_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, NULL)"#,
    )
    .bind(&[
        JsValue::from_str(&publication_id),
        JsValue::from_str(&request.work_id),
        JsValue::from_f64(request.chat_id as f64),
        JsValue::from_f64(anchor as f64),
        JsValue::from_str(&message_ids_json),
        JsValue::from_str(&request.publish_state),
        JsValue::from_str(&now),
    ])?
    .run()
    .await?;

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

fn js_iso_now() -> String {
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
        .prepare("SELECT removed_json FROM catalog_prune_receipts WHERE decision_id = ?")
        .bind(&[JsValue::from_str(&request.decision_id)])?
        .first::<CatalogPruneReceiptRow>(None)
        .await?
    {
        let removed: Vec<String> = serde_json::from_str(&receipt.removed_json).unwrap_or_default();
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
            db.prepare("UPDATE works SET page_count=(SELECT COUNT(*) FROM images WHERE work_id=?) WHERE id=?")
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

fn validate_work_id_value(work_id: &str) -> std::result::Result<(), &'static str> {
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

fn same_work_prune_plan(
    keep_work_id: &str,
    remove_work_ids: &[String],
    stored_keep_work_id: &str,
    stored_remove_work_ids: &[String],
) -> bool {
    if keep_work_id != stored_keep_work_id {
        return false;
    }
    let mut left = remove_work_ids.to_vec();
    let mut right = stored_remove_work_ids.to_vec();
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
        if !same_work_prune_plan(
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
    for work_id in &request.remove_work_ids {
        if db
            .prepare("SELECT id FROM works WHERE id=? AND deleted_at IS NULL")
            .bind(&[JsValue::from_str(work_id)])?
            .first::<ActiveWorkRow>(None)
            .await?
            .map(|row| row.id)
            .as_deref()
            != Some(work_id.as_str())
        {
            return json_response(
                &json!({ "ok": false, "error": format!("remove work not active: {work_id}") }),
                409,
            );
        }
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
                r#"SELECT id, work_id, chat_id, message_ids_json
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
            db.prepare("UPDATE works SET deleted_at=? WHERE id=?")
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
    statements.push(
        db.prepare(
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
        ])?,
    );
    db.batch(statements).await?;
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
        .prepare("SELECT id, deleted_at FROM works WHERE id=?")
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
    if work.deleted_at.is_some() {
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
               WHERE i.work_id=? AND w.deleted_at IS NULL
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
    let mut statements = Vec::new();
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
    db.batch(statements).await?;
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

async fn handle_list_tags(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let rows = db.prepare(tags_list_sql()).all().await?;
    let tags = rows.results::<TagRow>()?;
    json_response(&json!({ "ok": true, "tags": tags }), 200)
}

async fn handle_list_sources(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let rows = db
        .prepare(
            r#"
            SELECT source, COUNT(*) AS cnt
            FROM works
            WHERE deleted_at IS NULL
            GROUP BY source
            ORDER BY cnt DESC
            "#,
        )
        .all()
        .await?;
    let sources = rows.results::<SourceRow>()?;
    json_response(&json!({ "ok": true, "sources": sources }), 200)
}

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
    // Always override with immutable long-cache; ignore stored cache_control for media.
    let _ = cache_control;
    pairs.push((
        "Cache-Control".into(),
        "public, max-age=31536000, immutable".into(),
    ));
    // Prevent browsers from MIME-sniffing image responses into executable contexts.
    pairs.push(("X-Content-Type-Options".into(), "nosniff".into()));
    if !etag.is_empty() {
        pairs.push(("etag".into(), etag.to_string()));
    }
    pairs
}

async fn handle_media(key: &str, env: &Env) -> Result<Response> {
    let decoded = urlencoding::decode(key)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| key.to_string());

    let bucket = env.bucket("MEDIA")?;
    let Some(obj) = bucket.get(&decoded).execute().await? else {
        return Response::error("not found", 404);
    };

    let body = obj
        .body()
        .ok_or_else(|| Error::RustError("empty object body".into()))?;
    let response_body = body.response_body()?;

    let meta = obj.http_metadata();
    let pairs = media_header_pairs(
        meta.content_type.as_deref(),
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

    Ok(Response::from_body(response_body)?.with_headers(headers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            origin: "hanabi".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            cover_url: Some("/media/pixiv/1/00.jpg".into()),
            tags: vec!["foo".into(), "bar".into()],
        };
        let v = serde_json::to_value(&work).unwrap();
        assert_eq!(v["id"], "pixiv:1");
        assert_eq!(v["cover_url"], "/media/pixiv/1/00.jpg");
        assert_eq!(v["tags"][0], "foo");
        assert_eq!(v["is_r18"], false);
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
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(
            map.get("X-Content-Type-Options").map(String::as_str),
            Some("nosniff")
        );
        assert_eq!(map.get("etag").map(String::as_str), Some("\"abc\""));
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
        assert!(same_work_prune_plan(
            "pixiv:2",
            &["douyin:1".into()],
            "pixiv:2",
            &["douyin:1".into()]
        ));
        assert!(!same_work_prune_plan(
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
