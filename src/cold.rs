//! Originals moved off R2 to keep the bucket inside the free tier. The Oracle
//! archiver (deploy/oracle-cold) copies the oldest objects to local disk, then
//! asks `handle_archive` to drop the R2 copy; reads fall back here on an R2 miss.

use serde::{Deserialize, Serialize};
use serde_json::json;
use wasm_bindgen::JsValue;
use worker::*;

use crate::{admin, json_response};

const LIST_LIMIT: u32 = 500;

fn object_url(env: &Env, key: &str) -> Result<String> {
    let origin = env.var("COLD_STORE_URL")?.to_string();
    let path = key
        .split('/')
        .map(|part| urlencoding::encode(part).into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("{}/o/{path}", origin.trim_end_matches('/')))
}

async fn send(env: &Env, method: Method, url: &str, body: Option<String>) -> Result<Response> {
    let headers = Headers::new();
    headers.set(
        "Authorization",
        &format!("Bearer {}", admin::env_ingest_token(env)),
    )?;
    if body.is_some() {
        headers.set("Content-Type", "application/json")?;
    }
    let mut init = RequestInit::new();
    init.with_method(method)
        .with_headers(headers)
        .with_body(body.map(|body| JsValue::from_str(&body)));
    Fetch::Request(Request::new_with_init(url, &init)?)
        .send()
        .await
}

/// The cold copy of `key`, or None when the cold store does not have it.
pub async fn get(env: &Env, key: &str, method: Method) -> Result<Option<Response>> {
    let response = send(env, method, &object_url(env, key)?, None).await?;
    match response.status_code() {
        200 => Ok(Some(response)),
        404 => Ok(None),
        status => Err(Error::RustError(format!("cold store HTTP {status}"))),
    }
}

/// Size and stored SHA-256 of the cold copy, for the same checks R2 heads serve.
pub async fn head(env: &Env, key: &str) -> Result<Option<(u64, String)>> {
    let Some(response) = get(env, key, Method::Head).await? else {
        return Ok(None);
    };
    let headers = response.headers();
    let size = headers
        .get("Content-Length")?
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::RustError("cold store returned no size".into()))?;
    Ok(Some((size, headers.get("X-Sha256")?.unwrap_or_default())))
}

pub async fn copy(env: &Env, from: &str, to: &str) -> Result<()> {
    let url = object_url(env, to)?;
    let body = json!({ "from": from }).to_string();
    let status = send(env, Method::Put, &url, Some(body))
        .await?
        .status_code();
    if status != 200 {
        return Err(Error::RustError(format!("cold store copy HTTP {status}")));
    }
    Ok(())
}

/// GET /api/storage/objects?cursor= — one page of the bucket listing.
pub async fn handle_objects(url: &Url, env: &Env) -> Result<Response> {
    let bucket = env.bucket("MEDIA")?;
    let mut list = bucket.list().limit(LIST_LIMIT);
    if let Some((_, cursor)) = url.query_pairs().find(|(name, _)| name == "cursor") {
        list = list.cursor(cursor.into_owned());
    }
    let page = list.execute().await?;
    let objects = page
        .objects()
        .iter()
        .map(|object| {
            json!({
                "key": object.key(),
                "size": object.size(),
                "uploaded": object.uploaded().as_millis(),
                "etag": object.http_etag(),
            })
        })
        .collect::<Vec<_>>();
    json_response(
        &json!({ "ok": true, "objects": objects, "cursor": page.truncated().then(|| page.cursor()).flatten() }),
        200,
    )
}

/// GET /api/storage/object?key= — the R2 bytes the archiver copies to disk.
pub async fn handle_object(url: &Url, env: &Env) -> Result<Response> {
    let Some((_, key)) = url.query_pairs().find(|(name, _)| name == "key") else {
        return json_response(&json!({ "ok": false, "error": "key required" }), 400);
    };
    let Some(object) = env.bucket("MEDIA")?.get(key.as_ref()).execute().await? else {
        return json_response(&json!({ "ok": false, "error": "not found" }), 404);
    };
    let headers = Headers::new();
    if let Some(content_type) = object.http_metadata().content_type {
        headers.set("Content-Type", &content_type)?;
    }
    headers.set("ETag", &object.http_etag())?;
    if let Some(sha256) = object.custom_metadata()?.get("sha256") {
        headers.set("X-Sha256", sha256)?;
    }
    let body = object
        .body()
        .ok_or_else(|| Error::RustError("empty object body".into()))?;
    Ok(Response::from_body(body.response_body()?)?.with_headers(headers))
}

#[derive(Deserialize, Serialize)]
struct ImageRow {
    id: String,
    r2_key: String,
    created_at: String,
}

/// GET /api/storage/images?after_created=&after_id= — gallery originals, oldest
/// first, so the archiver moves the earliest ingests rather than other objects.
pub async fn handle_images(url: &Url, env: &Env) -> Result<Response> {
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    let rows = env
        .d1("DB")?
        .prepare("SELECT id,r2_key,created_at FROM images WHERE (created_at,id)>(?,?) ORDER BY created_at,id LIMIT ?")
        .bind(&[
            JsValue::from_str(&param("after_created")),
            JsValue::from_str(&param("after_id")),
            JsValue::from_f64(LIST_LIMIT as f64),
        ])?
        .all()
        .await?
        .results::<ImageRow>()?;
    let cursor = (rows.len() == LIST_LIMIT as usize)
        .then(|| rows.last())
        .flatten()
        .map(|row| json!({ "created_at": row.created_at, "id": row.id }));
    json_response(
        &json!({ "ok": true, "images": rows, "cursor": cursor }),
        200,
    )
}

#[derive(Deserialize)]
struct ArchiveRequest {
    key: String,
    etag: String,
}

/// POST /api/storage/archive — drop the R2 copy once the cold store holds the
/// same object; the etag guards against a re-upload since the archiver read it.
pub async fn handle_archive(mut req: Request, env: &Env) -> Result<Response> {
    let Ok(request) = req.json::<ArchiveRequest>().await else {
        return json_response(&json!({ "ok": false, "error": "invalid json" }), 400);
    };
    let bucket = env.bucket("MEDIA")?;
    let Some(object) = bucket.head(request.key.as_str()).await? else {
        return json_response(&json!({ "ok": true, "already": true }), 200);
    };
    if object.http_etag() != request.etag {
        return json_response(&json!({ "ok": false, "error": "object changed" }), 409);
    }
    if head(env, &request.key).await?.map(|(size, _)| size) != Some(object.size()) {
        return json_response(&json!({ "ok": false, "error": "cold copy missing" }), 409);
    }
    bucket.delete(request.key).await?;
    json_response(&json!({ "ok": true }), 200)
}
