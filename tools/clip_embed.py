#!/usr/bin/env python3
"""Oracle-side CLIP embedder. Downloads missing images and PUTs embeddings."""

from __future__ import annotations

import argparse
import io
import http.client
import json
import math
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from itertools import islice
from pathlib import Path


ENDPOINT = "https://gallery.fontaine.blue"
MODEL_NAME = "ViT-B-32"
PRETRAINED = "openai"
DIM = 512
MAX_IMAGE_BYTES = 50 * 1024 * 1024
MAX_API_BYTES = 8 * 1024 * 1024
MAX_BATCH = 20
DOWNLOAD_WORKERS = 4


class HTTPFailure(RuntimeError):
    def __init__(self, code: int):
        self.code = code
        super().__init__(f"HTTP {code}")


def load_token(env_path: str) -> str:
    for line in Path(env_path).read_text().splitlines():
        if line.startswith("HANABI_GALLERY_TOKEN="):
            token = line.split("=", 1)[1].strip()
            if token:
                return token
    raise RuntimeError(f"missing HANABI_GALLERY_TOKEN in {env_path}")


def request_bytes(request, limit: int, timeout: int) -> bytes:
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                raw = response.read(limit + 1)
            if len(raw) > limit:
                raise RuntimeError(f"response exceeds {limit} bytes")
            return raw
        except urllib.error.HTTPError as exc:
            exc.close()
            if not (exc.code == 429 or 500 <= exc.code <= 599) or attempt == 2:
                raise HTTPFailure(exc.code) from None
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.IncompleteRead):
            if attempt == 2:
                raise RuntimeError("request failed after 3 attempts") from None
        time.sleep(2 ** (attempt + 1))
    raise AssertionError("unreachable")


def api(token: str, method: str, path: str, data=None):
    body = None if data is None else json.dumps(data).encode()
    req = urllib.request.Request(
        ENDPOINT + path,
        data=body,
        method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "vitrine-clip-embed/1",
        },
    )
    result = json.loads(request_bytes(req, MAX_API_BYTES, 120))
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise RuntimeError(f"{method} {path}: invalid API response")
    return result


def missing_keys(token: str) -> list[str]:
    keys = api(token, "GET", "/api/catalog/clip/missing").get("missing")
    if not isinstance(keys, list) or any(not isinstance(key, str) or not key for key in keys):
        raise RuntimeError("invalid missing image keys")
    return list(dict.fromkeys(keys))


def media_url(r2_key: str) -> str:
    parts = ["/media"] + [urllib.parse.quote(part, safe="") for part in r2_key.split("/")]
    return ENDPOINT + "/".join(parts)


def download_image(key: str) -> bytes:
    request = urllib.request.Request(media_url(key), headers={"User-Agent": "vitrine-clip-embed/1"})
    return request_bytes(request, MAX_IMAGE_BYTES, 60)


def prefetch_images(keys):
    keys = iter(keys)
    with ThreadPoolExecutor(max_workers=DOWNLOAD_WORKERS) as pool:
        pending = deque((key, pool.submit(download_image, key)) for key in islice(keys, DOWNLOAD_WORKERS))
        try:
            while pending:
                key, future = pending.popleft()
                yield key, future
                del future
                key = next(keys, None)
                if key is not None:
                    pending.append((key, pool.submit(download_image, key)))
        finally:
            for _, future in pending:
                future.cancel()


def pack_embedding(vec) -> str:
    values = [float(value) for value in vec]
    if len(values) != DIM or not all(math.isfinite(value) for value in values):
        raise ValueError("embedding must contain 512 finite values")
    norm = math.sqrt(sum(value * value for value in values))
    if not math.isfinite(norm) or norm <= 0:
        raise ValueError("embedding must have a finite positive norm")
    return struct.pack("<" + "f" * DIM, *(value / norm for value in values)).hex()


def load_model(threads: int = 1):
    import open_clip
    import torch

    torch.set_num_threads(threads)
    torch.set_num_interop_threads(1)
    device = "cpu"
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL_NAME, pretrained=PRETRAINED, device=device
    )
    model.eval()
    return model, preprocess, device, torch


