//! Hanabi-compatible perceptual clustering. Fingerprints are computed off-Worker.

use crate::ingest::sha256_hex;
use crate::{json_response, with_cors};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::JsValue;
use worker::*;

pub const MAX_FINGERPRINT_BATCH: usize = 50;
pub const MAX_CLIP_BATCH: usize = 20;
pub const MAX_SIMILAR_GROUPS: usize = 100;
pub const MAX_GROUP_ITEMS: usize = 20;
pub const CLIP_DIM: usize = 512;
pub const CLIP_SIMILAR: f32 = 0.93;
const SIMILAR_CACHE_VERSION: &str = "strict-only-v2";
pub const SIMILAR_ALGORITHM: &str = SIMILAR_CACHE_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    StrictSame,
    Similar { distance: u32 },
    Partial { distance: u32 },
    Different,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StrictSame => "strict",
            Self::Similar { .. } => "similar",
            Self::Partial { .. } => "partial",
            Self::Different => "different",
        }
    }

    pub fn distance(self) -> u32 {
        match self {
            Self::StrictSame => 0,
            Self::Similar { distance } | Self::Partial { distance } => distance,
            Self::Different => u32::MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionFingerprint {
    pub width: u32,
    pub height: u32,
    pub average_hash: String,
    pub difference_hash: String,
    pub color_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageFingerprint {
    pub r2_key: String,
    pub work_id: String,
    pub solid_color: bool,
    pub content_sha256: String,
    pub strict_key: String,
    pub average_hash: u64,
    pub difference_hash: u64,
    pub color_key: String,
    pub detail_key: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub format: String,
    pub regions: Vec<(u32, u32, u64, u64, String)>,
    pub source: String,
    pub title: String,
}

impl ImageFingerprint {
    pub fn quality_key(&self) -> (u64, u64, &str) {
        (
            u64::from(self.width) * u64::from(self.height),
            self.bytes,
            self.r2_key.as_str(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarItem {
    pub r2_key: String,
    pub work_id: String,
    pub source: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub kind: String,
    pub distance: u32,
    pub keep: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarGroup {
    pub id: String,
    pub items: Vec<SimilarItem>,
}

/// Fingerprint as computed off-Worker. Direct ingest omits `r2_key` and the
/// commit fills it in from the versioned key it verified.
#[derive(Clone, Deserialize)]
pub(crate) struct FingerprintIn {
    #[serde(default)]
    pub(crate) r2_key: String,
    #[serde(default)]
    solid_color: bool,
    pub(crate) content_sha256: String,
    strict_key: String,
    average_hash: String,
    difference_hash: String,
    color_key: String,
    detail_key: String,
    width: u32,
    height: u32,
    pub(crate) bytes: u64,
    format: String,
    #[serde(default)]
    regions: Vec<RegionFingerprint>,
}

impl FingerprintIn {
    pub(crate) fn is_valid(&self) -> bool {
        !self.r2_key.is_empty()
            && self.r2_key.len() <= 1024
            && parse_hash(&self.average_hash).is_some()
            && parse_hash(&self.difference_hash).is_some()
            && self.content_sha256.len() <= 64
            && self.strict_key.len() <= 128
            && self.color_key.len() <= 256
            && self.detail_key.len() <= 256
            && self.format.len() <= 32
            && self.regions.len() <= 32
    }

    pub(crate) fn upsert(
        &self,
        db: &D1Database,
        work_id: &str,
        now: &str,
    ) -> Result<D1PreparedStatement> {
        let regions_json =
            serde_json::to_string(&self.regions).map_err(|e| Error::RustError(e.to_string()))?;
        db.prepare(
            r#"
            INSERT INTO image_fingerprints (
              r2_key, work_id, solid_color, content_sha256, strict_key,
              average_hash, difference_hash, color_key, detail_key,
              width, height, bytes, format, regions_json, updated_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT(r2_key) DO UPDATE SET
              work_id=excluded.work_id,
              solid_color=excluded.solid_color,
              content_sha256=excluded.content_sha256,
              strict_key=excluded.strict_key,
              average_hash=excluded.average_hash,
              difference_hash=excluded.difference_hash,
              color_key=excluded.color_key,
              detail_key=excluded.detail_key,
              width=excluded.width,
              height=excluded.height,
              bytes=excluded.bytes,
              format=excluded.format,
              regions_json=excluded.regions_json,
              updated_at=excluded.updated_at
            WHERE (image_fingerprints.work_id, image_fingerprints.solid_color,
                   image_fingerprints.content_sha256, image_fingerprints.strict_key,
                   image_fingerprints.average_hash, image_fingerprints.difference_hash,
                   image_fingerprints.color_key, image_fingerprints.detail_key,
                   image_fingerprints.width, image_fingerprints.height, image_fingerprints.bytes,
                   image_fingerprints.format, image_fingerprints.regions_json)
              IS NOT (excluded.work_id, excluded.solid_color,
                      excluded.content_sha256, excluded.strict_key,
                      excluded.average_hash, excluded.difference_hash,
                      excluded.color_key, excluded.detail_key,
                      excluded.width, excluded.height, excluded.bytes,
                      excluded.format, excluded.regions_json)
            "#,
        )
        .bind(&[
            JsValue::from_str(&self.r2_key),
            JsValue::from_str(work_id),
            JsValue::from_f64(if self.solid_color { 1.0 } else { 0.0 }),
            JsValue::from_str(&self.content_sha256),
            JsValue::from_str(&self.strict_key),
            JsValue::from_str(&self.average_hash),
            JsValue::from_str(&self.difference_hash),
            JsValue::from_str(&self.color_key),
            JsValue::from_str(&self.detail_key),
            JsValue::from_f64(self.width as f64),
            JsValue::from_f64(self.height as f64),
            JsValue::from_f64(self.bytes as f64),
            JsValue::from_str(&self.format),
            JsValue::from_str(&regions_json),
            JsValue::from_str(now),
        ])
    }
}

#[derive(Deserialize)]
struct FingerprintPut {
    images: Vec<FingerprintIn>,
}

#[derive(Deserialize)]
struct DeleteBody {
    items: Vec<DeleteItem>,
}

#[derive(Deserialize)]
struct ClipIn {
    r2_key: String,
    dim: u32,
    embedding: String,
}

#[derive(Deserialize)]
struct ClipPut {
    images: Vec<ClipIn>,
}

#[derive(Serialize, Deserialize)]
struct ClipRow {
    r2_key: String,
    embedding: String,
}

#[derive(Deserialize)]
struct KeyRow {
    r2_key: String,
}

#[derive(Deserialize)]
struct DeleteItem {
    r2_key: String,
    decision_id: String,
    #[serde(default)]
    keep_r2_keys: Vec<String>,
}

#[derive(Deserialize)]
struct DeleteReceipt {
    payload: String,
    state: String,
}

fn valid_delete_items(items: &[DeleteItem]) -> bool {
    let keys: std::collections::HashSet<_> = items.iter().map(|item| &item.r2_key).collect();
    let mut decisions = std::collections::HashSet::new();
    !items.is_empty()
        && items.len() <= 100
        && keys.len() == items.len()
        && items.iter().all(|item| {
            !item.r2_key.is_empty()
                && item.r2_key.len() <= 1024
                && item.decision_id.starts_with("sim-")
                && (8..=160).contains(&item.decision_id.len())
                && item
                    .decision_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && decisions.insert(&item.decision_id)
                && item.keep_r2_keys.len() <= MAX_GROUP_ITEMS
                && item
                    .keep_r2_keys
                    .iter()
                    .all(|key| !key.is_empty() && key.len() <= 1024 && !keys.contains(key))
                && item
                    .keep_r2_keys
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == item.keep_r2_keys.len()
        })
}

#[derive(Serialize, Deserialize)]
struct FingerprintRow {
    r2_key: String,
    work_id: String,
    solid_color: i64,
    content_sha256: String,
    strict_key: String,
    average_hash: String,
    difference_hash: String,
    color_key: String,
    detail_key: String,
    width: i64,
    height: i64,
    bytes: i64,
    format: String,
    regions_json: String,
    source: String,
    title: String,
}

#[derive(Deserialize)]
struct CountRow {
    n: i64,
}

#[derive(Deserialize)]
pub struct SimilarInput {
    generation: u64,
    algorithm: String,
    fingerprints: Vec<FingerprintRow>,
    clip: Vec<ClipRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SimilarResponse {
    ok: bool,
    engine: String,
    groups: Vec<SimilarGroup>,
    auto_pairs: Vec<AutoPair>,
}

#[derive(Serialize, Deserialize)]
pub struct SimilarResult {
    generation: u64,
    algorithm: String,
    hash_count: usize,
    clip_count: usize,
    response: SimilarResponse,
}

pub fn compute_offline(input: SimilarInput) -> std::result::Result<SimilarResult, &'static str> {
    if input.algorithm != SIMILAR_ALGORITHM
        || input.generation == 0
        || input.generation > 9_007_199_254_740_991
    {
        return Err("unsupported algorithm or generation");
    }
    let images: Vec<_> = input
        .fingerprints
        .into_iter()
        .filter_map(row_to_fingerprint)
        .collect();
    let embeddings: std::collections::HashMap<_, _> = input
        .clip
        .into_iter()
        .filter_map(|row| Some((row.r2_key, parse_clip_embedding(&row.embedding)?)))
        .collect();
    let (groups, auto_pairs) = similar_view(&images, &embeddings);
    Ok(SimilarResult {
        generation: input.generation,
        algorithm: input.algorithm,
        hash_count: images.len(),
        clip_count: embeddings.len(),
        response: SimilarResponse {
            ok: true,
            engine: if has_clip_coverage(embeddings.len(), images.len()) {
                "clip"
            } else {
                "hash"
            }
            .into(),
            groups,
            auto_pairs,
        },
    })
}

pub fn parse_hash(value: &str) -> Option<u64> {
    u64::from_str_radix(value, 16).ok()
}

pub fn parse_clip_embedding(hex: &str) -> Option<Vec<f32>> {
    let bytes = hex::decode(hex).ok()?;
    if bytes.len() != CLIP_DIM * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(CLIP_DIM);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().ok()?;
        out.push(f32::from_le_bytes(arr));
    }
    out.iter().all(|value| value.is_finite()).then_some(out)
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
}

pub fn classify_similarity(left: &ImageFingerprint, right: &ImageFingerprint) -> MatchKind {
    if left.solid_color || right.solid_color {
        return MatchKind::Different;
    }
    if left.content_sha256 == right.content_sha256 {
        return MatchKind::StrictSame;
    }
    let average_distance = (left.average_hash ^ right.average_hash).count_ones();
    let difference_distance = (left.difference_hash ^ right.difference_hash).count_ones();
    let same_whole_aspect = same_aspect_ratio(left, right, 0.03);
    if same_whole_aspect
        && same_aspect_ratio(left, right, 0.005)
        && left.color_key == right.color_key
        && left.detail_key == right.detail_key
        && average_distance + difference_distance <= 1
    {
        return MatchKind::StrictSame;
    }
    let distance = average_distance + difference_distance;
    if let Some(distance) = partial_distance(left, right) {
        return MatchKind::Partial { distance };
    }
    if same_whole_aspect && average_distance <= 10 && difference_distance <= 10 && distance <= 16 {
        MatchKind::Similar { distance }
    } else {
        MatchKind::Different
    }
}

fn same_aspect_ratio(left: &ImageFingerprint, right: &ImageFingerprint, tolerance: f64) -> bool {
    if left.height == 0 || right.height == 0 {
        return false;
    }
    let left_ratio = f64::from(left.width) / f64::from(left.height);
    let right_ratio = f64::from(right.width) / f64::from(right.height);
    ((left_ratio - right_ratio).abs() / left_ratio.max(right_ratio)) <= tolerance
}

fn partial_distance(left: &ImageFingerprint, right: &ImageFingerprint) -> Option<u32> {
    region_distance(&left.regions, right)
        .into_iter()
        .chain(region_distance(&right.regions, left))
        .min()
}

fn region_distance(
    regions: &[(u32, u32, u64, u64, String)],
    whole: &ImageFingerprint,
) -> Option<u32> {
    regions
        .iter()
        .filter(|region| same_dim_aspect(region.0, region.1, whole, 0.03))
        .filter(|region| region.4 == whole.color_key)
        .filter_map(|region| {
            let average = (region.2 ^ whole.average_hash).count_ones();
            let difference = (region.3 ^ whole.difference_hash).count_ones();
            let distance = average + difference;
            (average <= 10 && difference <= 10 && distance <= 16).then_some(distance)
        })
        .min()
}

fn same_dim_aspect(width: u32, height: u32, other: &ImageFingerprint, tolerance: f64) -> bool {
    if height == 0 || other.height == 0 {
        return false;
    }
    let left_ratio = f64::from(width) / f64::from(height);
    let right_ratio = f64::from(other.width) / f64::from(other.height);
    ((left_ratio - right_ratio).abs() / left_ratio.max(right_ratio)) <= tolerance
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, value: usize) -> usize {
        if self.parent[value] != value {
            self.parent[value] = self.find(self.parent[value]);
        }
        self.parent[value]
    }

    fn union(&mut self, left: usize, right: usize) {
        let left = self.find(left);
        let right = self.find(right);
        if left != right {
            self.parent[right] = left;
        }
    }
}

pub fn cluster_similar(images: &[ImageFingerprint]) -> Vec<SimilarGroup> {
    let mut sets = DisjointSet::new(images.len());
    let mut same_sha: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    let mut region_colors: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, image) in images
        .iter()
        .enumerate()
        .filter(|(_, image)| !image.solid_color)
    {
        same_sha
            .entry(&image.content_sha256)
            .or_default()
            .push(index);
        let mut colors = std::collections::HashSet::new();
        for region in &image.regions {
            if colors.insert(region.4.as_str()) {
                region_colors.entry(&region.4).or_default().push(index);
            }
        }
    }
    for members in same_sha.values() {
        let first = members[0];
        if let Some(&other_work) = members
            .iter()
            .find(|&&index| images[index].work_id != images[first].work_id)
        {
            for &index in members {
                sets.union(
                    index,
                    if images[index].work_id == images[first].work_id {
                        other_work
                    } else {
                        first
                    },
                );
            }
        }
    }
    // Every non-SHA whole match meets these hash bounds; partial matches require a shared region color.
    for left in 0..images.len() {
        if images[left].solid_color {
            continue;
        }
        for right in (left + 1)..images.len() {
            let average = (images[left].average_hash ^ images[right].average_hash).count_ones();
            let difference =
                (images[left].difference_hash ^ images[right].difference_hash).count_ones();
            if images[right].solid_color
                || average > 10
                || difference > 10
                || average + difference > 16
                || images[left].work_id == images[right].work_id
            {
                continue;
            }
            if classify_similarity(&images[left], &images[right]) != MatchKind::Different {
                sets.union(left, right);
            }
        }
        if let Some(owners) = region_colors.get(images[left].color_key.as_str()) {
            for &right in owners {
                if images[left].work_id != images[right].work_id
                    && classify_similarity(&images[left], &images[right]) != MatchKind::Different
                {
                    sets.union(left, right);
                }
            }
        }
    }
    groups_from_sets(images, sets)
}

fn groups_from_sets(images: &[ImageFingerprint], mut sets: DisjointSet) -> Vec<SimilarGroup> {
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); images.len()];
    for index in 0..images.len() {
        let root = sets.find(index);
        buckets[root].push(index);
    }
    let mut groups = buckets
        .into_iter()
        .filter(|members| members.len() > 1)
        .map(|mut members| {
            members.sort_by(|left, right| {
                images[*right]
                    .quality_key()
                    .cmp(&images[*left].quality_key())
            });
            members.truncate(MAX_GROUP_ITEMS);
            let keep = members[0];
            let items = members
                .into_iter()
                .enumerate()
                .map(|(offset, index)| {
                    let matched = if offset == 0 {
                        MatchKind::StrictSame
                    } else {
                        classify_similarity(&images[keep], &images[index])
                    };
                    SimilarItem {
                        r2_key: images[index].r2_key.clone(),
                        work_id: images[index].work_id.clone(),
                        source: images[index].source.clone(),
                        title: images[index].title.clone(),
                        width: images[index].width,
                        height: images[index].height,
                        bytes: images[index].bytes,
                        kind: matched.as_str().to_string(),
                        distance: matched.distance(),
                        keep: offset == 0,
                    }
                })
                .collect::<Vec<_>>();
            let id = skip_key_from_items(&items);
            SimilarGroup { id, items }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .items
            .len()
            .cmp(&left.items.len())
            .then_with(|| left.id.cmp(&right.id))
    });
    groups.truncate(MAX_SIMILAR_GROUPS);
    groups
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoPair {
    pub keep: SimilarItem,
    pub remove: SimilarItem,
}

fn item_from(image: &ImageFingerprint, kind: MatchKind, keep: bool) -> SimilarItem {
    SimilarItem {
        r2_key: image.r2_key.clone(),
        work_id: image.work_id.clone(),
        source: image.source.clone(),
        title: image.title.clone(),
        width: image.width,
        height: image.height,
        bytes: image.bytes,
        kind: kind.as_str().to_string(),
        distance: kind.distance(),
        keep,
    }
}

pub fn split_auto_pairs(
    groups: Vec<SimilarGroup>,
    images: &[ImageFingerprint],
) -> (Vec<SimilarGroup>, Vec<AutoPair>) {
    let by_key: std::collections::HashMap<&str, &ImageFingerprint> = images
        .iter()
        .map(|image| (image.r2_key.as_str(), image))
        .collect();
    let mut review = Vec::new();
    let mut auto_pairs = Vec::new();
    for group in groups {
        if group.items.len() == 2 {
            let keep = by_key.get(group.items[0].r2_key.as_str());
            let remove = by_key.get(group.items[1].r2_key.as_str());
            if let (Some(keep), Some(remove)) = (keep, remove) {
                let hashed = classify_similarity(keep, remove);
                if hashed == MatchKind::StrictSame {
                    auto_pairs.push(AutoPair {
                        keep: item_from(keep, MatchKind::StrictSame, true),
                        remove: item_from(remove, hashed, false),
                    });
                    continue;
                }
            }
        }
        review.push(group);
    }
    (review, auto_pairs)
}

fn clip_item(image: &ImageFingerprint, score: f32, hashed: MatchKind, keep: bool) -> SimilarItem {
    let mut item = item_from(
        image,
        if keep {
            MatchKind::StrictSame
        } else {
            MatchKind::Similar {
                distance: ((1.0 - score) * 1000.0).round() as u32,
            }
        },
        keep,
    );
    if keep {
        item.kind = "keep".into();
    } else if hashed == MatchKind::StrictSame {
        item.kind = "strict".into();
    } else {
        item.kind = format!("clip {score:.3}");
    }
    item
}

pub fn cluster_clip(
    images: &[ImageFingerprint],
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> Vec<SimilarGroup> {
    let indexed: Vec<usize> = {
        let mut order: Vec<usize> = images
            .iter()
            .enumerate()
            .filter(|(_, image)| !image.solid_color && embeddings.contains_key(&image.r2_key))
            .map(|(index, _)| index)
            .collect();
        order.sort_by(|left, right| {
            images[*right]
                .quality_key()
                .cmp(&images[*left].quality_key())
        });
        order
    };
    if indexed.len() < 2 {
        return Vec::new();
    }
    let mut used = vec![false; images.len()];
    let mut groups = Vec::new();
    for &keep in &indexed {
        if used[keep] {
            continue;
        }
        let Some(keep_vec) = embeddings.get(&images[keep].r2_key) else {
            continue;
        };
        let mut members = vec![keep];
        for &index in &indexed {
            if index == keep || used[index] || images[keep].work_id == images[index].work_id {
                continue;
            }
            let Some(other) = embeddings.get(&images[index].r2_key) else {
                continue;
            };
            if cosine(keep_vec, other) >= CLIP_SIMILAR {
                members.push(index);
            }
        }
        if members.len() < 2 {
            continue;
        }
        members.truncate(MAX_GROUP_ITEMS);
        for &index in &members {
            used[index] = true;
        }
        let items = members
            .iter()
            .enumerate()
            .map(|(offset, &index)| {
                let score = if offset == 0 {
                    1.0
                } else {
                    cosine(keep_vec, &embeddings[&images[index].r2_key])
                };
                let hashed = if offset == 0 {
                    MatchKind::StrictSame
                } else {
                    classify_similarity(&images[keep], &images[index])
                };
                clip_item(&images[index], score, hashed, offset == 0)
            })
            .collect::<Vec<_>>();
        let id = skip_key_from_items(&items);
        groups.push(SimilarGroup { id, items });
    }
    groups.sort_by(|left, right| {
        right
            .items
            .len()
            .cmp(&left.items.len())
            .then_with(|| left.id.cmp(&right.id))
    });
    groups.truncate(MAX_SIMILAR_GROUPS);
    groups
}

pub fn reject_empty_groups(groups: &[SimilarGroup], delete_keys: &[String]) -> bool {
    let delete: std::collections::HashSet<&str> = delete_keys.iter().map(String::as_str).collect();
    groups.iter().any(|group| {
        let touches = group
            .items
            .iter()
            .any(|item| delete.contains(item.r2_key.as_str()));
        let remaining = group
            .items
            .iter()
            .filter(|item| !delete.contains(item.r2_key.as_str()))
            .count();
        touches && remaining == 0
    })
}

fn reject(status: u16, error: &str) -> Result<Response> {
    Ok(with_cors(json_response(
        &json!({ "ok": false, "error": error }),
        status,
    )?))
}

fn row_to_fingerprint(row: FingerprintRow) -> Option<ImageFingerprint> {
    let average_hash = parse_hash(&row.average_hash)?;
    let difference_hash = parse_hash(&row.difference_hash)?;
    let regions: Vec<RegionFingerprint> = serde_json::from_str(&row.regions_json).ok()?;
    let regions = regions
        .into_iter()
        .filter_map(|region| {
            Some((
                region.width,
                region.height,
                parse_hash(&region.average_hash)?,
                parse_hash(&region.difference_hash)?,
                region.color_key,
            ))
        })
        .collect();
    Some(ImageFingerprint {
        r2_key: row.r2_key,
        work_id: row.work_id,
        solid_color: row.solid_color != 0,
        content_sha256: row.content_sha256,
        strict_key: row.strict_key,
        average_hash,
        difference_hash,
        color_key: row.color_key,
        detail_key: row.detail_key,
        width: row.width.max(0) as u32,
        height: row.height.max(0) as u32,
        bytes: row.bytes.max(0) as u64,
        format: row.format,
        regions,
        source: row.source,
        title: row.title,
    })
}

fn has_clip_coverage(clip_count: usize, image_count: usize) -> bool {
    clip_count * 5 >= image_count.max(1) * 4
}

fn similar_view(
    images: &[ImageFingerprint],
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> (Vec<SimilarGroup>, Vec<AutoPair>) {
    let (mut review, mut auto_pairs) = split_auto_pairs(cluster_similar(images), images);
    if has_clip_coverage(embeddings.len(), images.len()) {
        let (clip_review, clip_auto) = split_auto_pairs(cluster_clip(images, embeddings), images);
        review = clip_review;
        auto_pairs.extend(clip_auto);
        auto_pairs.sort_by(|left, right| left.remove.r2_key.cmp(&right.remove.r2_key));
        auto_pairs.dedup_by(|left, right| left.remove.r2_key == right.remove.r2_key);
    }
    (review, auto_pairs)
}

pub async fn handle_put(mut req: Request, env: Env) -> Result<Response> {
    let body: FingerprintPut = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid json"),
    };
    if body.images.is_empty() || body.images.len() > MAX_FINGERPRINT_BATCH {
        return reject(400, "invalid fingerprint batch");
    }
    let db = env.d1("DB")?;
    let now = crate::js_iso_now();
    let mut statements = Vec::new();
    for image in &body.images {
        if !image.is_valid() {
            return reject(400, "invalid fingerprint");
        }
        let work = db
            .prepare("SELECT work_id FROM images WHERE r2_key=? LIMIT 1")
            .bind(&[JsValue::from_str(&image.r2_key)])?
            .first::<WorkIdRow>(None)
            .await?;
        let Some(work) = work else {
            return reject(400, "unknown r2_key");
        };
        statements.push(image.upsert(&db, &work.work_id, &now)?);
    }
    db.batch(statements).await?;
    Ok(with_cors(json_response(
        &json!({ "ok": true, "upserted": body.images.len() }),
        200,
    )?))
}

pub async fn handle_put_clip(mut req: Request, env: Env) -> Result<Response> {
    let body: ClipPut = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid json"),
    };
    if body.images.is_empty() || body.images.len() > MAX_CLIP_BATCH {
        return reject(400, "invalid clip batch");
    }
    let db = env.d1("DB")?;
    let now = crate::js_iso_now();
    let mut statements = Vec::new();
    for image in &body.images {
        if image.r2_key.is_empty()
            || image.r2_key.len() > 1024
            || image.dim as usize != CLIP_DIM
            || parse_clip_embedding(&image.embedding).is_none()
        {
            return reject(400, "invalid clip embedding");
        }
        let work = db
            .prepare("SELECT work_id FROM images WHERE r2_key=? LIMIT 1")
            .bind(&[JsValue::from_str(&image.r2_key)])?
            .first::<WorkIdRow>(None)
            .await?;
        if work.is_none() {
            return reject(400, "unknown r2_key");
        }
        statements.push(
            db.prepare(
                r#"
                INSERT INTO image_clip (r2_key, dim, embedding, updated_at)
                VALUES (?,?,?,?)
                ON CONFLICT(r2_key) DO UPDATE SET
                  dim=excluded.dim,
                  embedding=excluded.embedding,
                  updated_at=excluded.updated_at
                WHERE (image_clip.dim, image_clip.embedding) IS NOT (excluded.dim, excluded.embedding)
                "#,
            )
            .bind(&[
                JsValue::from_str(&image.r2_key),
                JsValue::from_f64(CLIP_DIM as f64),
                JsValue::from_str(&image.embedding),
                JsValue::from_str(&now),
            ])?,
        );
    }
    db.batch(statements).await?;
    Ok(with_cors(json_response(
        &json!({ "ok": true, "upserted": body.images.len() }),
        200,
    )?))
}

pub async fn handle_clip_missing(_req: Request, env: Env) -> Result<Response> {
    let rows = env
        .d1("DB")?
        .prepare(
            r#"
            SELECT i.r2_key
            FROM images i
            JOIN works w ON w.id = i.work_id
            WHERE w.deleted_at IS NULL
              AND NOT EXISTS (SELECT 1 FROM image_clip c WHERE c.r2_key = i.r2_key)
            "#,
        )
        .all()
        .await?
        .results::<KeyRow>()?;
    let missing: Vec<String> = rows.into_iter().map(|row| row.r2_key).collect();
    Ok(with_cors(json_response(
        &json!({ "ok": true, "missing": missing }),
        200,
    )?))
}

#[derive(Deserialize)]
struct WorkIdRow {
    work_id: String,
}

#[derive(Deserialize, Serialize)]
struct SimilarStatistics {
    generation: u64,
    online: i64,
    hashed: i64,
    clipped: i64,
}

async fn statistics(
    req: &Request,
    db: &D1Database,
    generation: u64,
) -> Result<Option<(SimilarStatistics, &'static str)>> {
    let cache = Cache::default();
    let key = format!(
        "{}/__similar-statistics/v1/{generation}",
        req.url()?.origin().ascii_serialization()
    );
    let cached = match cache.get(key.clone(), false).await {
        Ok(Some(mut response)) => response.json::<SimilarStatistics>().await.ok(),
        _ => None,
    };
    let (stats, source) = if let Some(stats) = cached.filter(|stats| stats.generation == generation)
    {
        (stats, "HIT")
    } else {
        let online = db
            .prepare("SELECT COUNT(*) AS n FROM images i JOIN works w ON w.id=i.work_id WHERE w.deleted_at IS NULL")
            .first::<CountRow>(None)
            .await?
            .map(|row| row.n)
            .unwrap_or(0);
        let hashed = db
            .prepare(
                r#"SELECT COUNT(*) AS n FROM image_fingerprints f
                   JOIN images i ON i.r2_key=f.r2_key
                   JOIN works w ON w.id=i.work_id
                   WHERE w.deleted_at IS NULL"#,
            )
            .first::<CountRow>(None)
            .await?
            .map(|row| row.n)
            .unwrap_or(0);
        let clipped = db
            .prepare(
                r#"SELECT COUNT(*) AS n FROM image_clip c
                   JOIN images i ON i.r2_key=c.r2_key
                   JOIN works w ON w.id=i.work_id
                   WHERE w.deleted_at IS NULL"#,
            )
            .first::<CountRow>(None)
            .await?
            .map(|row| row.n)
            .unwrap_or(0);
        (
            SimilarStatistics {
                generation,
                online,
                hashed,
                clipped,
            },
            "MISS",
        )
    };
    if current_generation(db).await? != generation {
        return Ok(None);
    }
    if source == "MISS" {
        let mut response = Response::from_json(&stats)?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=86400")?;
        // A cache eviction only affects read cost; generation remains D1-authoritative.
        let _ = cache.put(key, response).await;
    }
    Ok(Some((stats, source)))
}

pub async fn handle_status(req: Request, env: Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let generation = current_generation(&db).await?;
    let Some((stats, source)) = statistics(&req, &db, generation).await? else {
        return reject(409, "similar snapshot changed");
    };
    let mut response = json_response(
        &json!({
            "ok": true,
            "generation": generation,
            "hashed": stats.hashed,
            "clipped": stats.clipped,
            "online": stats.online,
            "pending": (stats.online - stats.hashed).max(0),
            "pending_clip": (stats.online - stats.clipped).max(0),
        }),
        200,
    )?;
    response
        .headers_mut()
        .set("X-Similar-Stats-Cache", source)?;
    Ok(with_cors(response))
}

#[derive(Deserialize)]
struct SkipBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    keys: Vec<String>,
}

#[derive(Deserialize)]
struct SkipRow {
    group_id: String,
    #[serde(default)]
    members: String,
}

fn valid_group_id(id: &str) -> bool {
    let len = id.len();
    (32..=128).contains(&len) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn skip_key_from_items(items: &[SimilarItem]) -> String {
    skip_key_from_r2(items.iter().map(|item| item.r2_key.as_str()))
}

fn skip_key_from_r2<'a>(keys: impl Iterator<Item = &'a str>) -> String {
    let mut keys: Vec<&str> = keys.collect();
    keys.sort_unstable();
    keys.dedup();
    sha256_hex(keys.join("\n").as_bytes())
}

async fn load_skips(db: &D1Database) -> Result<Vec<SkipRow>> {
    db.prepare("SELECT group_id, COALESCE(members,'') AS members FROM similar_skips")
        .all()
        .await?
        .results::<SkipRow>()
}

fn group_is_skipped(group: &SimilarGroup, skips: &[SkipRow]) -> bool {
    let keys: std::collections::HashSet<&str> = group
        .items
        .iter()
        .map(|item| item.r2_key.as_str())
        .collect();
    skips.iter().any(|skip| {
        if skip.group_id == group.id {
            return true;
        }
        if skip.members.is_empty() {
            return false;
        }
        let set: std::collections::HashSet<&str> =
            skip.members.split('\n').filter(|s| !s.is_empty()).collect();
        !keys.is_empty() && keys.iter().all(|key| set.contains(*key))
    })
}

#[derive(Deserialize)]
struct CacheRow {
    payload: String,
    generation: u64,
    computed_at: String,
}

struct ReviewResult {
    result: SimilarResult,
    generation: u64,
    computed_at: String,
}

#[derive(Deserialize)]
struct GenerationRow {
    generation: u64,
}

pub(crate) async fn current_generation(db: &D1Database) -> Result<u64> {
    db.prepare("SELECT generation FROM similar_generation WHERE id=1")
        .first::<GenerationRow>(None)
        .await?
        .map(|row| row.generation)
        .ok_or_else(|| Error::RustError("similar generation missing".into()))
}

async fn review_result(req: &Request, db: &D1Database) -> Result<Option<ReviewResult>> {
    let row = db.prepare("SELECT c.payload,c.computed_at,g.generation FROM similar_cache c JOIN similar_generation g ON g.id=c.id WHERE c.id=1")
        .first::<CacheRow>(None).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let Ok(mut result) = serde_json::from_str::<SimilarResult>(&row.payload) else {
        return Ok(None);
    };
    if !valid_result(&result) || result.generation > row.generation {
        return Ok(None);
    }
    {
        let cache = Cache::default();
        let key = format!(
            "{}/__similar-review/v2/{}/{}/{}",
            req.url()?.origin().ascii_serialization(),
            row.generation,
            result.generation,
            sha256_hex(row.payload.as_bytes())
        );
        let cached = match cache.get(key.clone(), false).await {
            Ok(Some(mut response)) => response.json::<SimilarResponse>().await.ok(),
            _ => None,
        };
        if let Some(response) = cached {
            result.response = response;
        } else {
            let keys: std::collections::HashSet<_> = result
                .response
                .groups
                .iter()
                .flat_map(|group| group.items.iter())
                .chain(
                    result
                        .response
                        .auto_pairs
                        .iter()
                        .flat_map(|pair| [&pair.keep, &pair.remove]),
                )
                .map(|item| &item.r2_key)
                .collect();
            let keys = serde_json::to_string(&keys)
                .map_err(|error| Error::RustError(error.to_string()))?;
            let rows = db
                .prepare(
                    r#"
                SELECT f.r2_key,f.work_id,f.solid_color,f.content_sha256,f.strict_key,
                       f.average_hash,f.difference_hash,f.color_key,f.detail_key,
                       f.width,f.height,f.bytes,f.format,f.regions_json,w.source,w.title
                FROM image_fingerprints f CROSS JOIN images i ON i.r2_key=f.r2_key
                CROSS JOIN works w ON w.id=i.work_id
                WHERE f.r2_key IN (SELECT value FROM json_each(?))
                  AND f.work_id=i.work_id AND f.solid_color=0 AND w.deleted_at IS NULL
            "#,
                )
                .bind(&[JsValue::from_str(&keys)])?
                .all()
                .await?
                .results::<FingerprintRow>()?;
            let images = rows
                .into_iter()
                .filter_map(row_to_fingerprint)
                .map(|image| (image.r2_key.clone(), image))
                .collect();
            let clips = db.prepare("SELECT r2_key,embedding FROM image_clip WHERE r2_key IN (SELECT value FROM json_each(?))")
                .bind(&[JsValue::from_str(&keys)])?.all().await?.results::<ClipRow>()?;
            let embeddings = clips
                .into_iter()
                .filter_map(|row| Some((row.r2_key, parse_clip_embedding(&row.embedding)?)))
                .collect();
            result.response = revalidate_candidates(result.response, &images, &embeddings);
            if current_generation(db).await? != row.generation {
                return Err(Error::RustError(
                    "similar snapshot changed; refresh before reviewing".into(),
                ));
            }
            let mut response = Response::from_json(&result.response)?;
            response
                .headers_mut()
                .set("Cache-Control", "public, max-age=86400")?;
            let _ = cache.put(key, response).await;
        }
    }
    if current_generation(db).await? != row.generation {
        return Err(Error::RustError(
            "similar snapshot changed; refresh before reviewing".into(),
        ));
    }
    Ok(Some(ReviewResult {
        result,
        generation: row.generation,
        computed_at: row.computed_at,
    }))
}

fn revalidate_candidates(
    mut response: SimilarResponse,
    images: &std::collections::HashMap<String, ImageFingerprint>,
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> SimilarResponse {
    // Only recheck existing bounded groups; discovery remains on Oracle's daily schedule.
    response.groups = response
        .groups
        .into_iter()
        .filter_map(|group| {
            let members: Option<Vec<_>> = group
                .items
                .iter()
                .map(|item| {
                    images
                        .get(&item.r2_key)
                        .filter(|image| image.work_id == item.work_id)
                        .cloned()
                })
                .collect();
            let members = members?;
            let current = if response.engine == "clip" {
                let keeper = group.items.iter().find(|item| item.keep)?;
                let vector = embeddings.get(&keeper.r2_key)?;
                if members.iter().any(|image| {
                    embeddings
                        .get(&image.r2_key)
                        .is_none_or(|other| cosine(vector, other) < CLIP_SIMILAR)
                }) {
                    return None;
                }
                cluster_clip(&members, embeddings)
            } else {
                cluster_similar(&members)
            };
            current.into_iter().find(|candidate| {
                candidate.id == group.id
                    && candidate
                        .items
                        .iter()
                        .filter(|item| item.keep)
                        .map(|item| &item.r2_key)
                        .eq(group
                            .items
                            .iter()
                            .filter(|item| item.keep)
                            .map(|item| &item.r2_key))
            })
        })
        .collect();
    response.auto_pairs = response
        .auto_pairs
        .into_iter()
        .filter_map(|pair| {
            let keep = images.get(&pair.keep.r2_key)?;
            let remove = images.get(&pair.remove.r2_key)?;
            (keep.work_id == pair.keep.work_id
                && remove.work_id == pair.remove.work_id
                && keep.quality_key() >= remove.quality_key()
                && classify_similarity(keep, remove) == MatchKind::StrictSame)
                .then(|| AutoPair {
                    keep: item_from(keep, MatchKind::StrictSame, true),
                    remove: item_from(remove, MatchKind::StrictSame, false),
                })
        })
        .collect();
    response
}

fn valid_result(result: &SimilarResult) -> bool {
    let valid_item = |item: &SimilarItem| {
        !item.r2_key.is_empty()
            && item.r2_key.len() <= 1024
            && !item.work_id.is_empty()
            && item.work_id.len() <= 2048
            && item.source.len() <= 64
            && item.title.len() <= 2048
    };
    result.algorithm == SIMILAR_ALGORITHM
        && result.generation > 0
        && result.generation <= 9_007_199_254_740_991
        && result.response.ok
        && matches!(result.response.engine.as_str(), "hash" | "clip")
        && result.response.groups.len() <= MAX_SIMILAR_GROUPS
        && result.response.auto_pairs.len() <= MAX_SIMILAR_GROUPS * 2
        && result.response.groups.iter().all(|group| {
            valid_group_id(&group.id)
                && (2..=MAX_GROUP_ITEMS).contains(&group.items.len())
                && group.items.iter().all(valid_item)
                && group.items.iter().any(|item| item.keep)
                && group
                    .items
                    .iter()
                    .map(|item| &item.r2_key)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == group.items.len()
        })
        && result.response.auto_pairs.iter().all(|pair| {
            valid_item(&pair.keep)
                && valid_item(&pair.remove)
                && pair.keep.r2_key != pair.remove.r2_key
                && pair.keep.work_id != pair.remove.work_id
                && pair.keep.keep
                && !pair.remove.keep
                && pair.keep.kind == "strict"
                && pair.remove.kind == "strict"
        })
}

#[derive(Deserialize)]
struct SnapshotRow {
    generation: u64,
    result_generation: Option<u64>,
    result_algorithm: Option<String>,
    result_ok: Option<i64>,
}

pub async fn handle_snapshot(req: Request, env: Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let row = db.prepare(r#"
        SELECT g.generation,
          CASE WHEN json_valid(c.payload) THEN json_extract(c.payload,'$.generation') END AS result_generation,
          CASE WHEN json_valid(c.payload) THEN json_extract(c.payload,'$.algorithm') END AS result_algorithm,
          CASE WHEN json_valid(c.payload) THEN json_extract(c.payload,'$.response.ok') END AS result_ok
        FROM similar_generation g LEFT JOIN similar_cache c ON c.id=g.id WHERE g.id=1
    "#).first::<SnapshotRow>(None).await?
        .ok_or_else(|| Error::RustError("similar generation missing".into()))?;
    let mut body = json!({
        "ok": true, "generation": row.generation, "algorithm": SIMILAR_ALGORITHM,
        "result_generation": row.result_generation,
        "result_fresh": row.result_generation == Some(row.generation) && row.result_algorithm.as_deref() == Some(SIMILAR_ALGORITHM) && row.result_ok == Some(1),
    });
    let counts = req
        .url()?
        .query_pairs()
        .any(|(key, value)| key == "counts" && value == "1");
    let source = if counts {
        let Some((stats, source)) = statistics(&req, &db, row.generation).await? else {
            return reject(409, "similar snapshot changed");
        };
        body["fingerprint_count"] = json!(stats.hashed);
        body["clip_count"] = json!(stats.clipped);
        Some(source)
    } else {
        None
    };
    let mut response = json_response(&body, 200)?;
    if let Some(source) = source {
        response
            .headers_mut()
            .set("X-Similar-Stats-Cache", source)?;
    }
    Ok(with_cors(response))
}

pub async fn handle_input(req: Request, env: Env) -> Result<Response> {
    let query: std::collections::HashMap<_, _> = req.url()?.query_pairs().into_owned().collect();
    let Some(generation) = query
        .get("generation")
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return reject(400, "generation required");
    };
    if query
        .get("offset")
        .is_some_and(|value| value.parse::<u32>() != Ok(0))
    {
        return reject(400, "offset unsupported; use after cursor");
    }
    let after = query.get("after").map_or("", String::as_str);
    if after.chars().count() > 1024 {
        return reject(400, "similar input cursor too long");
    }
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100)
        .clamp(1, 100);
    let db = env.d1("DB")?;
    if current_generation(&db).await? != generation {
        return reject(409, "similar snapshot changed");
    }
    let bound = [JsValue::from_str(after), JsValue::from_f64(limit as f64)];
    let items = match query.get("kind").map(String::as_str) {
        Some("fingerprints") => serde_json::to_value(db.prepare(r#"
            SELECT f.r2_key,f.work_id,f.solid_color,f.content_sha256,f.strict_key,
                   f.average_hash,f.difference_hash,f.color_key,f.detail_key,
                   f.width,f.height,f.bytes,f.format,f.regions_json,w.source,w.title
            FROM image_fingerprints f CROSS JOIN images i ON i.r2_key=f.r2_key CROSS JOIN works w ON w.id=i.work_id
            WHERE f.r2_key > ? AND w.deleted_at IS NULL ORDER BY f.r2_key LIMIT ?
        "#).bind(&bound)?.all().await?.results::<FingerprintRow>()?),
        Some("clip") => serde_json::to_value(db.prepare(r#"
            SELECT c.r2_key,c.embedding FROM image_clip c CROSS JOIN images i ON i.r2_key=c.r2_key CROSS JOIN works w ON w.id=i.work_id
            WHERE c.r2_key > ? AND w.deleted_at IS NULL ORDER BY c.r2_key LIMIT ?
        "#).bind(&bound)?.all().await?.results::<ClipRow>()?),
        _ => return reject(400, "unknown similar input kind"),
    }.map_err(|error| Error::RustError(error.to_string()))?;
    if current_generation(&db).await? != generation {
        return reject(409, "similar snapshot changed");
    }
    let rows = items
        .as_array()
        .ok_or_else(|| Error::RustError("similar input rows missing".into()))?;
    let next_cursor = if rows.len() == limit as usize {
        rows.last().and_then(|row| row["r2_key"].as_str())
    } else {
        None
    };
    Ok(with_cors(json_response(
        &json!({
            "ok": true, "generation": generation, "algorithm": SIMILAR_ALGORITHM, "items": items,
            "next_cursor": next_cursor,
        }),
        200,
    )?))
}

pub async fn handle_publish(mut req: Request, env: Env) -> Result<Response> {
    let result: SimilarResult = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid similar result"),
    };
    if !valid_result(&result) {
        return reject(400, "invalid similar result");
    }
    let db = env.d1("DB")?;
    let payload =
        serde_json::to_string(&result).map_err(|error| Error::RustError(error.to_string()))?;
    let updated = db
        .prepare(
            r#"
        INSERT INTO similar_cache (id,clip_count,hash_count,payload,computed_at)
        SELECT 1,?,?,?,? FROM similar_generation WHERE id=1 AND generation=?
        ON CONFLICT(id) DO UPDATE SET clip_count=excluded.clip_count,hash_count=excluded.hash_count,
          payload=excluded.payload,computed_at=excluded.computed_at
    "#,
        )
        .bind(&[
            JsValue::from_f64(result.clip_count as f64),
            JsValue::from_f64(result.hash_count as f64),
            JsValue::from_str(&payload),
            JsValue::from_str(&crate::js_iso_now()),
            JsValue::from_f64(result.generation as f64),
        ])?
        .run()
        .await?;
    if updated.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        return reject(409, "similar snapshot changed");
    }
    Ok(with_cors(json_response(
        &json!({"ok":true,"generation":result.generation}),
        200,
    )?))
}

#[cfg(test)]
fn similar_revision(
    images: &[ImageFingerprint],
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut images: Vec<_> = images.iter().collect();
    images.sort_by(|a, b| a.r2_key.cmp(&b.r2_key));
    let mut embeddings: Vec<_> = embeddings.iter().collect();
    embeddings.sort_by(|a, b| a.0.cmp(b.0));
    let bytes = serde_json::to_vec(&images).map_err(|error| Error::RustError(error.to_string()))?;
    let mut hash = Sha256::new();
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(&bytes);
    hash.update((embeddings.len() as u64).to_le_bytes());
    // Binary embeddings avoid formatting 512 floats per image just to check the cache.
    let mut floats = Vec::with_capacity(CLIP_DIM * 4);
    for (key, values) in embeddings {
        hash.update((key.len() as u64).to_le_bytes());
        hash.update(key.as_bytes());
        hash.update((values.len() as u64).to_le_bytes());
        floats.clear();
        for value in values {
            floats.extend_from_slice(&value.to_le_bytes());
        }
        hash.update(&floats);
    }
    Ok(format!(
        "{SIMILAR_CACHE_VERSION}:binary-v1:{}",
        hex::encode(hash.finalize())
    ))
}

fn apply_skips(mut payload: Value, skips: &[SkipRow]) -> Value {
    if let Some(groups) = payload.get_mut("groups").and_then(Value::as_array_mut) {
        groups.retain(|group| {
            let Ok(parsed) = serde_json::from_value::<SimilarGroup>(group.clone()) else {
                return true;
            };
            !group_is_skipped(&parsed, skips)
        });
    }
    payload
}

pub async fn handle_list(req: Request, env: Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let Some(review) = review_result(&req, &db).await? else {
        return Ok(with_cors(json_response(
            &json!({"ok":true,"pending":true,"groups":[],"auto_pairs":[]}),
            200,
        )?));
    };
    let skipped = load_skips(&db).await?;
    let mut payload = serde_json::to_value(review.result.response)
        .map_err(|error| Error::RustError(error.to_string()))?;
    payload["pending"] = json!(false);
    payload["stale"] = json!(review.result.generation != review.generation);
    payload["computed_at"] = json!(review.computed_at);
    Ok(with_cors(json_response(
        &apply_skips(payload, &skipped),
        200,
    )?))
}

pub async fn handle_skip(mut req: Request, env: Env) -> Result<Response> {
    let body: SkipBody = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid json"),
    };
    let keys: Vec<String> = body
        .keys
        .into_iter()
        .filter(|key| !key.is_empty() && key.len() <= 1024)
        .collect();
    let members = {
        let mut keys = keys.clone();
        keys.sort();
        keys.dedup();
        keys.join("\n")
    };
    let id = if !keys.is_empty() {
        skip_key_from_r2(keys.iter().map(String::as_str))
    } else if valid_group_id(&body.id) {
        body.id.clone()
    } else {
        return reject(400, "invalid group id");
    };
    let db = env.d1("DB")?;
    let now = crate::js_iso_now();
    db.prepare(
        "INSERT INTO similar_skips (group_id, created_at, members) VALUES (?, ?, ?)
         ON CONFLICT(group_id) DO UPDATE SET members=excluded.members",
    )
    .bind(&[
        JsValue::from_str(&id),
        JsValue::from_str(&now),
        JsValue::from_str(&members),
    ])?
    .run()
    .await?;
    if valid_group_id(&body.id) && body.id != id {
        db.prepare(
            "INSERT INTO similar_skips (group_id, created_at, members) VALUES (?, ?, ?)
             ON CONFLICT(group_id) DO UPDATE SET members=excluded.members",
        )
        .bind(&[
            JsValue::from_str(&body.id),
            JsValue::from_str(&now),
            JsValue::from_str(&members),
        ])?
        .run()
        .await?;
    }
    Ok(with_cors(json_response(
        &json!({ "ok": true, "id": id }),
        200,
    )?))
}

pub async fn handle_delete(mut req: Request, env: Env) -> Result<Response> {
    let body: DeleteBody = match req.json().await {
        Ok(value) => value,
        Err(_) => return reject(400, "invalid json"),
    };
    if !valid_delete_items(&body.items) {
        return reject(400, "invalid delete set or decision id");
    }
    let keys: Vec<String> = body.items.iter().map(|item| item.r2_key.clone()).collect();
    let db = env.d1("DB")?;
    let cached = review_result(&req, &db).await?;
    let fresh = cached
        .as_ref()
        .is_some_and(|review| review.result.generation == review.generation);
    let mut generation = cached.as_ref().map(|review| review.generation);
    let mut guarded = Vec::new();
    if let Some(review) = cached {
        guarded = review.result.response.groups;
        for pair in review.result.response.auto_pairs {
            guarded.push(SimilarGroup {
                id: pair.keep.r2_key.clone(),
                items: vec![pair.keep, pair.remove],
            });
        }
    }
    if reject_empty_groups(&guarded, &keys) {
        return reject(400, "keep at least one image in each group");
    }
    let mut keeper_keys: Vec<String> = body
        .items
        .iter()
        .flat_map(|item| item.keep_r2_keys.iter().cloned())
        .collect();
    for group in &guarded {
        if group.items.iter().any(|item| keys.contains(&item.r2_key)) {
            keeper_keys.extend(
                group
                    .items
                    .iter()
                    .filter(|item| !keys.contains(&item.r2_key))
                    .map(|item| item.r2_key.clone()),
            );
        }
    }
    keeper_keys.sort();
    keeper_keys.dedup();
    let keeper_rows = db.prepare("SELECT i.r2_key,i.work_id,w.review_version FROM images i JOIN works w ON w.id=i.work_id WHERE w.deleted_at IS NULL AND i.r2_key IN (SELECT value FROM json_each(?))")
        .bind(&[JsValue::from_str(&serde_json::to_string(&keeper_keys).unwrap())])?.all().await?.results::<crate::image_review::SimilarKeeper>()?;
    let keepers: std::collections::HashMap<_, _> = keeper_rows
        .into_iter()
        .map(|keeper| (keeper.r2_key.clone(), keeper))
        .collect();
    let bot = env
        .secret("TELEGRAM_BOT_TOKEN")
        .map(|value| value.to_string())
        .or_else(|_| env.var("TELEGRAM_BOT_TOKEN").map(|value| value.to_string()))
        .unwrap_or_default();
    let mut results = Vec::new();
    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    let mut telegram_failed = Vec::new();
    let mut telegram_pending = Vec::new();
    for item in &body.items {
        let can_start = fresh
            || guarded.iter().any(|group| {
                group
                    .items
                    .iter()
                    .any(|member| member.r2_key == item.r2_key)
            });
        let mut required_groups = vec![item.keep_r2_keys.clone()];
        for group in &guarded {
            if group
                .items
                .iter()
                .any(|member| member.r2_key == item.r2_key)
            {
                required_groups.push(
                    group
                        .items
                        .iter()
                        .filter(|member| !keys.contains(&member.r2_key))
                        .map(|member| member.r2_key.clone())
                        .collect(),
                );
            }
        }
        let survivors: Vec<Vec<_>> = required_groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .filter_map(|key| keepers.get(key).cloned())
                    .collect()
            })
            .collect();
        let result =
            match delete_item(&env, &db, item, &bot, &survivors, can_start, generation).await {
                Ok(result) => result,
                Err(_) => json!({"ok": false, "error": "image review failed; retry this item"}),
            };
        if result.get("ok") == Some(&Value::Bool(true)) {
            if let Some(next) = result.get("generation").and_then(Value::as_u64) {
                generation = Some(next);
            }
            deleted.push(item.r2_key.clone());
        } else {
            failed.push(
                json!({"r2_key": item.r2_key, "decision_id": item.decision_id,
                "error": result.get("error")}),
            );
        }
        if let Some(targets) = result.get("telegram_failed").and_then(Value::as_array) {
            telegram_failed.extend(targets.iter().cloned());
        }
        if let Some(targets) = result.get("telegram_pending").and_then(Value::as_array) {
            telegram_pending.extend(targets.iter().cloned());
        }
        results.push(
            json!({"r2_key": item.r2_key, "decision_id": item.decision_id,
            "result": result}),
        );
    }
    Ok(with_cors(json_response(
        &json!({
            "ok": true,
            "partial": !failed.is_empty(),
            "deleted": deleted,
            "failed": failed,
            "results": results,
            "telegram_failed": telegram_failed,
            "telegram_pending": telegram_pending,
        }),
        200,
    )?))
}

async fn delete_item(
    env: &Env,
    db: &D1Database,
    item: &DeleteItem,
    bot: &str,
    survivors: &[Vec<crate::image_review::SimilarKeeper>],
    can_start: bool,
    generation: Option<u64>,
) -> Result<Value> {
    if let Some(receipt) = db
        .prepare("SELECT payload,state FROM catalog_image_reviews WHERE decision_id=?")
        .bind(&[JsValue::from_str(&item.decision_id)])?
        .first::<DeleteReceipt>(None)
        .await?
    {
        let snapshot: Value =
            serde_json::from_str(&receipt.payload).map_err(|e| Error::RustError(e.to_string()))?;
        if snapshot["image"]["r2_key"].as_str() != Some(&item.r2_key) || receipt.state == "restored"
        {
            return Ok(json!({"ok": false, "error": "image review decision conflict"}));
        }
        if receipt.state == "deleted" {
            let active = db
                .prepare("SELECT r2_key FROM images WHERE r2_key=? LIMIT 1")
                .bind(&[JsValue::from_str(&item.r2_key)])?
                .first::<KeyRow>(None)
                .await?;
            if active.is_some() {
                return Ok(json!({"ok": false, "error": "image was restored or replaced"}));
            }
            // A replay confirms the gallery result, but must not delete old Telegram
            // targets again after an undo or publication change. Receipts retain them.
            let pending: Vec<Value> = snapshot["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|target| {
                    json!({"r2_key": item.r2_key, "decision_id": item.decision_id,
                    "publication_id": target["publication_id"], "chat_id": target["chat_id"],
                    "message_id": target["message_id"]})
                })
                .collect();
            return Ok(json!({"ok": true, "replayed": true, "telegram_pending": pending}));
        }
    }
    if !can_start {
        return Ok(
            json!({"ok":false,"error":"similar results are pending; retry after Oracle computation"}),
        );
    }
    if survivors.iter().any(Vec::is_empty) {
        return Ok(
            json!({"ok":false,"error":"keep at least one online image outside the delete set"}),
        );
    }
    for action in ["prepare", "delete"] {
        let response = crate::image_review::apply_with_survivors(
            env,
            crate::image_review::ImageRequest {
                decision_id: item.decision_id.clone(),
                r2_key: item.r2_key.clone(),
                action: action.into(),
                allow_missing_telegram: true,
                restored: Vec::new(),
            },
            survivors,
            generation,
        )
        .await?;
        let result = prepare_json(response).await?;
        if result.get("ok") != Some(&Value::Bool(true)) {
            return Ok(result);
        }
        if action == "delete" {
            let mut failed = Vec::new();
            if let Some(targets) = result.get("targets").and_then(Value::as_array) {
                for target in targets {
                    let (Some(chat_id), Some(message_id)) =
                        (target["chat_id"].as_i64(), target["message_id"].as_i64())
                    else {
                        continue;
                    };
                    if let Err(error) = delete_telegram_message(bot, chat_id, message_id).await {
                        failed.push(
                            json!({"r2_key": item.r2_key, "decision_id": item.decision_id,
                            "publication_id": target["publication_id"], "chat_id": chat_id,
                            "message_id": message_id, "error": error}),
                        );
                    }
                }
            }
            return Ok(
                json!({"ok": true, "replayed": false, "telegram_failed": failed,
                "generation": result.get("generation")}),
            );
        }
    }
    unreachable!()
}

/// Index of the copy to keep among strict duplicates: most pixels, then most
/// bytes; on a tie the earlier upload stays, so a repost never replaces it.
fn dedup_keeper(members: &[(ImageFingerprint, String)]) -> usize {
    (0..members.len())
        .max_by(|&a, &b| {
            let (left, left_at) = &members[a];
            let (right, right_at) = &members[b];
            (u64::from(left.width) * u64::from(left.height), left.bytes)
                .cmp(&(
                    u64::from(right.width) * u64::from(right.height),
                    right.bytes,
                ))
                .then_with(|| right_at.cmp(left_at))
                .then_with(|| right.r2_key.cmp(&left.r2_key))
        })
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct DedupMeta {
    r2_key: String,
    work_id: String,
    created_at: String,
    review_version: i64,
}

#[derive(Deserialize)]
struct StateRow {
    state: String,
}

const ACTIVE_FINGERPRINTS: &str = r#"
    SELECT f.r2_key,f.work_id,f.solid_color,f.content_sha256,f.strict_key,
           f.average_hash,f.difference_hash,f.color_key,f.detail_key,
           f.width,f.height,f.bytes,f.format,f.regions_json,w.source,w.title
    FROM image_fingerprints f CROSS JOIN images i ON i.r2_key=f.r2_key
    CROSS JOIN works w ON w.id=i.work_id
    WHERE f.work_id=i.work_id AND f.solid_color=0 AND w.deleted_at IS NULL"#;

/// Keep only the best copy of `r2_key`'s strict duplicates in other active works,
/// removing the rest through the recoverable image review the admin uses
/// (review-trash backup, D1 update, Telegram message deletion).
pub(crate) async fn dedup_image(env: &Env, r2_key: &str) -> Result<Value> {
    let db = env.d1("DB")?;
    let Some(target) = db
        .prepare(format!("{ACTIVE_FINGERPRINTS} AND f.r2_key=?"))
        .bind(&[JsValue::from_str(r2_key)])?
        .first::<FingerprintRow>(None)
        .await?
        .and_then(row_to_fingerprint)
    else {
        return Ok(json!({ "ok": true, "removed": [] }));
    };
    let mut members: Vec<ImageFingerprint> = db
        .prepare(format!(
            "{ACTIVE_FINGERPRINTS} AND f.work_id!=? AND (f.content_sha256=? OR (f.color_key=? AND f.detail_key=?))"
        ))
        .bind(&[
            JsValue::from_str(&target.work_id),
            JsValue::from_str(&target.content_sha256),
            JsValue::from_str(&target.color_key),
            JsValue::from_str(&target.detail_key),
        ])?
        .all()
        .await?
        .results::<FingerprintRow>()?
        .into_iter()
        .filter_map(row_to_fingerprint)
        .filter(|other| classify_similarity(&target, other) == MatchKind::StrictSame)
        .collect();
    if members.is_empty() {
        return Ok(json!({ "ok": true, "removed": [] }));
    }
    members.push(target);
    let keys: Vec<&str> = members.iter().map(|m| m.r2_key.as_str()).collect();
    let metas: std::collections::HashMap<String, DedupMeta> = db
        .prepare("SELECT i.r2_key,i.work_id,i.created_at,w.review_version FROM images i JOIN works w ON w.id=i.work_id WHERE i.r2_key IN (SELECT value FROM json_each(?))")
        .bind(&[JsValue::from_str(&serde_json::to_string(&keys).unwrap())])?
        .all()
        .await?
        .results::<DedupMeta>()?
        .into_iter()
        .map(|meta| (meta.r2_key.clone(), meta))
        .collect();
    let ranked: Vec<(ImageFingerprint, String)> = members
        .into_iter()
        .filter_map(|m| Some((metas.get(&m.r2_key)?.created_at.clone(), m)))
        .map(|(created_at, m)| (m, created_at))
        .collect();
    let keep = dedup_keeper(&ranked);
    let keeper_meta = &metas[&ranked[keep].0.r2_key];
    let keeper = crate::image_review::SimilarKeeper {
        r2_key: keeper_meta.r2_key.clone(),
        work_id: keeper_meta.work_id.clone(),
        review_version: keeper_meta.review_version,
    };
    let bot = env
        .secret("TELEGRAM_BOT_TOKEN")
        .map(|value| value.to_string())
        .or_else(|_| env.var("TELEGRAM_BOT_TOKEN").map(|value| value.to_string()))
        .unwrap_or_default();
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for (index, (image, _)) in ranked.iter().enumerate() {
        if index == keep {
            continue;
        }
        let decision_id = format!(
            "dedup-{}",
            &sha256_hex(format!("{}\n{}", image.r2_key, keeper.r2_key).as_bytes())[..40]
        );
        // A restored decision means the owner chose to keep this copy.
        let prior = db
            .prepare("SELECT state FROM catalog_image_reviews WHERE decision_id=?")
            .bind(&[JsValue::from_str(&decision_id)])?
            .first::<StateRow>(None)
            .await?;
        if prior.is_some_and(|row| row.state != "prepared") {
            continue;
        }
        let mut outcome = None;
        for action in ["prepare", "delete"] {
            let response = crate::image_review::apply_with_survivors(
                env,
                crate::image_review::ImageRequest {
                    decision_id: decision_id.clone(),
                    r2_key: image.r2_key.clone(),
                    action: action.into(),
                    allow_missing_telegram: true,
                    restored: Vec::new(),
                },
                &[vec![keeper.clone()]],
                None,
            )
            .await?;
            let result = prepare_json(response).await?;
            if result.get("ok") != Some(&Value::Bool(true)) {
                outcome = Some(Err(result));
                break;
            }
            outcome = Some(Ok(result));
        }
        match outcome {
            Some(Ok(result)) => {
                for target in result["targets"].as_array().into_iter().flatten() {
                    if let (Some(chat_id), Some(message_id)) =
                        (target["chat_id"].as_i64(), target["message_id"].as_i64())
                    {
                        if let Err(error) = delete_telegram_message(&bot, chat_id, message_id).await
                        {
                            console_error!(
                                "dedup telegram delete failed for {}: {error}",
                                image.r2_key
                            );
                        }
                    }
                }
                removed.push(image.r2_key.clone());
            }
            Some(Err(result)) => {
                failed.push(json!({"r2_key": image.r2_key, "error": result.get("error")}))
            }
            None => {}
        }
    }
    Ok(json!({ "ok": true, "keep": keeper.r2_key, "removed": removed, "failed": failed }))
}

/// POST /api/catalog/dedup `{ "r2_key": "..." }` (Bearer): dedup one image on demand.
pub async fn handle_dedup(mut req: Request, env: Env) -> Result<Response> {
    #[derive(Deserialize)]
    struct Body {
        r2_key: String,
    }
    let Ok(body) = req.json::<Body>().await else {
        return reject(400, "invalid json");
    };
    if body.r2_key.is_empty() || body.r2_key.len() > 1024 {
        return reject(400, "invalid r2_key");
    }
    Ok(with_cors(json_response(
        &dedup_image(&env, &body.r2_key).await?,
        200,
    )?))
}

async fn prepare_json(mut response: Response) -> Result<Value> {
    let text = response.text().await?;
    serde_json::from_str(&text).map_err(|e| Error::RustError(e.to_string()))
}

async fn delete_telegram_message(
    token: &str,
    chat_id: i64,
    message_id: i64,
) -> std::result::Result<(), String> {
    if token.is_empty() {
        return Err("TELEGRAM_BOT_TOKEN not configured".into());
    }
    let url = format!("https://api.telegram.org/bot{token}/deleteMessage");
    let headers = Headers::new();
    headers
        .set("content-type", "application/json")
        .map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(
            &json!({ "chat_id": chat_id, "message_id": message_id }).to_string(),
        )));
    let request =
        Request::new_with_init(&url, &init).map_err(|_| "invalid Telegram request".to_string())?;
    let mut response = Fetch::Request(request)
        .send()
        .await
        .map_err(|_| "Telegram request failed".to_string())?;
    if response.status_code() >= 400 {
        return Err(format!("telegram HTTP {}", response.status_code()));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| "invalid Telegram response".to_string())?;
    if body.get("ok") != Some(&Value::Bool(true)) {
        return Err("Telegram deletion was not confirmed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_keeps_best_quality_then_earliest_upload() {
        let image = |key: &str, width: u32, bytes: u64| ImageFingerprint {
            r2_key: key.into(),
            work_id: format!("x:{key}"),
            solid_color: false,
            content_sha256: "same".into(),
            strict_key: String::new(),
            average_hash: 0,
            difference_hash: 0,
            color_key: String::new(),
            detail_key: String::new(),
            width,
            height: 100,
            bytes,
            format: "JPEG".into(),
            regions: Vec::new(),
            source: "x".into(),
            title: String::new(),
        };
        let at = |s: &str| s.to_string();
        // More pixels beats more bytes and an earlier upload.
        let members = [
            (image("old", 100, 900), at("2026-01-01")),
            (image("new", 200, 100), at("2026-02-01")),
        ];
        assert_eq!(dedup_keeper(&members), 1);
        // Same pixels: more bytes wins.
        let members = [
            (image("old", 100, 100), at("2026-01-01")),
            (image("new", 100, 200), at("2026-02-01")),
        ];
        assert_eq!(dedup_keeper(&members), 1);
        // Identical quality: the earlier upload stays, whatever the order.
        let members = [
            (image("new", 100, 100), at("2026-02-01")),
            (image("old", 100, 100), at("2026-01-01")),
        ];
        assert_eq!(dedup_keeper(&members), 1);
    }

    #[test]
    fn offline_contract_preserves_strict_only_and_empty_results() {
        let empty: SimilarInput = serde_json::from_value(json!({
            "generation": 1, "algorithm": SIMILAR_ALGORITHM, "fingerprints": [], "clip": []
        }))
        .unwrap();
        let result = compute_offline(empty).unwrap();
        assert!(valid_result(&result));
        assert_eq!(result.response.engine, "hash");
        assert!(result.response.groups.is_empty() && result.response.auto_pairs.is_empty());
        let mut first = json!({
            "r2_key":"a", "work_id":"pixiv:1", "solid_color":0,
            "content_sha256":"same", "strict_key":"same", "average_hash":"0", "difference_hash":"0",
            "color_key":"color", "detail_key":"detail", "width":100, "height":100, "bytes":10,
            "format":"png", "regions_json":"[]", "source":"pixiv", "title":"first"
        });
        let mut second = first.clone();
        second["r2_key"] = json!("b");
        second["work_id"] = json!("pixiv:2");
        let input = |a: &Value, b: &Value, clip: Value| {
            serde_json::from_value(json!({
                "generation": 1, "algorithm": SIMILAR_ALGORITHM, "fingerprints": [a,b], "clip":clip
            }))
            .unwrap()
        };
        let mut strict = compute_offline(input(&first, &second, json!([]))).unwrap();
        assert_eq!(strict.response.auto_pairs.len(), 1);
        assert!(valid_result(&strict));
        strict.response.auto_pairs[0].remove.kind = "clip 1.000".into();
        assert!(!valid_result(&strict));
        strict.response.auto_pairs[0].remove.kind = "strict".into();
        strict.algorithm = "old algorithm".into();
        assert!(!valid_result(&strict));
        first["content_sha256"] = json!("first");
        second["content_sha256"] = json!("second");
        second["average_hash"] = json!("ffffffffffffffff");
        second["difference_hash"] = json!("ffffffffffffffff");
        let mut vector = vec![0.0_f32; CLIP_DIM];
        vector[0] = 1.0;
        let encoded = hex::encode(
            vector
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let semantic = compute_offline(input(
            &first,
            &second,
            json!([
                {"r2_key":"a","embedding":encoded}, {"r2_key":"b","embedding":encoded}
            ]),
        ))
        .unwrap();
        assert_eq!(semantic.response.engine, "clip");
        assert_eq!(semantic.response.groups.len(), 1);
        assert!(semantic.response.auto_pairs.is_empty());
        assert!(valid_result(&semantic));
        let old: SimilarInput = serde_json::from_value(json!({
            "generation":1,"algorithm":"old","fingerprints":[],"clip":[]
        }))
        .unwrap();
        assert!(compute_offline(old).is_err());
    }

    fn all_pairs_reference(images: &[ImageFingerprint]) -> Vec<SimilarGroup> {
        let mut sets = DisjointSet::new(images.len());
        for left in 0..images.len() {
            for right in (left + 1)..images.len() {
                if images[left].work_id != images[right].work_id
                    && classify_similarity(&images[left], &images[right]) != MatchKind::Different
                {
                    sets.union(left, right);
                }
            }
        }
        groups_from_sets(images, sets)
    }

    #[test]
    fn incomplete_clip_coverage_never_affects_hash_output() {
        assert!(!has_clip_coverage(2402, 4315));
        assert!(!has_clip_coverage(3451, 4315));
        assert!(has_clip_coverage(3452, 4315));
        assert!(!has_clip_coverage(0, 0));
        let images = vec![
            fp("a", "pixiv:1", "same", 1, 100, 100, 10),
            fp("b", "pixiv:2", "same", 1, 100, 100, 10),
        ];
        let incomplete = std::collections::HashMap::from([("a".into(), vec![1.0; CLIP_DIM])]);
        assert_eq!(
            similar_view(&images, &incomplete),
            similar_view(&images, &std::collections::HashMap::new())
        );
    }

    #[test]
    fn old_candidates_require_current_members_and_matches() {
        let a = fp("a", "pixiv:1", "first", 1, 200, 200, 20);
        let b = fp("b", "pixiv:2", "second", u64::MAX, 100, 100, 10);
        let mut vector = vec![0.0; CLIP_DIM];
        vector[0] = 1.0;
        let mut embeddings =
            std::collections::HashMap::from([("a".into(), vector.clone()), ("b".into(), vector)]);
        let response = SimilarResponse {
            ok: true,
            engine: "clip".into(),
            groups: cluster_clip(&[a.clone(), b.clone()], &embeddings),
            auto_pairs: vec![],
        };
        assert_eq!(response.groups.len(), 1);
        let mut images = std::collections::HashMap::from([("a".into(), a), ("b".into(), b)]);
        images.insert("new".into(), fp("new", "pixiv:3", "new", 42, 100, 100, 10));
        assert_eq!(
            revalidate_candidates(response.clone(), &images, &embeddings),
            response
        );
        images.get_mut("b").unwrap().work_id = "replaced".into();
        assert!(
            revalidate_candidates(response.clone(), &images, &embeddings)
                .groups
                .is_empty()
        );
        images.get_mut("b").unwrap().work_id = "pixiv:2".into();
        embeddings.get_mut("b").unwrap().swap(0, 1);
        assert!(
            revalidate_candidates(response.clone(), &images, &embeddings)
                .groups
                .is_empty()
        );
        embeddings.remove("b");
        assert!(revalidate_candidates(response, &images, &embeddings)
            .groups
            .is_empty());

        let a = fp("a", "pixiv:1", "same", 1, 200, 200, 20);
        let b = fp("b", "pixiv:2", "same", 1, 100, 100, 10);
        let response = SimilarResponse {
            ok: true,
            engine: "hash".into(),
            groups: vec![],
            auto_pairs: vec![AutoPair {
                keep: item_from(&a, MatchKind::StrictSame, true),
                remove: item_from(&b, MatchKind::StrictSame, false),
            }],
        };
        let mut images = std::collections::HashMap::from([("a".into(), a), ("b".into(), b)]);
        assert_eq!(
            revalidate_candidates(response.clone(), &images, &embeddings)
                .auto_pairs
                .len(),
            1
        );
        images.get_mut("b").unwrap().width = 1000;
        assert!(
            revalidate_candidates(response.clone(), &images, &embeddings)
                .auto_pairs
                .is_empty()
        );
        images.remove("a");
        assert!(revalidate_candidates(response, &images, &embeddings)
            .auto_pairs
            .is_empty());
    }

    #[test]
    fn clip_excludes_solid_images_even_with_identical_embeddings() {
        let mut images = vec![
            fp("blank", "pixiv:1", "blank", 1, 200, 200, 20),
            fp("visible", "pixiv:2", "visible", 2, 100, 100, 10),
        ];
        let mut vector = vec![0.0; CLIP_DIM];
        vector[0] = 1.0;
        let embeddings = std::collections::HashMap::from([
            ("blank".into(), vector.clone()),
            ("visible".into(), vector),
        ]);
        assert_eq!(cluster_clip(&images, &embeddings).len(), 1);
        images[0].solid_color = true;
        assert!(cluster_clip(&images, &embeddings).is_empty());
        assert!(similar_view(&images, &embeddings).0.is_empty());
    }

    #[test]
    fn candidate_clustering_matches_all_pairs() {
        for seed in 1..=5 {
            let mut random = seed;
            let mut next = || {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random
            };
            let mut images: Vec<_> = (0..128)
                .map(|index| {
                    let bits = next();
                    let mut image = fp(
                        &format!("{index}"),
                        &format!("work-{}", index % 13),
                        &format!("sha-{}", index % 29),
                        if index % 3 == 0 { bits } else { bits & 255 },
                        (index % 7) * 100,
                        (index % 5) * 100,
                        100 + index as u64,
                    );
                    image.difference_hash = if index % 3 == 0 { next() } else { next() & 255 };
                    image.color_key = format!("color-{}", index % 5);
                    image.detail_key = format!("detail-{}", index % 3);
                    image.solid_color = index % 17 == 0;
                    image
                })
                .collect();
            for index in 0..images.len() {
                let target = &images[(index * 7 + seed as usize) % images.len()];
                let region = (
                    target.width,
                    target.height,
                    target.average_hash ^ (1 << (index % 64)),
                    target.difference_hash,
                    target.color_key.clone(),
                );
                images[index].regions = vec![region.clone(), region];
            }
            assert_eq!(
                cluster_similar(&images),
                all_pairs_reference(&images),
                "seed {seed}"
            );
            images.reverse();
            assert_eq!(
                cluster_similar(&images),
                all_pairs_reference(&images),
                "reversed seed {seed}"
            );
        }
    }

    #[test]
    #[ignore = "representative local performance measurement"]
    fn similar_representative_scale() {
        let mut random = 1_u64;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            random
        };
        let images: Vec<_> = (0..4315)
            .map(|index| {
                let mut image = fp(
                    &format!("pixiv/{index}/0.png"),
                    &format!("pixiv:{index}"),
                    &format!("{index:064x}"),
                    next(),
                    1200,
                    1800,
                    2_000_000,
                );
                image.difference_hash = next();
                image.color_key = format!("{:06x}", next() & 0xff_ffff);
                image.regions = (0..12)
                    .map(|_| (900, 1200, next(), next(), image.color_key.clone()))
                    .collect();
                image
            })
            .collect();
        let embeddings: std::collections::HashMap<_, _> = images[..2402]
            .iter()
            .map(|image| {
                let values = (0..CLIP_DIM)
                    .map(|_| (next() as f64 / u64::MAX as f64 - 0.5) as f32)
                    .collect();
                (image.r2_key.clone(), values)
            })
            .collect();
        let started = std::time::Instant::now();
        let mut sorted_images: Vec<_> = images.iter().collect();
        sorted_images.sort_by(|a, b| a.r2_key.cmp(&b.r2_key));
        let mut sorted_embeddings: Vec<_> = embeddings.iter().collect();
        sorted_embeddings.sort_by(|a, b| a.0.cmp(b.0));
        let bytes = serde_json::to_vec(&(sorted_images, sorted_embeddings)).unwrap();
        let old_revision = sha256_hex(&bytes);
        eprintln!(
            "old revision {:?}, serialized {} bytes",
            started.elapsed(),
            bytes.len()
        );
        assert_eq!(old_revision.len(), 64);
        drop(bytes);
        let started = std::time::Instant::now();
        let revision = similar_revision(&images, &embeddings).unwrap();
        eprintln!("new revision {:?}", started.elapsed());
        assert!(revision.starts_with(SIMILAR_CACHE_VERSION));
        let hash_only = std::collections::HashMap::new();
        assert!(!has_clip_coverage(embeddings.len(), images.len()));
        let started = std::time::Instant::now();
        similar_revision(&images, &hash_only).unwrap();
        eprintln!(
            "hash-only revision {:?}, no CLIP vectors loaded",
            started.elapsed()
        );
        let started = std::time::Instant::now();
        let reference = split_auto_pairs(all_pairs_reference(&images), &images);
        eprintln!("old hash view {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let (groups, pairs) = similar_view(&images, &hash_only);
        eprintln!(
            "new hash view {:?}, {} groups, {} pairs",
            started.elapsed(),
            groups.len(),
            pairs.len()
        );
        assert_eq!((groups.clone(), pairs), reference);
        assert!(groups.len() <= MAX_SIMILAR_GROUPS);
    }

    #[test]
    fn delete_sets_require_stable_distinct_decisions() {
        let item = |key: &str, decision: &str| DeleteItem {
            r2_key: key.into(),
            decision_id: decision.into(),
            keep_r2_keys: Vec::new(),
        };
        assert!(valid_delete_items(&[
            item("a", "sim-12345678"),
            item("b", "sim-87654321")
        ]));
        assert!(!valid_delete_items(&[
            item("a", "sim-12345678"),
            item("a", "sim-87654321")
        ]));
        assert!(!valid_delete_items(&[
            item("a", "sim-12345678"),
            item("b", "sim-12345678")
        ]));
        assert!(!valid_delete_items(&[item("a", "sim-../secret")]));
        assert!(!valid_delete_items(&[item("a", "")]));
        let mut removed_keeper = item("a", "sim-12345678");
        removed_keeper.keep_r2_keys.push("a".into());
        assert!(!valid_delete_items(&[removed_keeper]));
    }

    #[test]
    fn cache_revision_tracks_content_membership_metadata_and_clip_not_counts() {
        let original = fp("a", "pixiv:1", "sha", 1, 100, 100, 10);
        let other = fp("b", "pixiv:2", "sha", 1, 100, 100, 10);
        let embeddings = std::collections::HashMap::from([("a".into(), vec![1.0, 0.0])]);
        let revision = similar_revision(&[original.clone(), other.clone()], &embeddings).unwrap();
        assert_eq!(
            revision,
            similar_revision(&[other.clone(), original.clone()], &embeddings).unwrap()
        );
        for field in [
            "hash",
            "difference",
            "key",
            "title",
            "quality",
            "height",
            "bytes",
            "sha",
            "strict",
            "solid",
            "color",
            "detail",
            "regions",
            "source",
            "format",
        ] {
            let mut changed = original.clone();
            match field {
                "hash" => changed.average_hash = 2,
                "difference" => changed.difference_hash = 2,
                "key" => changed.r2_key = "replacement".into(),
                "title" => changed.title = "edited title".into(),
                "quality" => changed.width = 200,
                "height" => changed.height = 200,
                "bytes" => changed.bytes = 20,
                "sha" => changed.content_sha256 = "replacement".into(),
                "strict" => changed.strict_key = "replacement".into(),
                "solid" => changed.solid_color = true,
                "color" => changed.color_key = "different".into(),
                "detail" => changed.detail_key = "different".into(),
                "regions" => changed.regions.push((50, 50, 1, 1, "region".into())),
                "source" => changed.source = "x".into(),
                _ => changed.format = "png".into(),
            }
            assert_ne!(
                revision,
                similar_revision(&[changed, other.clone()], &embeddings).unwrap()
            );
        }
        let changed = std::collections::HashMap::from([("a".into(), vec![0.0, 1.0])]);
        assert_ne!(
            revision,
            similar_revision(&[original, other], &changed).unwrap()
        );
        let nan = vec![f32::NAN.to_le_bytes(); CLIP_DIM].concat();
        assert!(parse_clip_embedding(&hex::encode(nan)).is_none());
    }

    fn fp(
        r2: &str,
        work: &str,
        sha: &str,
        avg: u64,
        w: u32,
        h: u32,
        bytes: u64,
    ) -> ImageFingerprint {
        ImageFingerprint {
            r2_key: r2.into(),
            work_id: work.into(),
            solid_color: false,
            content_sha256: sha.into(),
            strict_key: sha.into(),
            average_hash: avg,
            difference_hash: avg,
            color_key: "c".into(),
            detail_key: "d".into(),
            width: w,
            height: h,
            bytes,
            format: "jpeg".into(),
            regions: Vec::new(),
            source: "pixiv".into(),
            title: work.into(),
        }
    }

    #[test]
    fn same_sha_is_strict() {
        let left = fp("a", "pixiv:1", "sha", 1, 10, 10, 10);
        let right = fp("b", "pixiv:2", "sha", 99, 1, 1, 1);
        assert_eq!(classify_similarity(&left, &right), MatchKind::StrictSame);
    }

    #[test]
    fn same_work_pages_are_not_clustered() {
        let images = vec![
            fp("a", "pixiv:1", "sha-a", 1, 100, 100, 10),
            fp("b", "pixiv:1", "sha-a", 1, 100, 100, 10),
        ];
        assert!(cluster_similar(&images).is_empty());
    }

    #[test]
    fn cluster_keeps_higher_resolution() {
        let images = vec![
            fp("low", "pixiv:1", "same", 1, 10, 10, 10),
            fp("high", "pixiv:2", "same", 1, 20, 20, 10),
        ];
        let groups = cluster_similar(&images);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items[0].r2_key, "high");
        assert!(groups[0].items[0].keep);
        assert!(!groups[0].items[1].keep);
    }

    #[test]
    fn reject_deleting_entire_group() {
        let group = SimilarGroup {
            id: "g".into(),
            items: vec![
                SimilarItem {
                    r2_key: "a".into(),
                    work_id: "pixiv:1".into(),
                    source: "pixiv".into(),
                    title: String::new(),
                    width: 1,
                    height: 1,
                    bytes: 1,
                    kind: "strict".into(),
                    distance: 0,
                    keep: true,
                },
                SimilarItem {
                    r2_key: "b".into(),
                    work_id: "pixiv:2".into(),
                    source: "pixiv".into(),
                    title: String::new(),
                    width: 1,
                    height: 1,
                    bytes: 1,
                    kind: "strict".into(),
                    distance: 0,
                    keep: false,
                },
            ],
        };
        assert!(reject_empty_groups(
            std::slice::from_ref(&group),
            &["a".into(), "b".into()]
        ));
        assert!(!reject_empty_groups(
            std::slice::from_ref(&group),
            &["b".into()]
        ));
    }

    #[test]
    fn skip_key_ignores_member_order() {
        let a = skip_key_from_r2(["b", "a"].into_iter());
        let b = skip_key_from_r2(["a", "b"].into_iter());
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn group_id_is_64_hex() {
        assert!(valid_group_id(&"ab".repeat(32)));
        assert!(!valid_group_id("nope"));
        assert!(!valid_group_id(&"g".repeat(64)));
    }

    #[test]
    fn hash_parser_reads_hex() {
        assert_eq!(parse_hash("0000000000000001"), Some(1));
        assert_eq!(parse_hash("not-hex"), None);
    }

    #[test]
    fn cosine_of_normalized_pair() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    fn xy_emb(x: f32, y: f32) -> Vec<f32> {
        let mut vec = vec![0.0; CLIP_DIM];
        vec[0] = x;
        vec[1] = y;
        vec
    }

    #[test]
    fn clip_does_not_chain_unrelated_neighbors() {
        let images = vec![
            fp("a", "pixiv:1", "a", 1, 10, 10, 10),
            fp("b", "pixiv:2", "b", 1, 10, 10, 10),
            fp("c", "pixiv:3", "c", 1, 10, 10, 10),
        ];
        let mut embeddings = std::collections::HashMap::new();
        embeddings.insert("a".into(), xy_emb(1.0, 0.0));
        embeddings.insert("b".into(), xy_emb(0.94, 0.341_565));
        embeddings.insert("c".into(), xy_emb(0.88, 0.474_973_68));
        let groups = cluster_clip(&images, &embeddings);
        assert!(
            groups.iter().all(|group| group.items.len() < 3),
            "transitive CLIP links must not form a 3-image blob: {groups:?}"
        );
        assert!(!groups.is_empty());
    }

    #[test]
    fn two_strict_images_become_auto_pairs() {
        let images = vec![
            fp("low", "pixiv:1", "same", 1, 10, 10, 10),
            fp("high", "pixiv:2", "same", 1, 20, 20, 10),
        ];
        let (review, auto_pairs) = split_auto_pairs(cluster_similar(&images), &images);
        assert!(review.is_empty());
        assert_eq!(auto_pairs.len(), 1);
        assert_eq!(auto_pairs[0].keep.r2_key, "high");
        assert_eq!(auto_pairs[0].remove.r2_key, "low");
    }

    #[test]
    fn high_clip_similarity_never_promotes_different_hashes_to_auto_cleanup() {
        let images = vec![
            fp("a", "pixiv:1", "sha-a", 0, 100, 100, 10),
            fp("b", "pixiv:2", "sha-b", u64::MAX, 100, 100, 10),
        ];
        assert_eq!(
            classify_similarity(&images[0], &images[1]),
            MatchKind::Different
        );
        let embeddings = std::collections::HashMap::from([
            ("a".into(), xy_emb(1.0, 0.0)),
            ("b".into(), xy_emb(1.0, 0.0)),
        ]);
        let (review, auto_pairs) = similar_view(&images, &embeddings);
        assert!(auto_pairs.is_empty());
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].items[1].kind, "clip 1.000");
        for stale_label in ["dup", "strict"] {
            let mut group = review[0].clone();
            group.items[1].kind = stale_label.into();
            assert!(split_auto_pairs(vec![group], &images).1.is_empty());
        }
    }
}
