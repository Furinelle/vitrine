//! Hand-picked image sets (migration 0022). A set's candidates are the images of works tagged
//! with any of its terms; the admin marks each image keep or exclude, and only kept images are
//! published. Excluding only drops an image from the set; deleting it from the gallery is
//! still the image-review flow.

use crate::admin::require_admin_cookie;
use crate::{js_iso_now, json_response, with_cors};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use wasm_bindgen::JsValue;
use worker::*;

/// Set name → tag terms. A work belongs when a tag name, or its Japanese or English
/// display name, contains a term (ASCII case-insensitive), as the gallery's tag search does.
const COLLECTIONS: [(&str, &[&str]); 1] = [("furina", &["芙宁娜", "芙芙", "furina", "フリーナ"])];
const MAX_KEYS: usize = 2000;

#[derive(Deserialize, Serialize)]
struct ImageRow {
    r2_key: String,
    page_index: i64,
    work_id: String,
    title: String,
    author_name: String,
    author_url: String,
    source: String,
    source_url: String,
    created_at: String,
    width: Option<i64>,
    height: Option<i64>,
    #[serde(default)]
    verdict: Option<String>,
}

#[derive(Deserialize)]
struct Decision {
    collection: String,
    r2_keys: Vec<String>,
    /// "keep", "exclude", or null to send the images back to review.
    verdict: Option<String>,
}

fn terms(collection: &str) -> Option<&'static [&'static str]> {
    COLLECTIONS
        .iter()
        .find(|(name, _)| *name == collection)
        .map(|(_, terms)| *terms)
}

const IMAGE_COLUMNS: &str =
    "i.r2_key, i.page_index, w.id AS work_id, w.title, w.author_name, w.author_url, \
     w.source, w.source_url, w.created_at, f.width, f.height";

fn candidates_sql(term_count: usize) -> String {
    let matches =
        vec!["lower(wt.tag) LIKE ? OR lower(t.ja) LIKE ? OR lower(t.en) LIKE ?"; term_count]
            .join(" OR ");
    format!(
        "SELECT {IMAGE_COLUMNS}, c.verdict FROM images i \
         JOIN works w ON w.id = i.work_id \
         LEFT JOIN image_fingerprints f ON f.r2_key = i.r2_key \
         LEFT JOIN image_curation c ON c.collection = ? AND c.r2_key = i.r2_key \
         WHERE w.deleted_at IS NULL AND EXISTS ( \
           SELECT 1 FROM work_tags wt LEFT JOIN tag_items t ON t.name = wt.tag \
           WHERE wt.work_id = w.id AND ({matches})) \
         ORDER BY w.created_at DESC, w.id, i.page_index"
    )
}

const KEPT_SQL: &str =
    "SELECT i.r2_key, i.page_index, w.id AS work_id, w.title, w.author_name, w.author_url, \
     w.source, w.source_url, w.created_at, f.width, f.height \
     FROM image_curation c JOIN images i ON i.r2_key = c.r2_key JOIN works w ON w.id = i.work_id \
     LEFT JOIN image_fingerprints f ON f.r2_key = i.r2_key \
     WHERE c.collection = ? AND c.verdict = 'keep' AND w.deleted_at IS NULL \
     ORDER BY w.created_at DESC, w.id, i.page_index";

fn page(row: &ImageRow) -> Value {
    json!({"url": crate::cover_url_from_key(Some(&row.r2_key)), "width": row.width, "height": row.height})
}

/// Kept images grouped by work, newest work first, pages in order.
fn group_works(rows: Vec<ImageRow>) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut works: HashMap<String, Value> = HashMap::new();
    for row in rows {
        let work = works.entry(row.work_id.clone()).or_insert_with(|| {
            order.push(row.work_id.clone());
            json!({
                "id": row.work_id, "title": row.title, "author_name": row.author_name,
                "author_url": row.author_url, "source": row.source, "source_url": row.source_url,
                "created_at": row.created_at, "pages": [],
            })
        });
        work["pages"].as_array_mut().unwrap().push(page(&row));
    }
    order
        .into_iter()
        .filter_map(|id| works.remove(&id))
        .collect()
}

/// GET /api/curation/{collection}: public list of kept images.
pub async fn handle_public(env: &Env, collection: &str) -> Result<Response> {
    if terms(collection).is_none() {
        return json_response(&json!({"ok": false, "error": "unknown collection"}), 404);
    }
    let rows = env
        .d1("DB")?
        .prepare(KEPT_SQL)
        .bind(&[JsValue::from_str(collection)])?
        .all()
        .await?
        .results::<ImageRow>()?;
    let mut response = json_response(&json!({"ok": true, "works": group_works(rows)}), 200)?;
    response
        .headers_mut()
        .set("Cache-Control", "public, max-age=300")?;
    Ok(response)
}

