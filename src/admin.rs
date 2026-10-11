//! Cookie-authenticated gallery admin: metadata patch and gallery-only hide.

use crate::ingest::{self, constant_time_eq, AuthCheck};
use crate::{json_response, normalize_tag, validate_work_id_value, with_cors};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wasm_bindgen::JsValue;
use worker::*;

pub const COOKIE_NAME: &str = "vitrine_admin";
pub const COOKIE_TTL_SECS: u64 = 7 * 24 * 3600;
const COOKIE_SKEW_SECS: u64 = 60;
const COOKIE_PURPOSE: &str = "vitrine-admin";
pub const HIDE_SQL: &str =
    "UPDATE works SET deleted_at=?,review_version=review_version+1 WHERE id=? AND deleted_at IS NULL";

#[derive(Deserialize)]
struct AdminWorkRow {
    #[serde(flatten)]
    work: crate::WorkRow,
    deleted_at: Option<String>,
    review_version: i64,
}

#[derive(Deserialize)]
struct RestoreImageRow {
    r2_key: String,
    byte_size: i64,
    sha256: String,
}

fn admin_work(row: AdminWorkRow) -> Value {
    let can_restore =
        row.deleted_at.is_some() && row.work.page_count > 0 && row.work.cover_key.is_some();
    let mut work = json!(crate::public_work(row.work));
    work["deleted_at"] = json!(row.deleted_at);
    work["can_restore"] = json!(can_restore);
    work["review_version"] = json!(row.review_version);
    work
}

pub async fn handle_list(req: Request, env: Env) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let url = req.url()?;
    let (limit, offset) = crate::parse_limit_offset(&url);
    let status = crate::query_param(&url, "status");
    let mut predicates = vec![match status.as_str() {
        "" | "active" => "w.deleted_at IS NULL",
        "hidden" => "w.deleted_at IS NOT NULL",
        _ => return reject(400, "invalid work status"),
    }];
    let source = crate::query_param(&url, "source").to_lowercase();
    let q = crate::query_param(&url, "q");
    if q.len() > 512 || source.len() > 64 {
        return reject(400, "query too long");
    }
    let mut values = Vec::new();
    if !source.is_empty() {
        predicates.push("w.source=?");
        values.push(JsValue::from_str(&source));
    }
    if !q.is_empty() {
        predicates.push("(w.title LIKE ? OR w.author_name LIKE ? OR w.source_id LIKE ?)");
        let like = JsValue::from_str(&format!("%{q}%"));
        values.extend([like.clone(), like.clone(), like]);
    }
    values.extend([
        JsValue::from_f64(limit as f64),
        JsValue::from_f64(offset as f64),
    ]);
    let sql = format!(
        "{} WHERE {} ORDER BY w.created_at DESC,w.id DESC LIMIT ? OFFSET ?",
        crate::WORK_SELECT,
        predicates.join(" AND ")
    );
    let works = env
        .d1("DB")?
        .prepare(&sql)
        .bind(&values)?
        .all()
        .await?
        .results::<AdminWorkRow>()?
        .into_iter()
        .map(admin_work)
        .collect::<Vec<_>>();
    Ok(with_cors(json_response(
        &json!({"ok": true, "works": works, "limit": limit, "offset": offset}),
        200,
    )?))
}

