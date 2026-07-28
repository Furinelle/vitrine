use serde::Serialize;
use serde_json::{json, Value};
use wasm_bindgen::JsValue;
use worker::*;

/// Cloudflare Worker entrypoint for shirogane (Rust / workers-rs).
/// Read-path parity with the existing TypeScript Worker: CORS/OPTIONS,
/// health, works/tags/sources list APIs, media proxy, and ASSETS fallback.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    match handle_request(req, env).await {
        Ok(res) => Ok(res),
        Err(err) => {
            console_error!("shirogane error: {err}");
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
            &json!({ "ok": true, "service": "shirogane" }),
            200,
        )?));
    }

    if path == "/api/works" && method == Method::Get {
        return Ok(with_cors(handle_list_works(&url, &env).await?));
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

    Response::ok("shirogane online")
}

fn with_cors(res: Response) -> Response {
    let headers = res.headers().clone();
    let _ = headers.set("Access-Control-Allow-Origin", "*");
    let _ = headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    let _ = headers.set(
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type",
    );
    res.with_headers(headers)
}

fn json_response(data: &Value, status: u16) -> Result<Response> {
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

    let where_sql = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

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

async fn handle_list_tags(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let rows = db
        .prepare(
            r#"
            SELECT t.name, COUNT(wt.work_id) AS cnt
            FROM tags t
            LEFT JOIN work_tags wt ON wt.tag = t.name
            GROUP BY t.name
            HAVING cnt > 0
            ORDER BY cnt DESC, t.name ASC
            LIMIT 200
            "#,
        )
        .all()
        .await?;
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
            GROUP BY source
            ORDER BY cnt DESC
            "#,
        )
        .all()
        .await?;
    let sources = rows.results::<SourceRow>()?;
    json_response(&json!({ "ok": true, "sources": sources }), 200)
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

    let headers = Headers::new();
    obj.write_http_metadata(headers.clone())?;
    let _ = headers.set("etag", &obj.http_etag());
    let _ = headers.set("Cache-Control", "public, max-age=31536000, immutable");

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
        let v = json!({ "ok": true, "service": "shirogane" });
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "shirogane");
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
}
