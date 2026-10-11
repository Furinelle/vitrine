#!/usr/bin/env python3
"""Save a read-only catalog JSONL snapshot or compare two complete snapshots offline."""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from similar_compute import ENDPOINT, MAX_RESPONSE_BYTES, MAX_SNAPSHOT_BYTES, load_token

FORMAT = "vitrine-catalog-v1"
FIELDS = ("catalog_id", "work_id", "source", "source_id", "source_url", "title",
          "page_index", "r2_key", "byte_size", "content_type", "sha256", "fingerprint_digest")


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode("utf-8")


def image_row(value):
    if not isinstance(value, dict) or any(key not in value for key in FIELDS):
        raise ValueError("catalog image fields missing")
    row = {key: value[key] for key in FIELDS}
    for key, item in row.items():
        if key in ("page_index", "byte_size"):
            valid = type(item) is int and item >= 0
        elif key == "fingerprint_digest":
            valid = item is None or (isinstance(item, str) and len(item) == 64 and all(c in "0123456789abcdef" for c in item))
        else:
            valid = isinstance(item, str)
        if not valid:
            raise ValueError("invalid catalog image field: " + key)
    if not row["catalog_id"] or not row["r2_key"]:
        raise ValueError("empty catalog image identity")
    return row


def endpoint_origin(endpoint):
    if not isinstance(endpoint, str):
        raise ValueError("invalid endpoint origin")
    value = urllib.parse.urlsplit(endpoint)
    if (value.scheme not in ("https", "http") or not value.hostname
            or value.username is not None or value.password is not None
            or value.path not in ("", "/") or value.query or value.fragment):
        raise ValueError("endpoint must be an HTTPS origin without credentials, path or query")
    if value.scheme == "http" and value.hostname not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("HTTP is allowed only for loopback fixtures")
    try:
        _ = value.port
    except ValueError:
        raise ValueError("invalid endpoint port") from None
    return urllib.parse.urlunsplit((value.scheme, value.netloc, "", "", ""))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


class CatalogClient:
    def __init__(self, endpoint, token):
        self.endpoint = endpoint_origin(endpoint)
        if not token or any(not 33 <= ord(c) <= 126 for c in token):
            raise ValueError("missing or invalid HANABI_GALLERY_TOKEN")
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect())

    def get(self, path):
        # All callers use fixed same-origin GET paths; never follow redirects or
        # print an HTTP response body/request that could contain credentials.
        request = urllib.request.Request(self.endpoint + path, headers={
            "Authorization": "Bearer " + self.token, "User-Agent": "vitrine-catalog-snapshot/1",
        }, method="GET")
        try:
            with self.opener.open(request, timeout=30) as response:
                raw = response.read(MAX_RESPONSE_BYTES + 1)
        except urllib.error.HTTPError as error:
            error.close()
            raise RuntimeError("catalog API HTTP " + str(error.code)) from None
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.HTTPException):
            raise RuntimeError("catalog API request failed") from None
        if len(raw) > MAX_RESPONSE_BYTES:
            raise ValueError("catalog API response exceeds 8 MiB")
        try:
            value = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            raise ValueError("invalid catalog API JSON") from None
        if not isinstance(value, dict) or value.get("ok") is not True:
            raise ValueError("invalid catalog API response")
        return value

    def generation(self):
        value = self.get("/api/catalog/similar/snapshot").get("generation")
        if type(value) is not int or value < 1:
            raise ValueError("invalid catalog generation")
        return value