pub async fn handle_detail(req: Request, env: Env, work_id: &str) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let Ok(work_id) = urlencoding::decode(work_id) else {
        return reject(400, "invalid work id");
    };
    if validate_work_id_value(&work_id).is_err() {
        return reject(400, "invalid work id");
    }
    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare(format!("{} WHERE w.id=?", crate::WORK_SELECT))
        .bind(&[JsValue::from_str(&work_id)])?
        .first::<AdminWorkRow>(None)
        .await?
    else {
        return reject(404, "work not found");
    };
    let images = db.prepare("SELECT r2_key,content_type,page_index,byte_size FROM images WHERE work_id=? ORDER BY page_index")
        .bind(&[JsValue::from_str(&work_id)])?.all().await?.results::<crate::PublicImage>()?;
    Ok(with_cors(json_response(
        &json!({"ok": true, "work": admin_work(work), "images": images}),
        200,
    )?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPatch {
    pub title: Option<String>,
    pub author_name: Option<String>,
    pub author_url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub is_r18: Option<bool>,
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

#[derive(Deserialize)]
struct WorkEditRow {
    id: String,
    title: String,
    author_name: String,
    author_url: String,
    is_r18: i64,
}

#[derive(Deserialize)]
struct HideRow {
    id: String,
    deleted_at: Option<String>,
}

#[derive(Deserialize)]
struct TagRow {
    tag: String,
}

pub fn env_ingest_token(env: &Env) -> String {
    env.secret("INGEST_TOKEN")
        .map(|value| value.to_string())
        .or_else(|_| env.var("INGEST_TOKEN").map(|value| value.to_string()))
        .unwrap_or_default()
}

pub fn is_mutating_method(method: Method) -> bool {
    matches!(
        method,
        Method::Post | Method::Put | Method::Patch | Method::Delete
    )
}

pub fn sign_cookie(token: &str, expiry: u64) -> String {
    let sig = cookie_signature(token.as_bytes(), expiry);
    format!("{expiry}.{sig}")
}

// HMAC-SHA256 using the SHA256 dependency already used for image fingerprints.
fn cookie_signature(token: &[u8], expiry: u64) -> String {
    let mut key = [0u8; 64];
    if token.len() > key.len() {
        key[..32].copy_from_slice(&Sha256::digest(token));
    } else {
        key[..token.len()].copy_from_slice(token);
    }
    let mut inner = Sha256::new();
    inner.update(key.map(|b| b ^ 0x36));
    inner.update(format!("{COOKIE_PURPOSE}\n{expiry}").as_bytes());
    let mut outer = Sha256::new();
    outer.update(key.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    hex::encode(outer.finalize())
}

pub fn verify_cookie(token: &str, value: &str, now: u64) -> bool {
    if token.is_empty() {
        return false;
    }
    let Some((expiry_s, sig)) = value.split_once('.') else {
        return false;
    };
    if sig.contains('.') {
        return false;
    }
    let Ok(expiry) = expiry_s.parse::<u64>() else {
        return false;
    };
    if expiry <= now {
        return false;
    }
    if expiry
        > now
            .saturating_add(COOKIE_TTL_SECS)
            .saturating_add(COOKIE_SKEW_SECS)
    {
        return false;
    }
    let expected = cookie_signature(token.as_bytes(), expiry);
    constant_time_eq(sig.as_bytes(), expected.as_bytes())
}

pub fn cookie_value(header: Option<&str>, name: &str) -> Option<String> {
    let header = header?;
    for part in header.split(';') {
        let part = part.trim();
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

pub fn set_cookie_header(value: &str, max_age: u64, secure: bool) -> String {
    let mut header =
        format!("{COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        header.push_str("; Secure");
    }
    header
}

pub fn origin_allowed(origin: Option<&str>, request_origin: &str) -> bool {
    matches!(origin, Some(value) if value != "null" && value == request_origin)
}

pub fn origin_matches_request(req: &Request) -> Result<bool> {
    let expected = req.url()?.origin().ascii_serialization();
    Ok(request_origin_allowed(
        req.headers().get("Origin")?.as_deref(),
        req.headers().get("Referer")?.as_deref(),
        &expected,
    ))
}

fn request_origin_allowed(origin: Option<&str>, referer: Option<&str>, expected: &str) -> bool {
    if origin.is_some() {
        return origin_allowed(origin, expected);
    }
    let Some(referer) = referer else {
        return false;
    };
    let Ok(referer_url) = Url::parse(referer) else {
        return false;
    };
    origin_allowed(Some(&referer_url.origin().ascii_serialization()), expected)
}

pub fn request_has_valid_cookie(req: &Request, token: &str) -> Result<bool> {
    let header = req.headers().get("Cookie")?;
    let Some(value) = cookie_value(header.as_deref(), COOKIE_NAME) else {
        return Ok(false);
    };
    Ok(verify_cookie(token, &value, unix_now()))
}

pub fn parse_work_patch(value: &Value) -> std::result::Result<WorkPatch, &'static str> {
    let Value::Object(map) = value else {
        return Err("invalid json");
    };
    Ok(WorkPatch {
        title: optional_capped(map.get("title"), "title", ingest::MAX_TITLE_LEN)?,
        author_name: optional_capped(
            map.get("author_name"),
            "author_name",
            ingest::MAX_AUTHOR_NAME_LEN,
        )?,
        author_url: optional_capped(
            map.get("author_url"),
            "author_url",
            ingest::MAX_AUTHOR_URL_LEN,
        )?,
        tags: optional_tags(map.get("tags"))?,
        is_r18: optional_bool(map.get("is_r18"), "is_r18")?,
    })
}

fn optional_capped(
    value: Option<&Value>,
    field: &'static str,
    max: usize,
) -> std::result::Result<Option<String>, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.len() > max {
                return Err(match field {
                    "title" => "title too long",
                    "author_name" => "author_name too long",
                    "author_url" => "author_url too long",
                    _ => "meta field too long",
                });
            }
            Ok(Some(s.clone()))
        }
        Some(_) => Err(match field {
            "title" => "invalid title",
            "author_name" => "invalid author_name",
            "author_url" => "invalid author_url",
            _ => "invalid meta field",
        }),
    }
}

fn optional_bool(
    value: Option<&Value>,
    field: &'static str,
) -> std::result::Result<Option<bool>, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => Ok(Some(i != 0)),
            None => Err("invalid is_r18"),
        },
        Some(_) => Err(if field == "is_r18" {
            "invalid is_r18"
        } else {
            "invalid meta field"
        }),
    }
}

