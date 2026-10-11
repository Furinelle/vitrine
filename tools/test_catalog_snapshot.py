#!/usr/bin/env python3
"""Offline catalog CLI acceptance: disposable files and a loopback HTTP server."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import catalog_snapshot as tool

TOKEN = "synthetic-local-token"
SCRIPT = Path(__file__).with_name("catalog_snapshot.py")


def image(identity, **changes):
    return {"catalog_id": identity, "work_id": "pixiv:fixture", "source": "pixiv",
            "source_id": "fixture", "source_url": "https://example.invalid/work", "title": "Fixture",
            "page_index": 0, "r2_key": identity + ".jpg", "byte_size": 10,
            "content_type": "image/jpeg", "sha256": "fixture", "fingerprint_digest": None, **changes}


class Fixture:
    def __init__(self, pages, generations=(7, 7)):
        self.pages, self.generations, self.requests, self.generation_reads = pages, generations, [], 0

    def __enter__(self):
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                fixture.requests.append((self.path, self.headers.get("Authorization")))
                url = urlsplit(self.path)
                if url.path == "/api/catalog/similar/snapshot":
                    index = min(fixture.generation_reads, len(fixture.generations) - 1)
                    fixture.generation_reads += 1
                    value = {"ok": True, "generation": fixture.generations[index]}
                elif url.path == "/api/catalog":
                    query = parse_qs(url.query, keep_blank_values=True)
                    assert query["limit"] == ["100"] and query["fingerprints"] == ["1"]
                    value = fixture.pages[query["after"][0]]
                else:
                    raise AssertionError("Unexpected media or API request")
                if callable(value):
                    value = value()
                status = 200
                headers = {}
                if isinstance(value, tuple):
                    status, value, headers = value
                raw = value if isinstance(value, bytes) else json.dumps(value).encode()
                self.send_response(status)
                for key, header in headers.items():
                    self.send_header(key, header)
                self.send_header("Content-Length", str(len(raw)))
                self.end_headers()
                try:
                    self.wfile.write(raw)
                except BrokenPipeError:
                    pass

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = "http://127.0.0.1:" + str(self.server.server_port)
        self.client = tool.CatalogClient(self.origin, TOKEN)
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def page(rows, cursor=None):
    return {"ok": True, "images": rows, "next_cursor": cursor}


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / "catalog.jsonl"

    def test_cli_complete_stable_snapshot_owner_only_and_allowlisted(self):
        pages = {"": page([image("a", unexpected_sensitive_field="must-not-export")], "a"),
                 "a": page([image("b")])}
        with Fixture(pages) as fixture:
            env = {**os.environ, "HANABI_GALLERY_TOKEN": TOKEN}
            command = [sys.executable, str(SCRIPT), "snapshot", "--endpoint", fixture.origin, "--output", str(self.output)]
            first = subprocess.run(command, env=env, capture_output=True, text=True, check=True)
            contents = self.output.read_bytes()
            second = subprocess.run(command, env=env, capture_output=True, text=True, check=True)
            self.assertEqual(first.stdout, second.stdout)
            self.assertEqual(contents, self.output.read_bytes())
            self.assertEqual(stat.S_IMODE(self.output.stat().st_mode), 0o600)
            self.assertNotIn(b"must-not-export", contents)
            self.assertNotIn(TOKEN.encode(), contents)
            header, rows, complete = tool.read_snapshot(self.output)
            self.assertEqual(list(rows), ["a", "b"])
            self.assertEqual(complete["count"], 2)
            self.assertEqual(header["generation"], 7)
            self.assertTrue(all(auth == "Bearer " + TOKEN for _, auth in fixture.requests))
            self.assertEqual(len(fixture.requests), 8)

    def test_partial_permissions_and_failures_never_replace_complete_file(self):
        self.output.write_bytes(b"old complete snapshot")
        cases = [
            ({"": page([image("a")], "a"), "a": (500, TOKEN.encode(), {})}, (7, 7)),
            ({"": page([image("a")], "a"), "a": page([image("a")], "a")}, (7, 7)),
            ({"": page([image("b"), image("a")])}, (7, 7)),
            ({"": page([image("a")], "wrong")}, (7, 7)),
            ({"": {"ok": True, "images": []}}, (7, 7)),
            ({"": b"invalid-json"}, (7, 7)),
            ({"": page([])}, (7, 8)),
            ({"": page([] , "a")}, (7, 7)),
        ]
        for pages, generations in cases:
            with self.subTest(pages=pages), Fixture(pages, generations) as fixture:
                def inspect_page(path):
                    self.assertEqual(stat.S_IMODE(next(self.output.parent.glob("*.partial")).stat().st_mode), 0o600)
                    return original(path)
                original = fixture.client.get
                with patch.object(fixture.client, "get", side_effect=lambda path: inspect_page(path) if path.startswith("/api/catalog?") else original(path)):
                    with self.assertRaises((ValueError, RuntimeError)) as error:
                        tool.snapshot(fixture.client, self.output)
                self.assertNotIn(TOKEN, str(error.exception))
                self.assertEqual(self.output.read_bytes(), b"old complete snapshot")
                self.assertEqual(list(self.output.parent.glob("*.partial")), [])

    def test_killed_process_keeps_old_file_and_private_partial(self):
        self.output.write_bytes(b"old complete snapshot")
        entered, release = threading.Event(), threading.Event()
        def paused_page():
            entered.set()
            release.wait(10)
            return page([image("b")])
        with Fixture({"": page([image("a")], "a"), "a": paused_page}) as fixture:
            process = subprocess.Popen([sys.executable, str(SCRIPT), "snapshot", "--endpoint", fixture.origin,
                                        "--output", str(self.output)],
                                       env={**os.environ, "HANABI_GALLERY_TOKEN": TOKEN},
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                self.assertTrue(entered.wait(5))
                partial = next(self.output.parent.glob("*.partial"))
                self.assertEqual(stat.S_IMODE(partial.stat().st_mode), 0o600)
                process.kill()
                process.communicate(timeout=5)
                self.assertEqual(self.output.read_bytes(), b"old complete snapshot")
                self.assertTrue(partial.exists())
                with self.assertRaises(ValueError):
                    tool.read_snapshot(partial)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)
                release.set()

    def test_redirect_cannot_forward_token_to_another_origin(self):
        with Fixture({}) as destination:
            with Fixture({"": (302, TOKEN.encode(), {"Location": destination.origin + "/steal"})}) as source:
                with self.assertRaisesRegex(RuntimeError, "HTTP 302"):
                    tool.snapshot(source.client, self.output)
            self.assertEqual(destination.requests, [])
            self.assertFalse(self.output.exists())

    def test_response_and_snapshot_size_caps(self):
        with Fixture({"": page([image("a")])}) as fixture:
            with patch.object(tool, "MAX_RESPONSE_BYTES", 200):
                with self.assertRaisesRegex(ValueError, "response exceeds"):
                    tool.snapshot(fixture.client, self.output)
        with Fixture({"": page([image("a")])}) as fixture:
            with patch.object(tool, "MAX_SNAPSHOT_BYTES", 1):
                with self.assertRaisesRegex(ValueError, "snapshot exceeds"):
                    tool.snapshot(fixture.client, self.output)
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.output.parent.glob("*.partial")), [])

    def test_empty_complete_catalog(self):
        with Fixture({"": page([])}) as fixture:
            summary = tool.snapshot(fixture.client, self.output)
        self.assertEqual(summary["count"], 0)
        self.assertEqual(summary["sha256"], hashlib.sha256(b"").hexdigest())
        self.assertEqual(tool.read_snapshot(self.output)[1], {})

    def test_offline_diff_and_incomplete_or_tampered_files(self):
        other = self.output.with_name("after.jsonl")
        with Fixture({"": page([image("a"), image("b")])}) as fixture:
            tool.snapshot(fixture.client, self.output)
            fixture.pages = {"": page([image("b", byte_size=20, title="Changed"), image("c")])}
            tool.snapshot(fixture.client, other)
        result = subprocess.run([sys.executable, str(SCRIPT), "diff", str(self.output), str(other)],
                                env={key: value for key, value in os.environ.items() if key != "HANABI_GALLERY_TOKEN"},
                                capture_output=True, text=True, check=True)
        diff = json.loads(result.stdout)
        self.assertEqual(diff["counts"], {"added": 1, "removed": 1, "changed": 1})
        self.assertEqual(diff["added"][0]["catalog_id"], "c")
        self.assertEqual(diff["removed"][0]["catalog_id"], "a")
        self.assertEqual(diff["changed"][0]["changes"]["byte_size"], {"before": 10, "after": 20})
        self.assertEqual(diff["changed"][0]["changes"]["title"]["after"], "Changed")
        valid = other.read_bytes()
        for damaged in [b"", b"\n".join(valid.splitlines()[:-1]), valid.replace(b'"Changed"', b'"Tampered"'), valid + b'{}\n']:
            other.write_bytes(damaged)
            with self.assertRaises(ValueError):
                tool.compare(self.output, other)

    def test_existing_env_file_mode_and_secret_safe_http_error(self):
        env_file = self.output.with_suffix(".env")
        env_file.write_text("UNRELATED=ignored\nHANABI_GALLERY_TOKEN=" + TOKEN + "\n")
        with Fixture({"": (403, ("headers Authorization: Bearer " + TOKEN).encode(), {})}) as fixture:
            result = subprocess.run([sys.executable, str(SCRIPT), "snapshot", "--endpoint", fixture.origin,
                                     "--env", str(env_file), "--output", str(self.output)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(TOKEN, result.stdout + result.stderr)
        self.assertIn("HTTP 403", result.stderr)

    def test_origin_validation(self):
        for endpoint in ["http://example.invalid", "https://u:secret@example.invalid", "https://example.invalid/path", "https://example.invalid?token=secret", "https://example.invalid:secret", None]:
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                tool.endpoint_origin(endpoint)


if __name__ == "__main__":
    unittest.main()