def snapshot(client, output):
    generation = client.generation()
    output = Path(output)
    descriptor, name = tempfile.mkstemp(prefix="." + output.name + ".", suffix=".partial", dir=output.parent)
    temporary = Path(name)
    count, size, cursor = 0, 0, ""
    digest = hashlib.sha256()
    try:
        with os.fdopen(descriptor, "wb") as file:
            file.write(encoded({"type": "header", "format": FORMAT, "origin": client.endpoint, "generation": generation}))
            while True:
                query = urllib.parse.urlencode({"after": cursor, "limit": 100, "fingerprints": 1})
                page = client.get("/api/catalog?" + query)
                rows = page.get("images")
                if not isinstance(rows, list) or len(rows) > 100 or "next_cursor" not in page:
                    raise ValueError("invalid catalog page")
                for value in rows:
                    row = image_row(value)
                    identity = row["catalog_id"]
                    if identity <= cursor:
                        raise ValueError("catalog IDs repeat or are not strictly ordered")
                    cursor = identity
                    raw = encoded({"type": "image", **row})
                    size += len(raw)
                    if size > MAX_SNAPSHOT_BYTES:
                        raise ValueError("snapshot exceeds 128 MiB")
                    file.write(raw)
                    digest.update(raw)
                    count += 1
                next_cursor = page["next_cursor"]
                if next_cursor is None:
                    break
                if not rows or next_cursor != cursor:
                    raise ValueError("invalid or looping catalog cursor")
            if client.generation() != generation:
                raise ValueError("catalog changed during export; retry")
            summary = {"type": "complete", "count": count, "sha256": digest.hexdigest()}
            file.write(encoded(summary))
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, output)
        return {"count": count, "sha256": digest.hexdigest(), "generation": generation}
    finally:
        temporary.unlink(missing_ok=True)


def read_snapshot(path):
    if Path(path).stat().st_size > MAX_SNAPSHOT_BYTES + MAX_RESPONSE_BYTES:
        raise ValueError("snapshot file too large")
    rows, header, complete = {}, None, None
    digest = hashlib.sha256()
    previous = ""
    with Path(path).open("rb") as file:
        for raw in file:
            try:
                value = json.loads(raw)
            except (ValueError, UnicodeDecodeError):
                raise ValueError("invalid snapshot JSONL") from None
            if not isinstance(value, dict) or complete is not None:
                raise ValueError("invalid snapshot record order")
            if header is None:
                if value.get("type") != "header" or value.get("format") != FORMAT:
                    raise ValueError("unsupported snapshot format")
                if type(value.get("generation")) is not int or value["generation"] < 1:
                    raise ValueError("invalid snapshot generation")
                endpoint_origin(value.get("origin", ""))
                header = value
            elif value.get("type") == "image":
                row = image_row(value)
                if row["catalog_id"] <= previous:
                    raise ValueError("snapshot IDs repeat or are not strictly ordered")
                previous = row["catalog_id"]
                rows[previous] = row
                digest.update(raw)
            elif value.get("type") == "complete":
                complete = value
            else:
                raise ValueError("invalid snapshot record type")
    if (complete is None or type(complete.get("count")) is not int
            or complete["count"] != len(rows) or complete.get("sha256") != digest.hexdigest()):
        raise ValueError("incomplete snapshot or digest/count mismatch")
    return header, rows, complete


def compare(before, after):
    old_header, old, old_summary = read_snapshot(before)
    new_header, new, new_summary = read_snapshot(after)
    if old_header["origin"] != new_header["origin"]:
        raise ValueError("cannot compare snapshots from different origins")
    added = [new[key] for key in sorted(new.keys() - old.keys())]
    removed = [old[key] for key in sorted(old.keys() - new.keys())]
    changed = []
    for key in sorted(old.keys() & new.keys()):
        fields = {field: {"before": old[key][field], "after": new[key][field]}
                  for field in FIELDS if old[key][field] != new[key][field]}
        if fields:
            changed.append({"catalog_id": key, "changes": fields})
    return {"before": {**old_summary, "generation": old_header["generation"]},
            "after": {**new_summary, "generation": new_header["generation"]},
            "counts": {"added": len(added), "removed": len(removed), "changed": len(changed)},
            "added": added, "removed": removed, "changed": changed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    save = actions.add_parser("snapshot", help="GET metadata only; atomically replace output after complete validation")
    save.add_argument("--endpoint", default=ENDPOINT)
    save.add_argument("--env", help="existing env file containing HANABI_GALLERY_TOKEN; otherwise use the environment")
    save.add_argument("--output", required=True, type=Path)
    diff = actions.add_parser("diff", help="compare complete snapshots without credentials or network")
    diff.add_argument("before", type=Path)
    diff.add_argument("after", type=Path)
    args = parser.parse_args()
    try:
        if args.action == "snapshot":
            token = load_token(args.env) if args.env else os.environ.get("HANABI_GALLERY_TOKEN", "")
            result = snapshot(CatalogClient(args.endpoint, token), args.output)
        else:
            result = compare(args.before, args.after)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True))
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print("catalog snapshot: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