fn optional_tags(value: Option<&Value>) -> std::result::Result<Option<Vec<String>>, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    Value::String(s) => {
                        let tag = normalize_tag(s);
                        if !tag.is_empty() && !out.contains(&tag) {
                            out.push(tag);
                        }
                    }
                    Value::Null => {}
                    _ => return Err("invalid tags"),
                }
            }
            out.truncate(40);
            Ok(Some(out))
        }
        Some(_) => Err("invalid tags"),
    }
}

fn unix_now() -> u64 {
    (js_sys::Date::now() / 1000.0).floor() as u64
}

fn request_is_https(req: &Request) -> bool {
    req.url()
        .map(|url| url.scheme() == "https")
        .unwrap_or(false)
}

fn with_cookie(mut res: Response, cookie: &str) -> Result<Response> {
    res.headers_mut().set("Set-Cookie", cookie)?;
    Ok(res)
}

fn reject(status: u16, error: &str) -> Result<Response> {
    Ok(with_cors(json_response(
        &json!({ "ok": false, "error": error }),
        status,
    )?))
}

pub(crate) fn require_admin_cookie(req: &Request, env: &Env) -> Result<Option<Response>> {
    let token = env_ingest_token(env);
    match ingest::check_bearer_auth(None, &token) {
        AuthCheck::NotConfigured => {
            return Ok(Some(reject(500, "INGEST_TOKEN not configured")?));
        }
        AuthCheck::Ok | AuthCheck::Unauthorized => {}
    }
    if !request_has_valid_cookie(req, &token)? {
        return Ok(Some(reject(401, "unauthorized")?));
    }
    if is_mutating_method(req.method()) && !origin_matches_request(req)? {
        return Ok(Some(reject(403, "invalid origin")?));
    }
    Ok(None)
}