/// GET /api/admin/curation?collection=: every candidate image with its verdict (null = to review).
pub async fn handle_list(req: Request, env: Env) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let collection = crate::query_param(&req.url()?, "collection");
    let Some(terms) = terms(&collection) else {
        return json_response(&json!({"ok": false, "error": "unknown collection"}), 404);
    };
    let mut binds = vec![JsValue::from_str(&collection)];
    for term in terms {
        let like = JsValue::from_str(&format!("%{}%", term.to_ascii_lowercase()));
        binds.extend([like.clone(), like.clone(), like]);
    }
    let images = env
        .d1("DB")?
        .prepare(candidates_sql(terms.len()))
        .bind(&binds)?
        .all()
        .await?
        .results::<ImageRow>()?
        .into_iter()
        .map(|row| {
            let mut image = page(&row);
            image["r2_key"] = json!(row.r2_key);
            image["page_index"] = json!(row.page_index);
            image["verdict"] = json!(row.verdict);
            image["work"] = json!({
                "id": row.work_id, "title": row.title, "author_name": row.author_name,
                "source": row.source, "source_url": row.source_url, "created_at": row.created_at,
            });
            image
        })
        .collect::<Vec<_>>();
    Ok(with_cors(json_response(
        &json!({"ok": true, "images": images}),
        200,
    )?))
}

/// POST /api/admin/curation: set one verdict on many images in one statement.
pub async fn handle_decide(mut req: Request, env: Env) -> Result<Response> {
    if let Some(response) = require_admin_cookie(&req, &env)? {
        return Ok(response);
    }
    let Ok(decision) = req.json::<Decision>().await else {
        return json_response(&json!({"ok": false, "error": "invalid body"}), 400);
    };
    if terms(&decision.collection).is_none() {
        return json_response(&json!({"ok": false, "error": "unknown collection"}), 404);
    }
    if decision.r2_keys.is_empty()
        || decision.r2_keys.len() > MAX_KEYS
        || decision
            .r2_keys
            .iter()
            .any(|key| key.is_empty() || key.len() > 512)
    {
        return json_response(&json!({"ok": false, "error": "invalid image keys"}), 400);
    }
    let keys = JsValue::from_str(&serde_json::to_string(&decision.r2_keys).unwrap());
    let collection = JsValue::from_str(&decision.collection);
    let statement = match decision.verdict.as_deref() {
        // `WHERE true` keeps SQLite from reading ON CONFLICT as a join constraint.
        Some(verdict @ ("keep" | "exclude")) => env.d1("DB")?
            .prepare("INSERT INTO image_curation(collection, r2_key, verdict, decided_at) \
                SELECT ?, value, ?, ? FROM json_each(?) WHERE true \
                ON CONFLICT(collection, r2_key) DO UPDATE SET verdict = excluded.verdict, decided_at = excluded.decided_at")
            .bind(&[collection, JsValue::from_str(verdict), JsValue::from_str(&js_iso_now()), keys])?,
        None => env.d1("DB")?
            .prepare("DELETE FROM image_curation WHERE collection = ? AND r2_key IN (SELECT value FROM json_each(?))")
            .bind(&[collection, keys])?,
        Some(_) => return json_response(&json!({"ok": false, "error": "invalid verdict"}), 400),
    };
    statement.run().await?;
    Ok(with_cors(json_response(
        &json!({"ok": true, "count": decision.r2_keys.len()}),
        200,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(work_id: &str, page_index: i64) -> ImageRow {
        ImageRow {
            r2_key: format!("x/{work_id}/{page_index:02}.jpg"),
            page_index,
            work_id: work_id.into(),
            title: String::new(),
            author_name: String::new(),
            author_url: String::new(),
            source: "x".into(),
            source_url: String::new(),
            created_at: String::new(),
            width: Some(1200),
            height: None,
            verdict: None,
        }
    }

    #[test]
    fn group_works_keeps_query_order_and_collects_pages() {
        let works = group_works(vec![row("b", 0), row("b", 2), row("a", 1)]);
        assert_eq!(works.len(), 2);
        assert_eq!(works[0]["id"], "b");
        assert_eq!(works[0]["pages"].as_array().unwrap().len(), 2);
        assert_eq!(works[0]["pages"][1]["url"], "/media/x/b/02.jpg");
        assert_eq!(works[1]["pages"][0]["width"], 1200);
        assert!(works[1]["pages"][0]["height"].is_null());
    }

    #[test]
    fn candidates_sql_binds_three_names_per_term() {
        let sql = candidates_sql(4);
        assert_eq!(sql.matches('?').count(), 1 + 4 * 3);
        assert!(sql.contains("w.deleted_at IS NULL"));
    }

    #[test]
    fn only_known_collections() {
        assert!(terms("furina").is_some() && terms("other").is_none());
    }
}
