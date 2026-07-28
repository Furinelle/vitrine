//! POST /api/ingest — Hanabi-compatible multipart ingest with versioned R2 keys,
//! D1 batch commit, idempotency receipts, and deferred GC of replaced objects.

use crate::{json_response, normalize_tag, with_cors};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use wasm_bindgen::JsValue;
use worker::*;

/// Default max number of files per ingest request.
pub const DEFAULT_MAX_FILE_COUNT: u32 = 40;
/// Default max bytes per single file (25 MiB).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// Default max total bytes across all files (100 MiB).
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

/// Ingest size / count limits (from env or defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestLimits {
    pub max_file_count: u32,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for IngestLimits {
    fn default() -> Self {
        Self {
            max_file_count: DEFAULT_MAX_FILE_COUNT,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

/// Parse limits from env vars; invalid / missing values fall back to defaults.
pub fn limits_from_env(env: &Env) -> IngestLimits {
    IngestLimits {
        max_file_count: env_u32(env, "MAX_FILE_COUNT", DEFAULT_MAX_FILE_COUNT),
        max_file_bytes: env_u64(env, "MAX_FILE_BYTES", DEFAULT_MAX_FILE_BYTES),
        max_total_bytes: env_u64(env, "MAX_TOTAL_BYTES", DEFAULT_MAX_TOTAL_BYTES),
    }
}

fn env_u32(env: &Env, name: &str, default: u32) -> u32 {
    env.var(name)
        .ok()
        .and_then(|v| v.to_string().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

fn env_u64(env: &Env, name: &str, default: u64) -> u64 {
    env.var(name)
        .ok()
        .and_then(|v| v.to_string().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Constant-time equality for secret bytes (length mismatch is not equal).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Always XOR over the max length so short-circuit on length alone is avoided
    // for the common equal-length case; unequal lengths still return false.
    let max = a.len().max(b.len());
    let mut diff: u8 = (a.len() != b.len()) as u8;
    let mut i = 0;
    while i < max {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
        i += 1;
    }
    diff == 0
}

/// Validate Authorization: Bearer <token> using constant-time compare.
pub fn check_bearer_auth(auth_header: Option<&str>, token: &str) -> AuthCheck {
    if token.is_empty() {
        return AuthCheck::NotConfigured;
    }
    let Some(header) = auth_header else {
        return AuthCheck::Unauthorized;
    };
    let expected = format!("Bearer {token}");
    if constant_time_eq(header.as_bytes(), expected.as_bytes()) {
        AuthCheck::Ok
    } else {
        AuthCheck::Unauthorized
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCheck {
    Ok,
    Unauthorized,
    NotConfigured,
}

/// Raw meta JSON shape accepted from Hanabi (strict field validation applied after parse).
#[derive(Debug, Clone, Deserialize)]
pub struct IngestMetaRaw {
    pub source: Option<Value>,
    pub source_id: Option<Value>,
    pub source_url: Option<Value>,
    pub title: Option<Value>,
    pub author_name: Option<Value>,
    pub author_url: Option<Value>,
    pub tags: Option<Value>,
    pub is_r18: Option<Value>,
    pub origin: Option<Value>,
}

/// Validated, normalized ingest meta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMeta {
    pub source: String,
    pub source_id: String,
    pub source_url: String,
    pub title: String,
    pub author_name: String,
    pub author_url: String,
    pub tags: Vec<String>,
    pub is_r18: bool,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaError {
    InvalidJson,
    MissingSource,
    MissingSourceId,
    InvalidSource,
    InvalidSourceId,
    InvalidField(&'static str),
}

impl MetaError {
    pub fn message(&self) -> &'static str {
        match self {
            MetaError::InvalidJson => "invalid meta json",
            MetaError::MissingSource => "source and source_id required",
            MetaError::MissingSourceId => "source and source_id required",
            MetaError::InvalidSource => "invalid source",
            MetaError::InvalidSourceId => "invalid source_id",
            MetaError::InvalidField(f) => match *f {
                "source_url" => "invalid source_url",
                "title" => "invalid title",
                "author_name" => "invalid author_name",
                "author_url" => "invalid author_url",
                "tags" => "invalid tags",
                "is_r18" => "invalid is_r18",
                "origin" => "invalid origin",
                _ => "invalid meta field",
            },
        }
    }
}

/// Strict validation of Hanabi meta JSON text.
pub fn validate_meta_json(raw: &str) -> std::result::Result<ValidatedMeta, MetaError> {
    let parsed: IngestMetaRaw = serde_json::from_str(raw).map_err(|_| MetaError::InvalidJson)?;
    validate_meta(parsed)
}

pub fn validate_meta(raw: IngestMetaRaw) -> std::result::Result<ValidatedMeta, MetaError> {
    let source = opt_string(raw.source, "source")?
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if source.is_empty() {
        return Err(MetaError::MissingSource);
    }
    if !is_safe_source(&source) {
        return Err(MetaError::InvalidSource);
    }

    let source_id = opt_string(raw.source_id, "source_id")?
        .unwrap_or_default()
        .trim()
        .to_string();
    if source_id.is_empty() {
        return Err(MetaError::MissingSourceId);
    }
    if !is_safe_source_id(&source_id) {
        return Err(MetaError::InvalidSourceId);
    }

    let source_url = opt_string(raw.source_url, "source_url")?.unwrap_or_default();
    let title = opt_string(raw.title, "title")?.unwrap_or_default();
    let author_name = opt_string(raw.author_name, "author_name")?.unwrap_or_default();
    let author_url = opt_string(raw.author_url, "author_url")?.unwrap_or_default();
    let origin = opt_string(raw.origin, "origin")?.unwrap_or_default();
    let is_r18 = opt_bool(raw.is_r18, "is_r18")?.unwrap_or(false);
    let tags = opt_tags(raw.tags)?;

    Ok(ValidatedMeta {
        source,
        source_id,
        source_url,
        title,
        author_name,
        author_url,
        tags,
        is_r18,
        origin,
    })
}

fn opt_string(
    v: Option<Value>,
    field: &'static str,
) -> std::result::Result<Option<String>, MetaError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(Value::Bool(b)) => Ok(Some(b.to_string())),
        Some(_) => Err(MetaError::InvalidField(field)),
    }
}

fn opt_bool(v: Option<Value>, field: &'static str) -> std::result::Result<Option<bool>, MetaError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i != 0))
            } else {
                Err(MetaError::InvalidField(field))
            }
        }
        Some(_) => Err(MetaError::InvalidField(field)),
    }
}

fn opt_tags(v: Option<Value>) -> std::result::Result<Vec<String>, MetaError> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    Value::String(s) => {
                        let t = normalize_tag(&s);
                        if !t.is_empty() && !out.contains(&t) {
                            out.push(t);
                        }
                    }
                    Value::Null => {}
                    _ => return Err(MetaError::InvalidField("tags")),
                }
            }
            out.truncate(40);
            Ok(out)
        }
        Some(_) => Err(MetaError::InvalidField("tags")),
    }
}