pub async fn serve_page(req: Request, env: Env) -> Result<Response> {
    let mut url = req.url()?;
    url.set_path("/admin.html");
    url.set_query(None);
    match env.assets("ASSETS") {
        Ok(assets) => {
            let response = assets
                .fetch_request(Request::new(url.as_str(), Method::Get)?)
                .await?;
            let headers = response.headers().clone();
            headers.set("Cache-Control", "no-store")?;
            Ok(response.with_headers(headers))
        }
        Err(_) => Response::error("not found", 404),
    }
}

pub async fn handle_login(mut req: Request, env: Env) -> Result<Response> {
    if !origin_matches_request(&req)? {
        return reject(403, "invalid origin");
    }
    let token = env_ingest_token(&env);
    if token.is_empty() {
        return reject(500, "INGEST_TOKEN not configured");
    }
    let body: LoginBody = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid json"),
    };
    if !constant_time_eq(body.token.as_bytes(), token.as_bytes()) {
        return reject(401, "unauthorized");
    }
    let expiry = unix_now().saturating_add(COOKIE_TTL_SECS);
    let cookie = set_cookie_header(
        &sign_cookie(&token, expiry),
        COOKIE_TTL_SECS,
        request_is_https(&req),
    );
    with_cookie(
        with_cors(json_response(&json!({ "ok": true }), 200)?),
        &cookie,
    )
}

pub async fn handle_logout(req: Request, _env: Env) -> Result<Response> {
    if !origin_matches_request(&req)? {
        return reject(403, "invalid origin");
    }
    let cookie = set_cookie_header("", 0, request_is_https(&req));
    with_cookie(
        with_cors(json_response(&json!({ "ok": true }), 200)?),
        &cookie,
    )
}

pub async fn handle_session(req: Request, env: Env) -> Result<Response> {
    let token = env_ingest_token(&env);
    if token.is_empty() {
        return reject(500, "INGEST_TOKEN not configured");
    }
    let authenticated = request_has_valid_cookie(&req, &token)?;
    Ok(with_cors(json_response(
        &json!({ "ok": true, "authenticated": authenticated }),
        200,
    )?))
}

