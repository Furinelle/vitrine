use super::*;

#[derive(Deserialize)]
pub(super) struct ImageRequest {
    decision_id: String,
    r2_key: String,
    action: String,
    #[serde(default)]
    restored: Vec<RestoredTarget>,
}

#[derive(Deserialize)]
struct RestoredTarget {
    publication_id: String,
    message_id: i64,
}

#[derive(Clone, Serialize, Deserialize)]
struct ImageRow {
    id: String,
    work_id: String,
    page_index: i64,
    r2_key: String,
    content_type: String,
    byte_size: i64,
    created_at: String,
    sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct ImageTarget {
    publication_id: String,
    chat_id: i64,
    message_id: i64,
    // The expected remaining IDs let undo reject a replaced publication.
    remaining: Vec<i64>,
    position: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    image: ImageRow,
    targets: Vec<ImageTarget>,
    backup_key: String,
    #[serde(default)]
    review_version: Option<i64>,
}

#[derive(Deserialize)]
struct Receipt {
    payload: String,
    state: String,
}

#[derive(Deserialize)]
struct Publication {
    id: String,
    chat_id: i64,
    message_ids_json: String,
    publish_state: String,
    deleted_at: Option<String>,
}

#[derive(Deserialize)]
struct WorkState {
    review_version: i64,
    deleted_at: Option<String>,
}

async fn snapshot_is_current(db: &D1Database, snapshot: &Snapshot, state: &str) -> Result<bool> {
    let image = &snapshot.image;
    let work = db
        .prepare("SELECT review_version,deleted_at FROM works WHERE id=?")
        .bind(&[JsValue::from_str(&image.work_id)])?
        .first::<WorkState>(None)
        .await?;
    let last_image_deleted =
        state == "deleted" && snapshot.targets.iter().all(|t| t.remaining.is_empty());
    if !work.is_some_and(|w| {
        Some(w.review_version) == snapshot.review_version
            && w.deleted_at.is_some() == last_image_deleted
    }) {
        return Ok(false);
    }
    let current = db
        .prepare("SELECT * FROM images WHERE id=? OR r2_key=?")
        .bind(&[
            JsValue::from_str(&image.id),
            JsValue::from_str(&image.r2_key),
        ])?
        .first::<ImageRow>(None)
        .await?;
    if state == "deleted" {
        if current.is_some() {
            return Ok(false);
        }
    } else if !current.is_some_and(|i| i.id == image.id && i.r2_key == image.r2_key) {
        return Ok(false);
    }
    if state == "restored" {
        return Ok(true);
    }
    let publications = db.prepare("SELECT id,chat_id,message_ids_json,publish_state,deleted_at FROM telegram_publications WHERE work_id=?")
        .bind(&[JsValue::from_str(&image.work_id)])?.all().await?.results::<Publication>()?;
    let expected_active = snapshot
        .targets
        .iter()
        .filter(|t| state == "prepared" || !t.remaining.is_empty())
        .count();
    if publications
        .iter()
        .filter(|p| p.deleted_at.is_none())
        .count()
        != expected_active
    {
        return Ok(false);
    }
    Ok(snapshot.targets.iter().all(|target| {
        let mut expected = target.remaining.clone();
        if state == "prepared" {
            expected.insert(target.position, target.message_id);
        }
        publications.iter().any(|p| {
            p.id == target.publication_id
                && p.chat_id == target.chat_id
                && p.publish_state == "full"
                && p.deleted_at.is_some() == expected.is_empty()
                && serde_json::from_str::<Vec<i64>>(&p.message_ids_json).ok()
                    == Some(expected.clone())
        })
    }))
}

// The read preflight gives a useful 409; these checks also run inside the mutation's transaction.
fn snapshot_guards(
    db: &D1Database,
    decision_id: &str,
    snapshot: &Snapshot,
    state: &str,
) -> Result<Vec<D1PreparedStatement>> {
    let image = &snapshot.image;
    let deleting = state == "prepared";
    let expected_active = snapshot
        .targets
        .iter()
        .filter(|t| deleting || !t.remaining.is_empty())
        .count();
    let mut statements = vec![db.prepare("UPDATE catalog_image_reviews SET state=CASE WHEN state=? AND EXISTS(SELECT 1 FROM works WHERE id=? AND review_version=? AND (deleted_at IS NOT NULL)=?) AND (SELECT COUNT(*) FROM telegram_publications WHERE work_id=? AND deleted_at IS NULL)=? AND (EXISTS(SELECT 1 FROM images WHERE id=? OR r2_key=?))=? THEN state ELSE 'conflict' END WHERE decision_id=?")
        .bind(&[JsValue::from_str(state), JsValue::from_str(&image.work_id), JsValue::from_f64(snapshot.review_version.unwrap_or(-1) as f64), JsValue::from_f64(if !deleting && expected_active == 0 { 1.0 } else { 0.0 }), JsValue::from_str(&image.work_id), JsValue::from_f64(expected_active as f64), JsValue::from_str(&image.id), JsValue::from_str(&image.r2_key), JsValue::from_f64(if deleting { 1.0 } else { 0.0 }), JsValue::from_str(decision_id)])?];
    if deleting {
        statements.push(db.prepare("UPDATE catalog_image_reviews SET state=CASE WHEN EXISTS(SELECT 1 FROM images WHERE id=? AND r2_key=?) THEN state ELSE 'conflict' END WHERE decision_id=?")
            .bind(&[JsValue::from_str(&image.id), JsValue::from_str(&image.r2_key), JsValue::from_str(decision_id)])?);
    }
    for target in &snapshot.targets {
        let mut expected = target.remaining.clone();
        if deleting {
            expected.insert(target.position, target.message_id);
        }
        statements.push(db.prepare("UPDATE catalog_image_reviews SET state=CASE WHEN EXISTS(SELECT 1 FROM telegram_publications WHERE id=? AND work_id=? AND chat_id=? AND message_ids_json=? AND publish_state='full' AND (deleted_at IS NOT NULL)=?) THEN state ELSE 'conflict' END WHERE decision_id=?")
            .bind(&[JsValue::from_str(&target.publication_id), JsValue::from_str(&image.work_id), JsValue::from_f64(target.chat_id as f64), JsValue::from_str(&serde_json::to_string(&expected).unwrap()), JsValue::from_f64(if expected.is_empty() { 1.0 } else { 0.0 }), JsValue::from_str(decision_id)])?);
    }
    Ok(statements)
}

fn image_target(
    publication: Publication,
    image_count: usize,
    position: usize,
) -> std::result::Result<ImageTarget, &'static str> {
    let mut ids: Vec<i64> = serde_json::from_str(&publication.message_ids_json)
        .map_err(|_| "invalid publication mapping")?;
    if publication.publish_state != "full"
        || ids.len() != image_count
        || position >= ids.len()
        || ids.iter().any(|id| *id <= 0)
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
    {
        return Err("cannot identify this image in the Telegram publication");
    }
    let message_id = ids.remove(position);
    Ok(ImageTarget {
        publication_id: publication.id,
        chat_id: publication.chat_id,
        message_id,
        remaining: ids,
        position,
    })
}