def embed_image(model, preprocess, device, torch, raw: bytes):
    from PIL import Image

    with Image.open(io.BytesIO(raw)) as source:
        image = source.convert("RGB")
    tensor = preprocess(image).unsqueeze(0)
    with torch.no_grad():
        feat = model.encode_image(tensor.to(device))
        feat = feat / feat.norm(dim=-1, keepdim=True)
    return feat.squeeze(0).cpu().numpy()


def push_batch(token: str, batch: list[dict]) -> None:
    result = api(token, "PUT", "/api/catalog/clip", {"images": batch})
    if type(result.get("upserted")) is not int or result["upserted"] != len(batch):
        raise RuntimeError("embedding upload count mismatch")


def scan(token: str, limit: int, batch_size: int, threads: int, quiet: bool) -> int:
    todo = missing_keys(token)
    pending = todo[:limit] if limit else todo
    if not pending:
        if not quiet:
            print(json.dumps({"missing": 0, "pushed": 0, "verified": 0, "failed": 0}), flush=True)
        return 0
    print(f"missing={len(todo)} this_run={len(pending)}", flush=True)
    model, preprocess, device, torch = load_model(threads)
    batch: list[dict] = []
    uploaded: set[str] = set()
    failed = 0
    skipped: set[str] = set()
    aborted = False
    try:
        with closing(prefetch_images(pending)) as downloads:
            for key, download in downloads:
                raw = None
                try:
                    raw = download.result()
                    vec = embed_image(model, preprocess, device, torch, raw)
                    item = {"r2_key": key, "dim": DIM, "embedding": pack_embedding(vec)}
                except HTTPFailure as exc:
                    if exc.code == 404:
                        skipped.add(key)
                        print(f"skip removed {key}", flush=True)
                        continue
                    failed += 1
                    print(f"fail {key}: {exc}", file=sys.stderr, flush=True)
                    continue
                except Exception as exc:  # Keep scanning valid images after a corrupt original.
                    failed += 1
                    print(f"fail {key}: {type(exc).__name__}", file=sys.stderr, flush=True)
                    continue
                finally:
                    # Release this original before the generator starts the next download.
                    raw = download = None
                batch.append(item)
                if len(batch) == batch_size:
                    push_batch(token, batch)
                    uploaded.update(item["r2_key"] for item in batch)
                    print(f"pushed={len(uploaded)}", flush=True)
                    batch = []
        if batch:
            push_batch(token, batch)
            uploaded.update(item["r2_key"] for item in batch)
    except (OSError, ValueError, RuntimeError) as exc:
        aborted = True
        print(f"embedding upload failed: {exc}", file=sys.stderr, flush=True)
    remaining = set(missing_keys(token))
    verified = len(uploaded - remaining)
    failed += len(uploaded & remaining)
    # A removed image may return 404; a still-active missing original needs attention.
    failed += len(skipped & remaining)
    print(json.dumps({"pushed": len(uploaded), "verified": verified, "failed": failed,
                      "skipped": len(skipped), "remaining": len(remaining), "aborted": aborted}), flush=True)
    return 1 if failed or aborted else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", default="/opt/hanabi-container/hanabi.env")
    parser.add_argument("--limit", type=int, default=0, help="max new embeddings this run; 0 means all")
    parser.add_argument("--batch-size", type=int, default=MAX_BATCH)
    parser.add_argument("--threads", type=int, default=1, help="Torch CPU threads")
    parser.add_argument("--quiet", action="store_true", help="omit output when no images need embedding")
    args = parser.parse_args()
    if args.limit < 0 or not 1 <= args.batch_size <= MAX_BATCH or args.threads < 1:
        parser.error("limit must be >= 0, batch-size 1..20, and threads >= 1")
    try:
        return scan(load_token(args.env), args.limit, args.batch_size, args.threads, args.quiet)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"embedding scan failed: {exc}", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