pub async fn handle_patch(mut req: Request, env: Env, work_id: &str) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let Ok(work_id) = urlencoding::decode(work_id) else {
        return reject(400, "invalid work id");
    };
    if validate_work_id_value(&work_id).is_err() {
        return reject(400, "invalid work id");
    }
    let body: Value = match req.text().await {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return reject(400, "invalid json"),
        },
        Err(_) => return reject(400, "invalid json"),
    };
    let patch = match parse_work_patch(&body) {
        Ok(patch) => patch,
        Err(message) => return reject(400, message),
    };

    let db = env.d1("DB")?;
    let Some(mut work) = db
        .prepare(
            "SELECT id, title, author_name, author_url, is_r18 FROM works WHERE id=? AND deleted_at IS NULL",
        )
        .bind(&[JsValue::from_str(&work_id)])?
        .first::<WorkEditRow>(None)
        .await?
    else {
        return reject(404, "work not found");
    };
    if let Some(title) = patch.title {
        work.title = title;
    }
    if let Some(author_name) = patch.author_name {
        work.author_name = author_name;
    }
    if let Some(author_url) = patch.author_url {
        work.author_url = author_url;
    }
    if let Some(is_r18) = patch.is_r18 {
        work.is_r18 = if is_r18 { 1 } else { 0 };
    }

    let current_tags = db
        .prepare("SELECT tag FROM work_tags WHERE work_id=?")
        .bind(&[JsValue::from_str(&work.id)])?
        .all()
        .await?
        .results::<TagRow>()?
        .into_iter()
        .map(|row| row.tag)
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    let tags = patch.tags.clone().unwrap_or_else(|| current_tags.clone());

    let mut statements = vec![db
        .prepare(
            "UPDATE works SET title=?,author_name=?,author_url=?,is_r18=? WHERE id=? AND deleted_at IS NULL",
        )
        .bind(&[
            JsValue::from_str(&work.title),
            JsValue::from_str(&work.author_name),
            JsValue::from_str(&work.author_url),
            JsValue::from_f64(work.is_r18 as f64),
            JsValue::from_str(&work.id),
        ])?];

    if let Some(new_tags) = patch.tags {
        statements.push(
            db.prepare(
                "DELETE FROM work_tags WHERE work_id=? AND EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL)",
            )
            .bind(&[JsValue::from_str(&work.id), JsValue::from_str(&work.id)])?,
        );
        for tag in &new_tags {
            statements.push(
                db.prepare(
                    "INSERT INTO tags (name, use_count) VALUES (?, 0) ON CONFLICT(name) DO NOTHING",
                )
                .bind(&[JsValue::from_str(tag)])?,
            );
            statements.push(
                db.prepare(
                    "INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT ?, ? WHERE EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL)",
                )
                .bind(&[
                    JsValue::from_str(&work.id),
                    JsValue::from_str(tag),
                    JsValue::from_str(&work.id),
                ])?,
            );
        }
        let mut recount = current_tags;
        for tag in &new_tags {
            if !recount.contains(tag) {
                recount.push(tag.clone());
            }
        }
        for tag in recount {
            statements.push(
                db.prepare(
                    r#"UPDATE tags SET use_count=(
                         SELECT COUNT(*) FROM work_tags WHERE tag=?
                       ) WHERE name=?"#,
                )
                .bind(&[JsValue::from_str(&tag), JsValue::from_str(&tag)])?,
            );
        }
    }
    db.batch(statements).await?;
    if db
        .prepare("SELECT id, deleted_at FROM works WHERE id=? AND deleted_at IS NULL")
        .bind(&[JsValue::from_str(&work.id)])?
        .first::<HideRow>(None)
        .await?
        .is_none()
    {
        return reject(404, "work not found");
    }
    Ok(with_cors(json_response(
        &json!({
            "ok": true,
            "work_id": work.id,
            "title": work.title,
            "author_name": work.author_name,
            "author_url": work.author_url,
            "tags": tags,
            "is_r18": work.is_r18 != 0,
        }),
        200,
    )?))
}

pub async fn handle_hide(req: Request, env: Env, work_id: &str) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let Ok(work_id) = urlencoding::decode(work_id) else {
        return reject(400, "invalid work id");
    };
    if validate_work_id_value(&work_id).is_err() {
        return reject(400, "invalid work id");
    }
    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare("SELECT id, deleted_at FROM works WHERE id=?")
        .bind(&[JsValue::from_str(&work_id)])?
        .first::<HideRow>(None)
        .await?
    else {
        return reject(404, "work not found");
    };
    if work.deleted_at.is_some() {
        return Ok(with_cors(json_response(
            &json!({ "ok": true, "work_id": work.id, "replayed": true }),
            200,
        )?));
    }
    let now = crate::js_iso_now();
    let updated = db
        .prepare(HIDE_SQL)
        .bind(&[JsValue::from_str(&now), JsValue::from_str(&work.id)])?
        .run()
        .await?;
    if updated.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        let again = db
            .prepare("SELECT id, deleted_at FROM works WHERE id=?")
            .bind(&[JsValue::from_str(&work.id)])?
            .first::<HideRow>(None)
            .await?;
        return match again {
            Some(row) if row.deleted_at.is_some() => Ok(with_cors(json_response(
                &json!({ "ok": true, "work_id": row.id, "replayed": true }),
                200,
            )?)),
            Some(_) => reject(409, "work changed; retry hide"),
            None => reject(404, "work not found"),
        };
    }
    Ok(with_cors(json_response(
        &json!({ "ok": true, "work_id": work.id, "replayed": false }),
        200,
    )?))
}

