#!/usr/bin/env python3
"""Find gallery images that are crops of another image, verified on pixels.

Runs in the Oracle CLIP virtualenv (numpy, Pillow). Candidates are pairs whose CLIP
embeddings are close but whose shapes differ; each is checked by sliding the smaller
picture, at every scale that fits, over the larger one and taking the best normalized
cross-correlation. Small grayscale copies and every pair's result are cached, so a
daily run only downloads and checks pairs that involve new images.

    crop_match.py --input export.json --output crops.json --cache DIR [--minutes N]

The input holds the similarity export's `fingerprints` and `clip` lists; the output is
`[{"crop", "full", "score"}]` for every verified crop known to the cache.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from multiprocessing import Pool
from pathlib import Path

import numpy as np
from PIL import Image

ENDPOINT = "https://gallery.fontaine.blue"
MAX_IMAGE_BYTES = 50 * 1024 * 1024
# Pages of one work are compared from this CLIP score, other works from CROSS_MIN; most
# Furina pictures share a scene closely enough that a lower cross-work line is millions of pairs.
WITHIN_MIN = 0.75
CROSS_MIN = 0.90
# A placement scoring at least NCC_MIN is a crop, as is one scoring NCC_LARGE that covers at
# least LARGE_COVER of the larger picture: on the gallery's pairs, lower-scoring matches of
# small templates were texture coincidences, while larger ones were real crops.
NCC_MIN = 0.9
NCC_LARGE = 0.8
LARGE_COVER = 0.15
# Covering nearly all of the larger picture is a resize, which the hash groups handle.
MAX_COVER = 0.9
CACHE_SIDE = 192
BIG_SIDE = 160
MIN_TEMPLATE = 24
SCALE_STEP = 0.93
Image.MAX_IMAGE_PIXELS = None


def cache_path(cache: Path, key: str) -> Path:
    digest = hashlib.sha256(key.encode()).hexdigest()
    return cache / "thumbs" / digest[:2] / f"{digest}.png"


def fetch(cache: Path, key: str) -> str:
    """Store a small grayscale copy of the original; ?width= would spend image-resizing quota.
    Returns "ok", "gone" (the original cannot be read) or "retry" (try again next run)."""
    path = cache_path(cache, key)
    if path.exists():
        return "ok"
    url = ENDPOINT + "/media/" + "/".join(urllib.parse.quote(part, safe="") for part in key.split("/"))
    request = urllib.request.Request(url, headers={"User-Agent": "vitrine-crop-match/1"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                raw = response.read(MAX_IMAGE_BYTES + 1)
            if len(raw) > MAX_IMAGE_BYTES:
                return "gone"
            with Image.open(io.BytesIO(raw)) as image:
                image = image.convert("L")
                image.thumbnail((CACHE_SIDE, CACHE_SIDE), Image.LANCZOS)
                path.parent.mkdir(parents=True, exist_ok=True)
                temporary = path.with_suffix(".tmp")
                image.save(temporary, format="PNG")
                temporary.replace(path)
            return "ok"
        except urllib.error.HTTPError as error:
            if error.code not in (429, 500, 502, 503, 504):
                return "gone"
        except Image.UnidentifiedImageError:
            return "gone"
        except (OSError, ValueError):
            pass
        time.sleep(2 + attempt * 3)
    return "retry"


def load_gray(path: Path, longest: int) -> np.ndarray:
    with Image.open(path) as image:
        scale = longest / max(image.size)
        size = (max(1, round(image.width * scale)), max(1, round(image.height * scale)))
        return np.asarray(image.convert("L").resize(size, Image.BILINEAR), dtype=np.float64)


def best_placement(small_path: Path, big_path: Path) -> tuple[float, float]:
    """Best normalized cross-correlation of the small picture inside the big one, and the
    share of the big picture it then covers."""
    big = load_gray(big_path, BIG_SIDE)
    height, width = big.shape
    spectrum = np.fft.rfft2(big)
    sums = np.pad(big, ((1, 0), (1, 0))).cumsum(0).cumsum(1)
    squares = np.pad(big * big, ((1, 0), (1, 0))).cumsum(0).cumsum(1)
    with Image.open(small_path) as source:
        small = source.convert("L")
        small.load()
    best = (-1.0, 0.0)
    scale = min(width / small.width, height / small.height)
    while True:
        w, h = round(small.width * scale), round(small.height * scale)
        if min(w, h) < MIN_TEMPLATE:
            break
        cover = w * h / (width * height)
        if cover < MAX_COVER:
            template = np.asarray(small.resize((w, h), Image.BILINEAR), dtype=np.float64)
            template -= template.mean()
            norm = np.sqrt((template * template).sum())
            if norm > 1e-6:
                corr = np.fft.irfft2(spectrum * np.conj(np.fft.rfft2(template, big.shape)), big.shape)
                corr = corr[: height - h + 1, : width - w + 1]
                window = lambda a: a[h:, w:] - a[:-h, w:] - a[h:, :-w] + a[:-h, :-w]
                variance = window(squares) - window(sums) ** 2 / (w * h)
                score = corr / (np.sqrt(np.maximum(variance, 1e-6)) * norm)
                # Flat regions (a blank margin) would otherwise match any flat template.
                score[variance < 1e-3 * w * h] = -1
                peak = float(score.max())
                if peak > best[0]:
                    best = (peak, cover)
        scale *= SCALE_STEP
    return best


def verify(job):
    """The pair, the better direction's (score, cover), and which picture is the crop."""
    left, right, left_path, right_path = job
    forward, backward = best_placement(left_path, right_path), best_placement(right_path, left_path)
    if forward >= backward:
        return left, right, forward, left, right
    return left, right, backward, right, left


