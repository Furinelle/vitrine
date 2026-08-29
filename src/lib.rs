mod ingest;

use serde::Serialize;
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
    let _ = headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
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

}