pub async fn handle_restore(req: Request, env: Env, work_id: &str) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let Ok(work_id) = urlencoding::decode(work_id) else {
        return reject(400, "invalid work id");
    };
    if validate_work_id_value(&work_id).is_err() {
        return reject(400, "invalid work id");
    }
    let db = env.d1("DB")?;
    let Some(work) = db
        .prepare(format!("{} WHERE w.id=?", crate::WORK_SELECT))
        .bind(&[JsValue::from_str(&work_id)])?
        .first::<AdminWorkRow>(None)
        .await?
    else {
        return reject(404, "work not found");
    };
    let Some(deleted_at) = work.deleted_at else {
        return Ok(with_cors(json_response(
            &json!({"ok": true, "work_id": work.work.id, "replayed": true}),
            200,
        )?));
    };
    let images = db
        .prepare("SELECT r2_key,byte_size,sha256 FROM images WHERE work_id=? ORDER BY page_index")
        .bind(&[JsValue::from_str(&work_id)])?
        .all()
        .await?
        .results::<RestoreImageRow>()?;
    if images.is_empty() || images.len() as i64 != work.work.page_count {
        return reject(409, "work images were removed; cannot restore");
    }
    let bucket = env.bucket("MEDIA")?;
    for image in &images {
        let Some(original) = bucket.head(image.r2_key.as_str()).await? else {
            return reject(409, "image originals are missing; cannot restore");
        };
        if (image.byte_size > 0 && original.size() != image.byte_size as u64)
            || (!image.sha256.is_empty()
                && original
                    .custom_metadata()?
                    .get("sha256")
                    .is_some_and(|sha| sha != &image.sha256))
        {
            return reject(409, "image originals changed; cannot restore");
        }
    }
    let keys = images
        .iter()
        .map(|image| image.r2_key.as_str())
        .collect::<Vec<_>>();
    let updated = db.prepare(
        "UPDATE works SET deleted_at=NULL,review_version=review_version+1 WHERE id=? AND deleted_at=? AND review_version=? AND page_count=? AND (SELECT COUNT(*) FROM images WHERE work_id=works.id)=? AND NOT EXISTS(SELECT 1 FROM images WHERE work_id=works.id AND r2_key NOT IN(SELECT value FROM json_each(?)))"
    ).bind(&[
        JsValue::from_str(&work_id), JsValue::from_str(&deleted_at), JsValue::from_f64(work.review_version as f64),
        JsValue::from_f64(work.work.page_count as f64), JsValue::from_f64(images.len() as f64), JsValue::from_str(&serde_json::to_string(&keys)?),
    ])?.run().await?;
    if updated.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        let current = db
            .prepare("SELECT id,deleted_at FROM works WHERE id=?")
            .bind(&[JsValue::from_str(&work_id)])?
            .first::<HideRow>(None)
            .await?;
        if current.is_none_or(|current| current.deleted_at.is_some()) {
            return reject(409, "work changed; refresh before restoring");
        }
        return Ok(with_cors(json_response(
            &json!({"ok": true, "work_id": work_id, "replayed": true}),
            200,
        )?));
    }
    Ok(with_cors(json_response(
        &json!({"ok": true, "work_id": work_id, "replayed": false}),
        200,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cookie_roundtrip_accepts_valid_signature() {
        let now = 1_900_000_000;
        let value = sign_cookie("secret", now + COOKIE_TTL_SECS);
        assert!(verify_cookie("secret", &value, now));
    }

    #[test]
    fn cookie_hmac_matches_independent_vectors() {
        assert_eq!(
            cookie_signature(b"secret", 1_900_604_800),
            "17acf75969bc5001a6fb79e29ece8f26cf4995977e873f97ba04412667b51c0b"
        );
        assert_eq!(
            cookie_signature(&[b'x'; 100], 1_900_604_800),
            "c6dddbadafff666e78e2e6e9921a6981cf1d0a2857f684c8f4a0dbefe1b9845f"
        );
    }

    #[test]
    fn cookie_rejects_wrong_token_expired_and_tampered() {
        let now = 1_900_000_000;
        let value = sign_cookie("secret", now + COOKIE_TTL_SECS);
        assert!(!verify_cookie("other", &value, now));
        assert!(!verify_cookie("secret", &value, now + COOKIE_TTL_SECS + 1));
        assert!(!verify_cookie("secret", "not-a-cookie", now));
        assert!(!verify_cookie(
            "secret",
            &format!("{}.ab", now + COOKIE_TTL_SECS),
            now
        ));
        let far = sign_cookie("secret", now + COOKIE_TTL_SECS + COOKIE_SKEW_SECS + 1);
        assert!(!verify_cookie("secret", &far, now));
        assert!(!verify_cookie("", &value, now));
    }

    #[test]
    fn cookie_header_parse_and_set_flags() {
        assert_eq!(
            cookie_value(Some("a=1; vitrine_admin=exp.sig; b=2"), COOKIE_NAME).as_deref(),
            Some("exp.sig")
        );
        assert_eq!(cookie_value(Some("other=1"), COOKIE_NAME), None);
        let https = set_cookie_header("exp.sig", COOKIE_TTL_SECS, true);
        assert!(https.contains("HttpOnly"));
        assert!(https.contains("SameSite=Lax"));
        assert!(https.contains("Path=/"));
        assert!(https.contains("Secure"));
        let http = set_cookie_header("exp.sig", COOKIE_TTL_SECS, false);
        assert!(!http.contains("Secure"));
    }

    #[test]
    fn patch_parser_normalizes_and_caps() {
        let patch = parse_work_patch(&json!({
            "title": "hello",
            "tags": ["#foo bar", "foo_bar", ""],
            "is_r18": true
        }))
        .unwrap();
        assert_eq!(patch.title.as_deref(), Some("hello"));
        assert_eq!(patch.tags, Some(vec!["foo_bar".into()]));
        assert_eq!(patch.is_r18, Some(true));
        assert!(parse_work_patch(&json!({})).unwrap().title.is_none());
        assert_eq!(
            parse_work_patch(&json!({ "title": "t".repeat(ingest::MAX_TITLE_LEN + 1) }))
                .unwrap_err(),
            "title too long"
        );
        assert_eq!(
            parse_work_patch(&json!({ "tags": "nope" })).unwrap_err(),
            "invalid tags"
        );
        assert_eq!(
            parse_work_patch(&json!({ "is_r18": "yes" })).unwrap_err(),
            "invalid is_r18"
        );
        assert_eq!(parse_work_patch(&json!("x")).unwrap_err(), "invalid json");
        assert!(parse_work_patch(&json!({ "tags": [] })).unwrap().tags == Some(vec![]));
    }

    #[test]
    fn hide_sql_is_gallery_only() {
        let sql = HIDE_SQL.to_ascii_lowercase();
        assert!(sql.contains("deleted_at"));
        assert!(sql.contains("review_version"));
        assert!(!sql.contains("telegram"));
        assert!(!sql.contains("delete from"));
        assert!(!sql.contains("images"));
        assert!(!sql.contains("work_tags"));
        assert!(!sql.contains("r2"));
    }

    #[test]
    fn mutating_methods_include_patch() {
        assert!(is_mutating_method(Method::Post));
        assert!(is_mutating_method(Method::Patch));
        assert!(is_mutating_method(Method::Put));
        assert!(!is_mutating_method(Method::Get));
    }

    #[test]
    fn origin_allowed_rejects_missing_null_and_mismatch() {
        let host = "https://gallery.fontaine.blue";
        assert!(!origin_allowed(None, host));
        assert!(!origin_allowed(Some("null"), host));
        assert!(!origin_allowed(Some("https://evil.example"), host));
        assert!(origin_allowed(Some(host), host));
        let referer = Some("https://gallery.fontaine.blue/admin");
        assert!(request_origin_allowed(None, referer, host));
        assert!(!request_origin_allowed(Some("null"), referer, host));
        assert!(!request_origin_allowed(
            Some("https://evil.example"),
            referer,
            host
        ));
        assert!(!request_origin_allowed(
            None,
            Some("https://evil.example/admin"),
            host
        ));
    }
}
