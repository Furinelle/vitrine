#!/usr/bin/env python3
"""Run Vitrine similarity on Oracle; publish only a complete, current snapshot."""

from __future__ import annotations

import argparse
import fcntl
import json
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


ENDPOINT = "https://gallery.fontaine.blue"
ALGORITHM = "strict-only-v2"
MAX_RESPONSE_BYTES = 8 * 1024 * 1024
MAX_RESULT_BYTES = 4 * 1024 * 1024
MAX_SNAPSHOT_BYTES = 128 * 1024 * 1024


class StaleSnapshot(Exception):
    pass


class QuotaExceeded(Exception):
    def __init__(self, retry_after: int):
        self.retry_after = retry_after


def load_token(env_path: str) -> str:
    for line in Path(env_path).read_text().splitlines():
        if line.startswith("HANABI_GALLERY_TOKEN="):
            token = line.split("=", 1)[1].strip()
            if token:
                return token
    raise RuntimeError(f"missing HANABI_GALLERY_TOKEN in {env_path}")


def api(token: str, endpoint: str, method: str, path: str, data=None):
    body = None if data is None else json.dumps(data, separators=(",", ":")).encode()
    request = urllib.request.Request(
        endpoint + path,
        data=body,
        method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "vitrine-similar-compute/1",
        },
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read(MAX_RESPONSE_BYTES + 1)
            if len(raw) > MAX_RESPONSE_BYTES:
                raise RuntimeError("API response exceeds 8 MiB")
            result = json.loads(raw)
            if not isinstance(result, dict) or result.get("ok") is not True:
                raise RuntimeError(f"{method} {path}: invalid API response")
            return result
        except urllib.error.HTTPError as exc:
            raw = exc.read(65536)
            exc.close()
            try:
                error = json.loads(raw)
            except (ValueError, UnicodeDecodeError):
                error = {}
            if isinstance(error, dict) and error.get("code") == "database_daily_limit":
                try:
                    retry_after = int(exc.headers.get("Retry-After", ""))
                except (TypeError, ValueError):
                    retry_after = 86400 - int(time.time()) % 86400
                raise QuotaExceeded(max(1, min(retry_after, 86400))) from exc
            if exc.code == 409:
                raise StaleSnapshot from exc
            if exc.code not in (429, 500, 502, 503, 504) or attempt == 2:
                raise RuntimeError(f"{method} {path}: HTTP {exc.code}") from exc
        except (urllib.error.URLError, TimeoutError) as exc:
            if attempt == 2:
                raise RuntimeError(f"{method} {path}: request failed") from exc
        time.sleep(2 ** (attempt + 1))
    raise AssertionError("unreachable")


def export_input(token: str, endpoint: str, snapshot: dict) -> dict:
    generation = snapshot["generation"]
    data = {"generation": generation, "algorithm": ALGORITHM}
    size = 0
    for kind, count_key in (("fingerprints", "fingerprint_count"), ("clip", "clip_count")):
        items = []
        cursor = ""
        while True:
            query = urllib.parse.urlencode(
                {"generation": generation, "kind": kind, "after": cursor, "limit": 100}
            )
            page = api(token, endpoint, "GET", "/api/catalog/similar/input?" + query)
            if page.get("generation") != generation or page.get("algorithm") != ALGORITHM:
                raise RuntimeError("snapshot page version mismatch")
            chunk = page.get("items")
            if not isinstance(chunk, list) or len(chunk) > 100:
                raise RuntimeError("invalid snapshot page")
            size += len(json.dumps(chunk, separators=(",", ":")).encode())
            if size > MAX_SNAPSHOT_BYTES:
                raise RuntimeError("snapshot exceeds 128 MiB")
            keys = [row.get("r2_key") if isinstance(row, dict) else None for row in chunk]
            if any(not isinstance(key, str) or not key or len(key) > 1024 for key in keys):
                raise RuntimeError("invalid snapshot image key")
            if keys and (keys[0] <= cursor or any(a >= b for a, b in zip(keys, keys[1:]))):
                raise RuntimeError("snapshot keys are not strictly ordered")
            items.extend(chunk)
            if len(items) > snapshot[count_key]:
                raise RuntimeError("snapshot exceeds declared count")
            if "next_cursor" not in page:
                raise RuntimeError("snapshot cursor missing")
            next_cursor = page["next_cursor"]
            if next_cursor is None:
                break
            if not keys or next_cursor != keys[-1]:
                raise RuntimeError("invalid snapshot pagination")
            cursor = next_cursor
        if len(items) != snapshot[count_key]:
            raise RuntimeError("snapshot count mismatch")
        data[kind] = items
    return data


def run_native(binary: str, data: dict, directory: Path) -> dict:
    source = directory / "input.json"
    output = directory / "result.json"
    errors = directory / "stderr.txt"
    source.write_text(json.dumps(data, separators=(",", ":")))
    with source.open("rb") as stdin, output.open("wb") as stdout, errors.open("wb") as stderr:
        result = subprocess.run([binary], stdin=stdin, stdout=stdout, stderr=stderr, timeout=1200)
    if result.returncode:
        with errors.open(errors="replace") as stderr:
            detail = stderr.read(400).strip()
        raise RuntimeError(f"native computation failed (exit {result.returncode}): {detail}")
    if output.stat().st_size > MAX_RESULT_BYTES:
        raise RuntimeError("native result exceeds 4 MiB")
    result = json.loads(output.read_text())
    if (
        not isinstance(result, dict)
        or result.get("generation") != data["generation"]
        or result.get("algorithm") != ALGORITHM
        or type(result.get("hash_count")) is not int
        or not 0 <= result["hash_count"] <= len(data["fingerprints"])
        or type(result.get("clip_count")) is not int
        or not 0 <= result["clip_count"] <= len(data["clip"])
        or not isinstance(result.get("response"), dict)
        or result["response"].get("ok") is not True
    ):
        raise RuntimeError("native result does not match snapshot")
    return result