def is_crop(score: float, cover: float) -> bool:
    return score >= NCC_MIN or (score >= NCC_LARGE and cover >= LARGE_COVER)


def candidates(fingerprints: list[dict], clip: list[dict]) -> list[tuple[str, str]]:
    meta = {row["r2_key"]: row for row in fingerprints if not row.get("solid_color")}
    rows = [row for row in clip if row["r2_key"] in meta]
    if len(rows) < 2:
        return []
    keys = [row["r2_key"] for row in rows]
    vectors = np.stack([np.frombuffer(bytes.fromhex(row["embedding"]), "<f4") for row in rows])
    work = np.array([meta[key]["work_id"] for key in keys])
    area = np.array([max(1, meta[key]["width"] * meta[key]["height"]) for key in keys], dtype=float)
    aspect = np.array([meta[key]["width"] / max(1, meta[key]["height"]) for key in keys])
    pairs = []
    for start in range(0, len(keys), 1024):
        scores = vectors[start:start + 1024] @ vectors.T
        left, right = np.nonzero(scores >= WITHIN_MIN)
        left = left + start
        upper = right > left
        left, right = left[upper], right[upper]
        # Same shape and size is a duplicate or a variant, not a crop.
        shaped = (np.abs(np.log(aspect[left] / aspect[right])) > 0.05) | (np.abs(np.log(area[left] / area[right])) >= 0.1)
        left, right = left[shaped], right[shaped]
        close = (work[left] == work[right]) | (scores[left - start, right] >= CROSS_MIN)
        pairs += [(keys[a], keys[b]) for a, b in zip(left[close], right[close])]
    return pairs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--minutes", type=float, default=10.0)
    parser.add_argument("--workers", type=int, default=4)
    args = parser.parse_args()
    deadline = time.monotonic() + args.minutes * 60
    data = json.loads(args.input.read_text())
    pairs = candidates(data["fingerprints"], data["clip"])
    del data
    args.cache.mkdir(parents=True, exist_ok=True)
    store = args.cache / "pairs.json"
    try:
        known = json.loads(store.read_text())
    except (OSError, ValueError):
        known = {}
    todo = [pair for pair in pairs if "\n".join(pair) not in known]
    needed = sorted({key for pair in todo for key in pair})
    missing = [key for key in needed if not cache_path(args.cache, key).exists()]
    fetched, gone = 0, set()
    with ThreadPoolExecutor(args.workers) as pool:
        for start in range(0, len(missing), 64):
            if time.monotonic() > deadline:
                break
            batch = missing[start:start + 64]
            for key, outcome in zip(batch, pool.map(lambda key: fetch(args.cache, key), batch)):
                fetched += outcome == "ok"
                if outcome == "gone":
                    gone.add(key)
    # A pair with an unreadable original is settled as no crop, so it stops holding the run open.
    for left, right in todo:
        if left in gone or right in gone:
            known["\n".join((left, right))] = [-1.0, 0.0, left, right]
    todo = [pair for pair in todo if "\n".join(pair) not in known]
    jobs = [
        (left, right, cache_path(args.cache, left), cache_path(args.cache, right))
        for left, right in todo
        if cache_path(args.cache, left).exists() and cache_path(args.cache, right).exists()
    ]
    checked = 0
    with Pool(args.workers) as pool:
        for left, right, (score, cover), crop, full in pool.imap_unordered(verify, jobs, chunksize=32):
            known["\n".join((left, right))] = [round(min(score, 1.0), 4), round(cover, 4), crop, full]
            checked += 1
            if time.monotonic() > deadline:
                pool.terminate()
                break
    live = {"\n".join(pair) for pair in pairs}
    # Pairs whose images left the gallery are dropped; the thumbnails stay (a few KB each).
    known = {pair: result for pair, result in known.items() if pair in live}
    temporary = store.with_suffix(".tmp")
    temporary.write_text(json.dumps(known, separators=(",", ":")))
    temporary.replace(store)
    crops = [
        {"crop": crop, "full": full, "score": score}
        for score, cover, crop, full in known.values()
        if is_crop(score, cover)
    ]
    args.output.write_text(json.dumps(crops, separators=(",", ":")))
    left = len(todo) - checked
    print(
        f"crop pairs={len(pairs)} new={len(todo)} downloaded={fetched}/{len(missing)} "
        f"checked={checked} pending={left} crops={len(crops)}",
        flush=True,
    )
    return 3 if left else 0


if __name__ == "__main__":
    sys.exit(main())