/// source path segment: lowercase alnum / underscore / hyphen only.
pub fn is_safe_source(source: &str) -> bool {
    !source.is_empty()
        && source.len() <= 64
        && source
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// source_id may not contain path separators or `..`.
pub fn is_safe_source_id(source_id: &str) -> bool {
    !source_id.is_empty()
        && source_id.len() <= 256
        && !source_id.contains('/')
        && !source_id.contains('\\')
        && !source_id.contains("..")
        && !source_id.contains('\0')
}

/// Work primary key: `{source}:{source_id}`.
pub fn work_id(source: &str, source_id: &str) -> String {
    format!("{source}:{source_id}")
}

/// SHA-256 hex of raw bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Stable fingerprint over meta + ordered file digests.
pub fn compute_fingerprint(meta: &ValidatedMeta, file_sha256s: &[String]) -> String {
    let mut hasher = Sha256::new();
    for part in [
        meta.source.as_str(),
        meta.source_id.as_str(),
        meta.source_url.as_str(),
        meta.title.as_str(),
        meta.author_name.as_str(),
        meta.author_url.as_str(),
        if meta.is_r18 { "1" } else { "0" },
        meta.origin.as_str(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.update(meta.tags.join(",").as_bytes());
    hasher.update([0]);
    for dig in file_sha256s {
        hasher.update(dig.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// Guess content-type from filename (Hanabi / TS parity).
pub fn guess_content_type(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "image/jpeg"
    }
}

/// Extension from content-type.
pub fn ext_from_content_type(ct: &str) -> &'static str {
    let lower = ct.to_ascii_lowercase();
    if lower.contains("png") {
        "png"
    } else if lower.contains("webp") {
        "webp"
    } else if lower.contains("gif") {
        "gif"
    } else if lower.contains("bmp") {
        "bmp"
    } else {
        "jpg"
    }
}

/// Immutable versioned R2 key: `source/source_id/<ingest_id>/<page>.ext`.
pub fn versioned_r2_key(
    source: &str,
    source_id: &str,
    ingest_id: &str,
    page: usize,
    ext: &str,
) -> String {
    format!("{source}/{source_id}/{ingest_id}/{:02}.{ext}", page)
}

/// Derive a new ingest id from entropy parts (timestamp, work id, random, fingerprint prefix).
pub fn shape_ingest_id(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p.as_bytes());
        hasher.update([0]);
    }
    // 32 hex chars = 128 bits
    hex::encode(hasher.finalize())[..32].to_string()
}

/// Validate file count / per-file / total byte limits.
pub fn check_file_limits(
    count: usize,
    sizes: &[u64],
    limits: &IngestLimits,
) -> std::result::Result<(), LimitError> {
    if count == 0 {
        return Err(LimitError::NoFiles);
    }
    if count as u32 > limits.max_file_count {
        return Err(LimitError::TooManyFiles {
            count: count as u32,
            max: limits.max_file_count,
        });
    }
    let mut total = 0u64;
    for (i, &sz) in sizes.iter().enumerate() {
        if sz == 0 {
            return Err(LimitError::EmptyFile { index: i });
        }
        if sz > limits.max_file_bytes {
            return Err(LimitError::FileTooLarge {
                index: i,
                bytes: sz,
                max: limits.max_file_bytes,
            });
        }
        total = total.saturating_add(sz);
    }
    if total > limits.max_total_bytes {
        return Err(LimitError::TotalTooLarge {
            bytes: total,
            max: limits.max_total_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitError {
    NoFiles,
    TooManyFiles { count: u32, max: u32 },
    EmptyFile { index: usize },
    FileTooLarge { index: usize, bytes: u64, max: u64 },
    TotalTooLarge { bytes: u64, max: u64 },
}

impl LimitError {
    pub fn message(&self) -> String {
        match self {
            LimitError::NoFiles => "no files".into(),
            LimitError::TooManyFiles { count, max } => {
                format!("too many files: {count} > max {max}")
            }
            LimitError::EmptyFile { index } => format!("empty file at index {index}"),
            LimitError::FileTooLarge { index, bytes, max } => {
                format!("file {index} too large: {bytes} > max {max}")
            }
            LimitError::TotalTooLarge { bytes, max } => {
                format!("total bytes too large: {bytes} > max {max}")
            }
        }
    }
}

/// Build success JSON matching Hanabi/TS contract (+ reliability fields).
pub fn success_response_body(
    work_id: &str,
    tags: &[String],
    images: &[SavedImage],
    ingest_id: &str,
    idempotent_replay: bool,
) -> Value {
    json!({
        "ok": true,
        "work_id": work_id,
        "pages": images.len(),
        "tags": tags,
        "images": images,
        "ingest_id": ingest_id,
        "idempotent": idempotent_replay,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedImage {
    pub page: usize,
    pub r2_key: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct IdempotencyRow {
    fingerprint: String,
    response_json: String,
    status_code: i64,
}

#[derive(Debug, Deserialize)]
struct OldKeyRow {
    r2_key: String,
}

struct PreparedFile {
    page: usize,
    bytes: Vec<u8>,
    content_type: String,
    sha256: String,
    r2_key: String,
    byte_size: u64,
}

/// Entry for POST /api/ingest.
pub async fn handle_ingest(mut req: Request, env: Env) -> Result<Response> {
    let token = match env.secret("INGEST_TOKEN") {
        Ok(s) => s.to_string(),
        Err(_) => match env.var("INGEST_TOKEN") {
            Ok(v) => v.to_string(),
            Err(_) => String::new(),
        },
    };

    match check_bearer_auth(req.headers().get("Authorization")?.as_deref(), &token) {
        AuthCheck::NotConfigured => {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": "INGEST_TOKEN not configured" }),
                500,
            )?));
        }
        AuthCheck::Unauthorized => {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": "unauthorized" }),
                401,
            )?));
        }
        AuthCheck::Ok => {}
    }

    let content_type = req.headers().get("Content-Type")?.unwrap_or_default();
    if !content_type
        .to_ascii_lowercase()
        .contains("multipart/form-data")
    {
        return Ok(with_cors(json_response(
            &json!({ "ok": false, "error": "expected multipart/form-data" }),
            400,
        )?));
    }

    let idem_key = req
        .headers()
        .get("Idempotency-Key")?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref k) = idem_key {
        if k.len() > 256 {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": "Idempotency-Key too long" }),
                400,
            )?));
        }
    }

    let form = req.form_data().await?;
    let meta_raw = match form.get("meta") {
        Some(FormEntry::Field(s)) => s,
        Some(FormEntry::File(_)) => {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": "meta must be a form field" }),
                400,
            )?));
        }
        None => {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": "missing meta field" }),
                400,
            )?));
        }
    };

    let meta = match validate_meta_json(&meta_raw) {
        Ok(m) => m,
        Err(e) => {
            return Ok(with_cors(json_response(
                &json!({ "ok": false, "error": e.message() }),
                400,
            )?));
        }
    };

    let file_entries = form.get_all("files").unwrap_or_default();
    let mut files: Vec<File> = Vec::new();
    for entry in file_entries {
        if let FormEntry::File(f) = entry {
            files.push(f);
        }
    }

    let limits = limits_from_env(&env);
    // Read file bodies first to know sizes/hashes.
    let mut bodies: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let name = f.name();
        let ct = {
            let t = f.type_();
            if t.is_empty() {
                guess_content_type(&name).to_string()
            } else {
                t
            }
        };
        let bytes = f
            .bytes()
            .await
            .map_err(|e| Error::RustError(format!("failed to read file {i}: {e}")))?;
        bodies.push((name, ct, bytes));
    }

    let sizes: Vec<u64> = bodies.iter().map(|(_, _, b)| b.len() as u64).collect();
    if let Err(e) = check_file_limits(bodies.len(), &sizes, &limits) {
        return Ok(with_cors(json_response(
            &json!({ "ok": false, "error": e.message() }),
            400,
        )?));
    }

    let digests: Vec<String> = bodies.iter().map(|(_, _, b)| sha256_hex(b)).collect();
    let fingerprint = compute_fingerprint(&meta, &digests);
    let wid = work_id(&meta.source, &meta.source_id);
    let db = env.d1("DB")?;

    // Idempotency pre-check
    if let Some(ref key) = idem_key {
        if let Some(row) = db
            .prepare(
                "SELECT fingerprint, response_json, status_code FROM idempotency_receipts WHERE idempotency_key = ?",
            )
            .bind(&[JsValue::from_str(key)])?
            .first::<IdempotencyRow>(None)
            .await?
        {
            if row.fingerprint == fingerprint {
                let body: Value = serde_json::from_str(&row.response_json).unwrap_or_else(|_| {
                    json!({ "ok": true, "work_id": wid, "idempotent": true })
                });
                return Ok(with_cors(json_response(&body, row.status_code as u16)?));
            }
            return Ok(with_cors(json_response(
                &json!({
                    "ok": false,
                    "error": "idempotency key reuse with different payload",
                    "conflict": true,
                }),
                409,
            )?));
        }
    }

    let now = js_iso_now();
    let mut entropy = [0u8; 16];
    let _ = getrandom::getrandom(&mut entropy);
    let ingest_id = shape_ingest_id(&[&now, &wid, &fingerprint, &hex::encode(entropy)]);

    let mut prepared: Vec<PreparedFile> = Vec::with_capacity(bodies.len());
    for (i, (_name, ct, bytes)) in bodies.into_iter().enumerate() {
        let ext = ext_from_content_type(&ct);
        let sha = digests[i].clone();
        let key = versioned_r2_key(&meta.source, &meta.source_id, &ingest_id, i, ext);
        let byte_size = bytes.len() as u64;
        prepared.push(PreparedFile {
            page: i,
            bytes,
            content_type: ct,
            sha256: sha,
            r2_key: key,
            byte_size,
        });
    }

    // Collect old image keys before mutation (for orphan/GC tracking).
    let old_rows = db
        .prepare("SELECT r2_key FROM images WHERE work_id = ?")
        .bind(&[JsValue::from_str(&wid)])?
        .all()
        .await?
        .results::<OldKeyRow>()
        .unwrap_or_default();
    let old_keys: Vec<String> = old_rows.into_iter().map(|r| r.r2_key).collect();

    let bucket = env.bucket("MEDIA")?;
    let mut uploaded_keys: Vec<String> = Vec::new();

    for file in &prepared {
        let mut custom = HashMap::new();
        custom.insert("work_id".to_string(), wid.clone());
        custom.insert("page_index".to_string(), file.page.to_string());
        custom.insert("ingest_id".to_string(), ingest_id.clone());
        custom.insert("sha256".to_string(), file.sha256.clone());

        let put = bucket
            .put(file.r2_key.clone(), file.bytes.clone())
            .http_metadata(HttpMetadata {
                content_type: Some(file.content_type.clone()),
                cache_control: Some("public, max-age=31536000, immutable".into()),
                ..Default::default()
            })
            .custom_metadata(custom);

        match put.execute().await {
            Ok(_) => uploaded_keys.push(file.r2_key.clone()),
            Err(e) => {
                best_effort_delete(&bucket, &uploaded_keys).await;
                let _ = write_audit(
                    &db,
                    &wid,
                    &meta,
                    &ingest_id,
                    &fingerprint,
                    idem_key.as_deref(),
                    prepared.len(),
                    prepared.iter().map(|p| p.byte_size).sum(),
                    "failed",
                    Some(&format!("r2 upload failed: {e}")),
                    &now,
                )
                .await;
                return Ok(with_cors(json_response(
                    &json!({ "ok": false, "error": format!("r2 upload failed: {e}") }),
                    500,
                )?));
            }
        }
    }

    // Single D1 batch for work / images / tags / work_tags / orphans / idempotency / audit.
    let saved: Vec<SavedImage> = prepared
        .iter()
        .map(|f| SavedImage {
            page: f.page,
            r2_key: f.r2_key.clone(),
            bytes: f.byte_size,
            sha256: f.sha256.clone(),
        })
        .collect();

    let response_body = success_response_body(&wid, &meta.tags, &saved, &ingest_id, false);
    let response_json = response_body.to_string();
    let total_bytes: u64 = prepared.iter().map(|p| p.byte_size).sum();
    let audit_id = shape_ingest_id(&["audit", &ingest_id, &now]);

    match commit_d1_batch(
        &db,
        &meta,
        &wid,
        &ingest_id,
        &fingerprint,
        idem_key.as_deref(),
        &prepared,
        &old_keys,
        &response_json,
        &audit_id,
        total_bytes,
        &now,
    )
    .await
    {
        Ok(()) => Ok(with_cors(json_response(&response_body, 200)?)),
        Err(e) => {
            best_effort_delete(&bucket, &uploaded_keys).await;
            let _ = write_audit(
                &db,
                &wid,
                &meta,
                &ingest_id,
                &fingerprint,
                idem_key.as_deref(),
                prepared.len(),
                total_bytes,
                "failed",
                Some(&e.to_string()),
                &now,
            )
            .await;
            Ok(with_cors(json_response(
                &json!({ "ok": false, "error": format!("d1 batch failed: {e}") }),
                500,
            )?))
        }
    }
}