pub(super) async fn handle(req: Request, env: &Env) -> Result<Response> {
    match handle_inner(req, env).await {
        Err(error) if error.to_string().contains("CHECK constraint failed") => json_response(
            &json!({"ok":false,"error":"image review changed concurrently; retry the action"}),
            409,
        ),
        result => result,
    }
}

async fn handle_inner(mut req: Request, env: &Env) -> Result<Response> {
    let request: ImageRequest = match req.json().await {
        Ok(value) => value,
        Err(_) => return json_response(&json!({"ok":false,"error":"invalid json"}), 400),
    };
    if request.decision_id.is_empty()
        || request.decision_id.len() > 160
        || request.r2_key.is_empty()
        || request.r2_key.len() > 1024
        || !matches!(request.action.as_str(), "prepare" | "delete" | "restore")
    {
        return json_response(
            &json!({"ok":false,"error":"invalid image review request"}),
            400,
        );
    }
    let db = env.d1("DB")?;
    let receipt = db
        .prepare("SELECT payload,state FROM catalog_image_reviews WHERE decision_id=?")
        .bind(&[JsValue::from_str(&request.decision_id)])?
        .first::<Receipt>(None)
        .await?;
    let (snapshot, state) = if let Some(receipt) = receipt {
        let snapshot: Snapshot =
            serde_json::from_str(&receipt.payload).map_err(|e| Error::RustError(e.to_string()))?;
        if snapshot.image.r2_key != request.r2_key {
            return json_response(
                &json!({"ok":false,"error":"image review decision conflict"}),
                409,
            );
        }
        (snapshot, receipt.state)
    } else {
        if request.action != "prepare" {
            return json_response(
                &json!({"ok":false,"error":"image review not prepared"}),
                409,
            );
        }
        let image = db.prepare("SELECT i.* FROM images i JOIN works w ON w.id=i.work_id WHERE i.r2_key=? AND w.deleted_at IS NULL")
            .bind(&[JsValue::from_str(&request.r2_key)])?.first::<ImageRow>(None).await?;
        let Some(image) = image else {
            return json_response(&json!({"ok":false,"error":"image not active"}), 409);
        };
        let work = db
            .prepare("SELECT review_version,deleted_at FROM works WHERE id=?")
            .bind(&[JsValue::from_str(&image.work_id)])?
            .first::<WorkState>(None)
            .await?
            .ok_or_else(|| Error::RustError("work disappeared".into()))?;
        let images = db
            .prepare("SELECT * FROM images WHERE work_id=? ORDER BY page_index")
            .bind(&[JsValue::from_str(&image.work_id)])?
            .all()
            .await?
            .results::<ImageRow>()?;
        let position = images
            .iter()
            .position(|i| i.id == image.id)
            .ok_or_else(|| Error::RustError("image disappeared".into()))?;
        let publications = db.prepare("SELECT id,chat_id,message_ids_json,publish_state,deleted_at FROM telegram_publications WHERE work_id=? AND deleted_at IS NULL ORDER BY id")
            .bind(&[JsValue::from_str(&image.work_id)])?.all().await?.results::<Publication>()?;
        if publications.is_empty() {
            return json_response(
                &json!({"ok":false,"error":"telegram publication mapping missing"}),
                409,
            );
        }
        let targets = match publications
            .into_iter()
            .map(|p| image_target(p, images.len(), position))
            .collect::<std::result::Result<Vec<_>, _>>()
        {
            Ok(targets) => targets,
            Err(error) => return json_response(&json!({"ok":false,"error":error}), 409),
        };
        let backup_key = catalog_backup_key(&request.decision_id, &image.r2_key);
        copy_catalog_object(&env.bucket("MEDIA")?, &image.r2_key, &backup_key).await?;
        let snapshot = Snapshot {
            image,
            targets,
            backup_key,
            review_version: Some(work.review_version),
        };
        db.prepare("INSERT INTO catalog_image_reviews(decision_id,payload,state,created_at) VALUES(?,?,'prepared',?) ON CONFLICT DO NOTHING")
            .bind(&[JsValue::from_str(&request.decision_id),JsValue::from_str(&serde_json::to_string(&snapshot).map_err(|e| Error::RustError(e.to_string()))?),JsValue::from_str(&js_iso_now())])?.run().await?;
        let receipt = db
            .prepare("SELECT payload,state FROM catalog_image_reviews WHERE decision_id=?")
            .bind(&[JsValue::from_str(&request.decision_id)])?
            .first::<Receipt>(None)
            .await?
            .ok_or_else(|| Error::RustError("image review disappeared".into()))?;
        let snapshot: Snapshot =
            serde_json::from_str(&receipt.payload).map_err(|e| Error::RustError(e.to_string()))?;
        if snapshot.image.r2_key != request.r2_key {
            return json_response(
                &json!({"ok":false,"error":"image review decision conflict"}),
                409,
            );
        }
        (snapshot, receipt.state)
    };
    if !snapshot_is_current(&db, &snapshot, &state).await? {
        return json_response(
            &json!({"ok":false,"error":"work or publication changed; image review is no longer current"}),
            409,
        );
    }
    if request.action == "prepare" {
        return json_response(
            &json!({"ok":true,"targets":snapshot.targets,"state":state}),
            200,
        );
    }
    if request.action == "delete" && state == "restored" {
        return json_response(
            &json!({"ok":false,"error":"this deletion was already undone"}),
            409,
        );
    }
    let image = &snapshot.image;
    let now = js_iso_now();
    if request.action == "delete" && state == "prepared" {
        let mut statements = snapshot_guards(&db, &request.decision_id, &snapshot, "prepared")?;
        statements.push(
            db.prepare("DELETE FROM images WHERE id=? AND r2_key=?")
                .bind(&[
                    JsValue::from_str(&image.id),
                    JsValue::from_str(&image.r2_key),
                ])?,
        );
        for target in &snapshot.targets {
            statements.push(db.prepare("UPDATE telegram_publications SET message_ids_json=?,anchor_message_id=?,deleted_at=? WHERE id=?")
                .bind(&[JsValue::from_str(&serde_json::to_string(&target.remaining).unwrap()),JsValue::from_f64(target.remaining.first().copied().unwrap_or(target.message_id) as f64),if target.remaining.is_empty() { JsValue::from_str(&now) } else { JsValue::NULL },JsValue::from_str(&target.publication_id)])?);
        }
        statements.push(db.prepare("UPDATE works SET page_count=(SELECT COUNT(*) FROM images WHERE work_id=?),deleted_at=CASE WHEN EXISTS(SELECT 1 FROM images WHERE work_id=?) THEN NULL ELSE ? END WHERE id=?")
            .bind(&[JsValue::from_str(&image.work_id),JsValue::from_str(&image.work_id),JsValue::from_str(&now),JsValue::from_str(&image.work_id)])?);
        statements.push(
            db.prepare("UPDATE catalog_image_reviews SET state='deleted' WHERE decision_id=?")
                .bind(&[JsValue::from_str(&request.decision_id)])?,
        );
        db.batch(statements).await?;
    }
    if request.action == "delete" {
        // Keep the immutable original: D1 controls visibility, so late retries cannot delete a restored image.
        return json_response(
            &json!({"ok":true,"targets":snapshot.targets,"state":"deleted"}),
            200,
        );
    }
    if state == "restored" {
        return json_response(&json!({"ok":true,"state":"restored"}), 200);
    }
    if state == "prepared" {
        // Cancel an uncommitted deletion too; a delayed delete must fail its state guard.
        let mut statements = snapshot_guards(&db, &request.decision_id, &snapshot, "prepared")?;
        statements.push(
            db.prepare("UPDATE catalog_image_reviews SET state='restored' WHERE decision_id=?")
                .bind(&[JsValue::from_str(&request.decision_id)])?,
        );
        db.batch(statements).await?;
        return json_response(&json!({"ok":true,"state":"restored"}), 200);
    }
    let restored = &request.restored;
    if restored.len() != snapshot.targets.len()
        || restored.iter().any(|r| r.message_id <= 0)
        || snapshot.targets.iter().any(|t| {
            restored
                .iter()
                .filter(|r| r.publication_id == t.publication_id)
                .count()
                != 1
                || restored.iter().any(|r| {
                    r.publication_id == t.publication_id && t.remaining.contains(&r.message_id)
                })
        })
    {
        return json_response(
            &json!({"ok":false,"error":"restored Telegram targets are incomplete"}),
            400,
        );
    }
    let mut statements = snapshot_guards(&db, &request.decision_id, &snapshot, "deleted")?;
    for target in &snapshot.targets {
        let mut ids = target.remaining.clone();
        ids.insert(
            target.position,
            restored
                .iter()
                .find(|r| r.publication_id == target.publication_id)
                .unwrap()
                .message_id,
        );
        statements.push(db.prepare("UPDATE telegram_publications SET message_ids_json=?,anchor_message_id=?,deleted_at=NULL WHERE id=?")
            .bind(&[JsValue::from_str(&serde_json::to_string(&ids).unwrap()),JsValue::from_f64(ids[0] as f64),JsValue::from_str(&target.publication_id)])?);
    }
    let bucket = env.bucket("MEDIA")?;
    if bucket.head(&image.r2_key).await?.is_none() {
        copy_catalog_object(&bucket, &snapshot.backup_key, &image.r2_key).await?;
    }
    statements.push(db.prepare("INSERT INTO images(id,work_id,page_index,r2_key,content_type,byte_size,created_at,sha256) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&[JsValue::from_str(&image.id),JsValue::from_str(&image.work_id),JsValue::from_f64(image.page_index as f64),JsValue::from_str(&image.r2_key),JsValue::from_str(&image.content_type),JsValue::from_f64(image.byte_size as f64),JsValue::from_str(&image.created_at),JsValue::from_str(&image.sha256)])?);
    statements.push(db.prepare("UPDATE works SET page_count=(SELECT COUNT(*) FROM images WHERE work_id=?),deleted_at=NULL WHERE id=?").bind(&[JsValue::from_str(&image.work_id),JsValue::from_str(&image.work_id)])?);
    statements.push(
        db.prepare("UPDATE catalog_image_reviews SET state='restored' WHERE decision_id=?")
            .bind(&[JsValue::from_str(&request.decision_id)])?,
    );
    db.batch(statements).await?;
    json_response(&json!({"ok":true,"state":"restored"}), 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delete_targets_only_the_selected_page_and_rejects_ambiguous_albums() {
        let publication = |state: &str, ids: &str| Publication {
            id: "p".into(),
            chat_id: -100,
            message_ids_json: ids.into(),
            publish_state: state.into(),
            deleted_at: None,
        };
        let target = image_target(publication("full", "[11,12,13]"), 3, 1).unwrap();
        assert_eq!(target.message_id, 12);
        assert_eq!(target.remaining, vec![11, 13]);
        assert!(image_target(publication("partial", "[11,13]"), 3, 1).is_err());
        assert!(image_target(publication("full", "[11,13]"), 3, 1).is_err());
    }
}
