#!/usr/bin/env python3
"""Keep the Vitrine R2 bucket inside the free tier.

`archive` copies the earliest-ingested gallery originals to local disk once the
bucket passes the high-water mark, then asks the Worker to drop each R2 copy.
Objects that are not gallery images (review backups, other prefixes) stay in R2. `serve` answers the
Worker's fallback reads (and review backup copies) for archived keys.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ENDPOINT = "https://gallery.fontaine.blue"
# R2 free storage is 10 GB-month per account; leave room for a day of ingest.
HIGH_WATER = 8_000_000_000
LOW_WATER = 7_500_000_000
CHUNK = 1 << 20
XATTRS = ("content_type", "etag", "sha256")


def load_token(env_path: str) -> str:
    for line in Path(env_path).read_text().splitlines():
        if line.startswith("HANABI_GALLERY_TOKEN="):
            token = line.split("=", 1)[1].strip()
            if token:
                return token
    raise RuntimeError(f"missing HANABI_GALLERY_TOKEN in {env_path}")


def object_path(root: Path, key: str) -> Path | None:
    parts = key.split("/")
    if not key or len(key) > 1024 or any(p in ("", ".", "..") for p in parts):
        return None
    if any(ord(c) < 32 for c in key):
        return None
    return root.joinpath(*parts)


def read_meta(path: Path) -> dict:
    meta = {}
    for name in XATTRS:
        try:
            meta[name] = os.getxattr(path, f"user.{name}").decode()
        except OSError:
            meta[name] = ""
    return meta


def write_meta(path: Path, meta: dict) -> None:
    for name in XATTRS:
        if meta.get(name):
            os.setxattr(path, f"user.{name}", meta[name].encode())


def fsync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


# ---------- serve ----------


def make_handler(root: Path, token: str):
    class Handler(BaseHTTPRequestHandler):
        server_version = "vitrine-cold/1"

        def log_message(self, fmt, *args):
            sys.stderr.write("%s %s\n" % (self.command, fmt % args))

        def reply(self, status: int, body: bytes = b"") -> None:
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def target(self) -> Path | None:
            header = self.headers.get("Authorization", "")
            if not hmac.compare_digest(header.encode(), f"Bearer {token}".encode()):
                self.reply(401)
                return None
            raw = urllib.parse.urlsplit(self.path).path
            if not raw.startswith("/o/"):
                self.reply(404)
                return None
            path = object_path(root, urllib.parse.unquote(raw[3:]))
            if path is None:
                self.reply(400)
            return path

        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            path = self.target()
            if path is None:
                return
            try:
                handle = path.open("rb")
            except (FileNotFoundError, NotADirectoryError, IsADirectoryError):
                return self.reply(404)
            with handle:
                meta = read_meta(path)
                self.send_response(200)
                self.send_header("Content-Length", str(os.fstat(handle.fileno()).st_size))
                self.send_header("Content-Type", meta["content_type"] or "application/octet-stream")
                if meta["etag"]:
                    self.send_header("ETag", meta["etag"])
                if meta["sha256"]:
                    self.send_header("X-Sha256", meta["sha256"])
                self.end_headers()
                if self.command == "GET":
                    while chunk := handle.read(CHUNK):
                        self.wfile.write(chunk)

        def do_PUT(self):
            """Copy an archived object (review backups and their restores)."""
            path = self.target()
            if path is None:
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                source = object_path(root, json.loads(self.rfile.read(min(length, 4096)))["from"])
            except (ValueError, KeyError, TypeError):
                return self.reply(400)
            if source is None or not source.is_file():
                return self.reply(404)
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_name(f".{path.name}.{os.getpid()}.{time.monotonic_ns()}")
            os.link(source, tmp)  # same inode keeps bytes and xattrs
            os.replace(tmp, path)
            fsync_dir(path.parent)
            self.reply(200, b'{"ok":true}')

    return Handler


def serve(args) -> int:
    root = Path(args.root)
    root.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer((args.host, args.port), make_handler(root, load_token(args.env)))
    server.daemon_threads = True
    server.serve_forever()
    return 0


# ---------- archive ----------


def api(token: str, method: str, path: str, data=None):
    body = None if data is None else json.dumps(data).encode()
    request = urllib.request.Request(
        ENDPOINT + path,
        data=body,
        method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "vitrine-cold-archive/1",
        },
    )
    return urllib.request.urlopen(request, timeout=60)


def list_objects(token: str) -> list[dict]:
    objects, cursor = [], None
    while True:
        query = "?cursor=" + urllib.parse.quote(cursor) if cursor else ""
        with api(token, "GET", "/api/storage/objects" + query) as response:
            page = json.load(response)
        objects += page["objects"]
        cursor = page.get("cursor")
        if not cursor:
            return objects


def oldest_images(token: str):
    """Gallery images in ingest order, fetched a page at a time as needed."""
    query = ""
    while True:
        with api(token, "GET", "/api/storage/images" + query) as response:
            page = json.load(response)
        yield from page["images"]
        cursor = page.get("cursor")
        if not cursor:
            return
        query = "?" + urllib.parse.urlencode(
            {"after_created": cursor["created_at"], "after_id": cursor["id"]}
        )


def pick(objects: list[dict], images, high: int, low: int) -> list[dict]:
    """R2 objects of the earliest images whose removal brings the bucket to `low`."""
    total = sum(o["size"] for o in objects)
    if total <= high:
        return []
    in_r2 = {o["key"]: o for o in objects}
    chosen = []
    for image in images:
        if total <= low:
            break
        obj = in_r2.pop(image["r2_key"], None)
        if obj is not None:
            chosen.append(obj)
            total -= obj["size"]
    return chosen


def verify(meta: dict, size: int, sha256: str, md5: str, expected_size: int) -> str | None:
    if size != expected_size:
        return f"size {size} != {expected_size}"
    if meta["sha256"] and meta["sha256"] != sha256:
        return "sha256 mismatch"
    etag = meta["etag"].strip('"')
    if re.fullmatch(r"[0-9a-f]{32}", etag) and etag != md5:
        return "md5 mismatch"
    return None


def archive_one(token: str, root: Path, obj: dict) -> None:
    path = object_path(root, obj["key"])
    if path is None:
        raise RuntimeError(f"unsafe key {obj['key']!r}")
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.archive")
    query = "?key=" + urllib.parse.quote(obj["key"], safe="")
    sha, md5, size = hashlib.sha256(), hashlib.md5(), 0
    with api(token, "GET", "/api/storage/object" + query) as response, tmp.open("wb") as out:
        meta = {
            "content_type": response.headers.get("Content-Type", ""),
            "etag": response.headers.get("ETag", ""),
            "sha256": response.headers.get("X-Sha256", ""),
        }
        while chunk := response.read(CHUNK):
            sha.update(chunk)
            md5.update(chunk)
            size += len(chunk)
            out.write(chunk)
        out.flush()
        os.fsync(out.fileno())
    if meta["etag"] != obj["etag"]:
        tmp.unlink()
        raise RuntimeError("object changed since listing")
    problem = verify(meta, size, sha.hexdigest(), md5.hexdigest(), obj["size"])
    if problem:
        tmp.unlink()
        raise RuntimeError(problem)
    write_meta(tmp, meta)
    os.replace(tmp, path)
    fsync_dir(path.parent)
    # The Worker re-checks the etag and our copy's size before deleting from R2.
    with api(token, "POST", "/api/storage/archive", {"key": obj["key"], "etag": obj["etag"]}):
        pass


def archive(args) -> int:
    token = load_token(args.env)
    root = Path(args.root)
    objects = list_objects(token)
    total = sum(o["size"] for o in objects)
    chosen = pick(objects, oldest_images(token), args.high, args.low)
    print(f"R2 {len(objects)} objects, {total / 1e9:.3f} GB; archiving {len(chosen)}")
    failed = moved = 0
    for obj in chosen:
        try:
            archive_one(token, root, obj)
            moved += obj["size"]
        except Exception as exc:  # keep going; the next run retries this key
            failed += 1
            print(f"failed {obj['key']}: {exc}", file=sys.stderr)
    print(f"archived {moved / 1e9:.3f} GB, {failed} failed; R2 now about {(total - moved) / 1e9:.3f} GB")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", default="/opt/hanabi-container/hanabi.env")
    parser.add_argument("--root", default="/var/lib/vitrine-cold/objects")
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("serve")
    s.add_argument("--host", default="127.0.0.1")
    s.add_argument("--port", type=int, default=8797)
    a = sub.add_parser("archive")
    a.add_argument("--high", type=int, default=HIGH_WATER)
    a.add_argument("--low", type=int, default=LOW_WATER)
    args = parser.parse_args()
    return serve(args) if args.command == "serve" else archive(args)


if __name__ == "__main__":
    sys.exit(main())