async fn best_effort_delete(bucket: &Bucket, keys: &[String]) {
    for key in keys {
        let _ = bucket.delete(key.clone()).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn commit_d1_batch(
    db: &D1Database,
    meta: &ValidatedMeta,
    wid: &str,
    ingest_id: &str,
    fingerprint: &str,
    idem_key: Option<&str>,
    prepared: &[PreparedFile],
    old_keys: &[String],
    response_json: &str,
    audit_id: &str,
    total_bytes: u64,
    now: &str,
) -> Result<()> {
    let mut stmts: Vec<D1PreparedStatement> = Vec::new();

    // Upsert work; clear soft-delete on re-ingest.
    stmts.push(
        db.prepare(
            r#"
            INSERT INTO works (
              id, source, source_id, source_url, title, author_name, author_url,
              is_r18, page_count, origin, created_at, deleted_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
            ON CONFLICT(id) DO UPDATE SET
              source_url=excluded.source_url,
              title=excluded.title,
              author_name=excluded.author_name,
              author_url=excluded.author_url,
              is_r18=excluded.is_r18,
              page_count=excluded.page_count,
              origin=excluded.origin,
              deleted_at=NULL
            "#,
        )
        .bind(&[
            JsValue::from_str(wid),
            JsValue::from_str(&meta.source),
            JsValue::from_str(&meta.source_id),
            JsValue::from_str(&meta.source_url),
            JsValue::from_str(&meta.title),
            JsValue::from_str(&meta.author_name),
            JsValue::from_str(&meta.author_url),
            JsValue::from_f64(if meta.is_r18 { 1.0 } else { 0.0 }),
            JsValue::from_f64(prepared.len() as f64),
            JsValue::from_str(&meta.origin),
            JsValue::from_str(now),
        ])?,
    );

    // Replace image rows (R2 objects for old keys are recorded as orphans, not deleted).
    stmts.push(
        db.prepare("DELETE FROM images WHERE work_id = ?")
            .bind(&[JsValue::from_str(wid)])?,
    );

    for file in prepared {
        let image_id = format!("{wid}#{}", file.page);
        stmts.push(
            db.prepare(
                r#"
                INSERT INTO images (
                  id, work_id, page_index, r2_key, content_type, byte_size, created_at, sha256
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&[
                JsValue::from_str(&image_id),
                JsValue::from_str(wid),
                JsValue::from_f64(file.page as f64),
                JsValue::from_str(&file.r2_key),
                JsValue::from_str(&file.content_type),
                JsValue::from_f64(file.byte_size as f64),
                JsValue::from_str(now),
                JsValue::from_str(&file.sha256),
            ])?,
        );
    }

    // Replace tags.
    stmts.push(
        db.prepare("DELETE FROM work_tags WHERE work_id = ?")
            .bind(&[JsValue::from_str(wid)])?,
    );
    for tag in &meta.tags {
        stmts.push(
            db.prepare(
                r#"
                INSERT INTO tags (name, use_count) VALUES (?, 1)
                ON CONFLICT(name) DO UPDATE SET use_count = use_count + 1
                "#,
            )
            .bind(&[JsValue::from_str(tag)])?,
        );
        stmts.push(
            db.prepare("INSERT OR IGNORE INTO work_tags (work_id, tag) VALUES (?, ?)")
                .bind(&[JsValue::from_str(wid), JsValue::from_str(tag)])?,
        );
    }

    // Record replaced R2 keys for later GC — never delete them inline.
    for key in old_keys {
        if key.is_empty() {
            continue;
        }
        stmts.push(
            db.prepare(
                r#"
                INSERT INTO orphan_objects (r2_key, work_id, reason, created_at, deleted_at)
                VALUES (?, ?, 'replaced', ?, NULL)
                ON CONFLICT(r2_key) DO UPDATE SET
                  work_id=excluded.work_id,
                  reason=excluded.reason,
                  created_at=excluded.created_at,
                  deleted_at=NULL
                "#,
            )
            .bind(&[
                JsValue::from_str(key),
                JsValue::from_str(wid),
                JsValue::from_str(now),
            ])?,
        );
    }

    if let Some(key) = idem_key {
        stmts.push(
            db.prepare(
                r#"
                INSERT INTO idempotency_receipts (
                  idempotency_key, fingerprint, work_id, status_code, response_json, created_at
                ) VALUES (?, ?, ?, 200, ?, ?)
                "#,
            )
            .bind(&[
                JsValue::from_str(key),
                JsValue::from_str(fingerprint),
                JsValue::from_str(wid),
                JsValue::from_str(response_json),
                JsValue::from_str(now),
            ])?,
        );
    }

    stmts.push(
        db.prepare(
            r#"
            INSERT INTO ingest_audits (
              id, work_id, source, source_id, ingest_id, fingerprint, idempotency_key,
              page_count, total_bytes, status, error, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'success', NULL, ?)
            "#,
        )
        .bind(&[
            JsValue::from_str(audit_id),
            JsValue::from_str(wid),
            JsValue::from_str(&meta.source),
            JsValue::from_str(&meta.source_id),
            JsValue::from_str(ingest_id),
            JsValue::from_str(fingerprint),
            match idem_key {
                Some(k) => JsValue::from_str(k),
                None => JsValue::NULL,
            },
            JsValue::from_f64(prepared.len() as f64),
            JsValue::from_f64(total_bytes as f64),
            JsValue::from_str(now),
        ])?,
    );

    db.batch(stmts).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn write_audit(
    db: &D1Database,
    wid: &str,
    meta: &ValidatedMeta,
    ingest_id: &str,
    fingerprint: &str,
    idem_key: Option<&str>,
    page_count: usize,
    total_bytes: u64,
    status: &str,
    error: Option<&str>,
    now: &str,
) -> Result<()> {
    let audit_id = shape_ingest_id(&["audit-fail", ingest_id, status, now]);
    db.prepare(
        r#"
        INSERT INTO ingest_audits (
          id, work_id, source, source_id, ingest_id, fingerprint, idempotency_key,
          page_count, total_bytes, status, error, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&[
        JsValue::from_str(&audit_id),
        JsValue::from_str(wid),
        JsValue::from_str(&meta.source),
        JsValue::from_str(&meta.source_id),
        JsValue::from_str(ingest_id),
        JsValue::from_str(fingerprint),
        match idem_key {
            Some(k) => JsValue::from_str(k),
            None => JsValue::NULL,
        },
        JsValue::from_f64(page_count as f64),
        JsValue::from_f64(total_bytes as f64),
        JsValue::from_str(status),
        match error {
            Some(e) => JsValue::from_str(e),
            None => JsValue::NULL,
        },
        JsValue::from_str(now),
    ])?
    .run()
    .await?;
    Ok(())
}

fn js_iso_now() -> String {
    // Prefer JS Date for Workers runtime ISO strings.
    js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"Secret"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn bearer_auth_checks() {
        assert_eq!(check_bearer_auth(Some("Bearer tok"), "tok"), AuthCheck::Ok);
        assert_eq!(
            check_bearer_auth(Some("Bearer no"), "tok"),
            AuthCheck::Unauthorized
        );
        assert_eq!(check_bearer_auth(None, "tok"), AuthCheck::Unauthorized);
        assert_eq!(
            check_bearer_auth(Some("Bearer x"), ""),
            AuthCheck::NotConfigured
        );
    }

    #[test]
    fn validate_meta_requires_source_fields() {
        let err = validate_meta_json(r#"{"source":"","source_id":"1"}"#).unwrap_err();
        assert_eq!(err, MetaError::MissingSource);

        let err = validate_meta_json(r#"{"source":"pixiv"}"#).unwrap_err();
        assert_eq!(err, MetaError::MissingSourceId);

        let ok = validate_meta_json(
            r##"{"source":"Pixiv","source_id":"123","tags":["#foo bar","foo_bar"],"is_r18":false}"##,
        )
        .unwrap();
        assert_eq!(ok.source, "pixiv");
        assert_eq!(ok.source_id, "123");
        assert_eq!(ok.tags, vec!["foo_bar"]);
    }

    #[test]
    fn validate_meta_rejects_path_injection() {
        assert!(validate_meta_json(r#"{"source":"pix/iv","source_id":"1"}"#).is_err());
        assert!(validate_meta_json(r#"{"source":"pixiv","source_id":"../x"}"#).is_err());
        assert!(validate_meta_json(r#"{"source":"pixiv","source_id":"a/b"}"#).is_err());
        assert!(validate_meta_json(r#"{"source":"pixiv","source_id":"ok","tags":1}"#).is_err());
    }

    #[test]
    fn limits_enforce_count_and_bytes() {
        let limits = IngestLimits {
            max_file_count: 2,
            max_file_bytes: 10,
            max_total_bytes: 15,
        };
        assert_eq!(
            check_file_limits(0, &[], &limits).unwrap_err(),
            LimitError::NoFiles
        );
        assert!(matches!(
            check_file_limits(3, &[1, 1, 1], &limits),
            Err(LimitError::TooManyFiles { .. })
        ));
        assert!(matches!(
            check_file_limits(1, &[11], &limits),
            Err(LimitError::FileTooLarge { .. })
        ));
        assert!(matches!(
            check_file_limits(2, &[10, 10], &limits),
            Err(LimitError::TotalTooLarge { .. })
        ));
        assert!(check_file_limits(2, &[5, 5], &limits).is_ok());
    }

    #[test]
    fn fingerprint_and_keys_stable() {
        let meta = ValidatedMeta {
            source: "pixiv".into(),
            source_id: "99".into(),
            source_url: "https://x".into(),
            title: "t".into(),
            author_name: "a".into(),
            author_url: "".into(),
            tags: vec!["tag".into()],
            is_r18: false,
            origin: "hanabi".into(),
        };
        let fp1 = compute_fingerprint(&meta, &["aa".into(), "bb".into()]);
        let fp2 = compute_fingerprint(&meta, &["aa".into(), "bb".into()]);
        let fp3 = compute_fingerprint(&meta, &["aa".into(), "cc".into()]);
        assert_eq!(fp1, fp2);
        assert_ne!(fp1, fp3);
        assert_eq!(fp1.len(), 64);

        let key = versioned_r2_key("pixiv", "99", "deadbeef", 0, "jpg");
        assert_eq!(key, "pixiv/99/deadbeef/00.jpg");
        assert_eq!(
            versioned_r2_key("pixiv", "99", "deadbeef", 12, "png"),
            "pixiv/99/deadbeef/12.png"
        );

        let id = shape_ingest_id(&["a", "b", "c"]);
        assert_eq!(id.len(), 32);
        assert_eq!(id, shape_ingest_id(&["a", "b", "c"]));
    }

    #[test]
    fn success_response_shape() {
        let images = vec![SavedImage {
            page: 0,
            r2_key: "pixiv/1/ing/00.jpg".into(),
            bytes: 12,
            sha256: "abc".into(),
        }];
        let v = success_response_body("pixiv:1", &["t".into()], &images, "ing", true);
        assert_eq!(v["ok"], true);
        assert_eq!(v["work_id"], "pixiv:1");
        assert_eq!(v["pages"], 1);
        assert_eq!(v["tags"][0], "t");
        assert_eq!(v["images"][0]["r2_key"], "pixiv/1/ing/00.jpg");
        assert_eq!(v["ingest_id"], "ing");
        assert_eq!(v["idempotent"], true);
    }

    #[test]
    fn content_type_and_ext() {
        assert_eq!(guess_content_type("a.PNG"), "image/png");
        assert_eq!(guess_content_type("a.webp"), "image/webp");
        assert_eq!(guess_content_type("a"), "image/jpeg");
        assert_eq!(ext_from_content_type("image/png"), "png");
        assert_eq!(ext_from_content_type("image/jpeg"), "jpg");
    }

    #[test]
    fn work_id_shape() {
        assert_eq!(work_id("pixiv", "123"), "pixiv:123");
    }
}