def compute(token: str, endpoint: str, binary: str, state_dir: Path) -> None:
    snapshot = api(token, endpoint, "GET", "/api/catalog/similar/snapshot")
    if snapshot.get("algorithm") != ALGORITHM:
        raise RuntimeError("unsupported similarity algorithm")
    if snapshot.get("result_fresh") is True:
        print(f"generation={snapshot['generation']} current", flush=True)
        return
    snapshot = api(token, endpoint, "GET", "/api/catalog/similar/snapshot?counts=1")
    if snapshot.get("algorithm") != ALGORITHM:
        raise RuntimeError("unsupported similarity algorithm")
    for field in ("generation", "fingerprint_count", "clip_count"):
        minimum = 1 if field == "generation" else 0
        if type(snapshot.get(field)) is not int or snapshot[field] < minimum:
            raise RuntimeError(f"invalid snapshot {field}")
    data = export_input(token, endpoint, snapshot)
    print(
        f"generation={snapshot['generation']} fingerprints={len(data['fingerprints'])} clip={len(data['clip'])}",
        flush=True,
    )
    with tempfile.TemporaryDirectory(prefix="compute-", dir=state_dir) as directory:
        result = run_native(binary, data, Path(directory))
        api(token, endpoint, "POST", "/api/catalog/similar/result", result)
    print(f"generation={snapshot['generation']} published", flush=True)


def scan_and_compute(token: str, args) -> None:
    endpoint = args.endpoint.rstrip("/")
    snapshot = api(token, endpoint, "GET", "/api/catalog/similar/snapshot")
    generation = snapshot.get("generation")
    if type(generation) is not int or generation < 1 or snapshot.get("algorithm") != ALGORITHM:
        raise RuntimeError("invalid similarity version")
    marker = args.state_dir / "last-scan.json"
    try:
        last_scan = json.loads(marker.read_text())
    except (OSError, ValueError):
        last_scan = None
    checked = {"generation": generation, "algorithm": ALGORITHM}
    if args.clip_scanner and last_scan == checked and snapshot.get("result_fresh") is True:
        print(f"generation={generation} unchanged; no scan or export", flush=True)
        return
    scan_failed = False
    if args.clip_scanner:
        try:
            scan = subprocess.run([
                args.clip_python, args.clip_scanner, "--env", args.env,
                "--threads", str(args.clip_threads), "--limit", str(args.clip_limit), "--quiet",
            ], timeout=1200)
            scan_failed = scan.returncode != 0
        except subprocess.TimeoutExpired:
            scan_failed = True
            print("CLIP scan timed out; uploaded progress is retained", flush=True)
    # Publish completed scan batches even if another image failed; the next run retries missing keys.
    compute(token, endpoint, args.binary, args.state_dir)
    if scan_failed:
        raise RuntimeError("CLIP scan incomplete; missing images will be retried")
    if args.clip_scanner:
        # Record the pre-scan version: writes during scanning must trigger another check.
        temporary = marker.with_suffix(".tmp")
        temporary.write_text(json.dumps(checked))
        temporary.replace(marker)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", default="/opt/hanabi-container/hanabi.env")
    parser.add_argument("--endpoint", default=ENDPOINT)
    parser.add_argument("--binary", default=str(Path(__file__).resolve().with_name("vitrine-similar")))
    parser.add_argument("--state-dir", type=Path, default=Path.home() / ".cache/vitrine-similar")
    parser.add_argument("--clip-python")
    parser.add_argument("--clip-scanner")
    parser.add_argument("--clip-limit", type=int, default=0)
    parser.add_argument("--clip-threads", type=int, default=1)
    args = parser.parse_args()
    if bool(args.clip_python) != bool(args.clip_scanner) or args.clip_limit < 0 or args.clip_threads < 1:
        parser.error("provide both CLIP paths and a nonnegative limit")
    if urllib.parse.urlsplit(args.endpoint).scheme != "https":
        parser.error("--endpoint must use HTTPS")
    try:
        args.state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        with (args.state_dir / "run.lock").open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print("another computation is running", flush=True)
                return 0
            # A killed run cannot clean up; the lock proves its files are no longer in use.
            for interrupted in args.state_dir.glob("compute-*"):
                if interrupted.is_dir() and not interrupted.is_symlink():
                    shutil.rmtree(interrupted)
            cooldown = args.state_dir / "retry-after"
            try:
                retry_at = float(cooldown.read_text())
            except (OSError, ValueError):
                retry_at = 0
            if time.time() < retry_at:
                print("database quota cooldown; no requests", flush=True)
                return 0
            try:
                scan_and_compute(load_token(args.env), args)
            except QuotaExceeded as exc:
                cooldown.write_text(str(time.time() + exc.retry_after))
                print(f"database daily quota exhausted; retry in {exc.retry_after}s", flush=True)
                return 0
            cooldown.unlink(missing_ok=True)
    except StaleSnapshot:
        print("catalog changed; retry on next scheduled run", flush=True)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as exc:
        print(f"similar computation failed: {exc}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
