#!/usr/bin/env python3
"""Offline checks for cold_store.py. The serve round trip needs Linux xattrs."""

import json
import os
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cold_store  # noqa: E402

class PickTest(unittest.TestCase):
    def test_earliest_images_down_to_low_water_and_leaves_other_objects(self):
        objects = [
            {"key": "x/new.jpg", "size": 40},
            {"key": "x/old.jpg", "size": 30},
            {"key": "bench/a.webp", "size": 30},
        ]
        images = [{"r2_key": "x/gone.jpg"}, {"r2_key": "x/old.jpg"}, {"r2_key": "x/new.jpg"}]
        self.assertEqual(cold_store.pick(objects, iter(images), 100, 50), [])
        picked = cold_store.pick(objects, iter(images), 90, 50)
        self.assertEqual([o["key"] for o in picked], ["x/old.jpg", "x/new.jpg"])
        picked = cold_store.pick(objects, iter(images), 90, 75)
        self.assertEqual([o["key"] for o in picked], ["x/old.jpg"])


class VerifyTest(unittest.TestCase):
    def test_checks_size_sha_and_single_part_md5(self):
        meta = {"sha256": "aa", "etag": '"' + "b" * 32 + '"'}
        self.assertIsNone(cold_store.verify(meta, 5, "aa", "b" * 32, 5))
        self.assertIn("size", cold_store.verify(meta, 4, "aa", "b" * 32, 5))
        self.assertIn("sha256", cold_store.verify(meta, 5, "cc", "b" * 32, 5))
        self.assertIn("md5", cold_store.verify(meta, 5, "aa", "c" * 32, 5))
        multipart = {"sha256": "", "etag": '"abc-2"'}
        self.assertIsNone(cold_store.verify(multipart, 5, "x", "y", 5))


class PathTest(unittest.TestCase):
    def test_rejects_escapes(self):
        root = Path("/srv")
        self.assertEqual(cold_store.object_path(root, "pixiv/1/00.jpg"), Path("/srv/pixiv/1/00.jpg"))
        for key in ("", "../x", "a/../b", "/abs", "a//b", "a/\n"):
            self.assertIsNone(cold_store.object_path(root, key), key)


@unittest.skipUnless(hasattr(os, "setxattr"), "needs Linux xattrs")
class ServeTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        obj = self.root / "pixiv" / "1" / "00.jpg"
        obj.parent.mkdir(parents=True)
        obj.write_bytes(b"\xff\xd8bytes")
        cold_store.write_meta(obj, {"content_type": "image/jpeg", "etag": '"e"', "sha256": "s"})
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), cold_store.make_handler(self.root, "tok"))
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.server.server_port}/o/"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.dir.cleanup()

    def call(self, method, key, token="tok", body=None):
        request = urllib.request.Request(self.base + key, method=method, data=body)
        request.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(request) as response:
                return response.status, response.headers, response.read()
        except urllib.error.HTTPError as exc:
            return exc.code, exc.headers, b""

    def test_get_head_copy_and_auth(self):
        status, headers, body = self.call("GET", "pixiv/1/00.jpg")
        self.assertEqual((status, body), (200, b"\xff\xd8bytes"))
        self.assertEqual(
            (headers["Content-Type"], headers["ETag"], headers["X-Sha256"]),
            ("image/jpeg", '"e"', "s"),
        )
        status, headers, body = self.call("HEAD", "pixiv/1/00.jpg")
        self.assertEqual((status, headers["Content-Length"], body), (200, "7", b""))
        self.assertEqual(self.call("GET", "pixiv/1/00.jpg", token="bad")[0], 401)
        self.assertEqual(self.call("GET", "pixiv/2/00.jpg")[0], 404)
        self.assertEqual(self.call("GET", "..%2Fetc")[0], 400)

        copy = json.dumps({"from": "pixiv/1/00.jpg"}).encode()
        self.assertEqual(self.call("PUT", "review-trash/d/pixiv/1/00.jpg", body=copy)[0], 200)
        status, headers, body = self.call("GET", "review-trash/d/pixiv/1/00.jpg")
        self.assertEqual((status, body, headers["X-Sha256"]), (200, b"\xff\xd8bytes", "s"))
        missing = json.dumps({"from": "pixiv/9/00.jpg"}).encode()
        self.assertEqual(self.call("PUT", "x/00.jpg", body=missing)[0], 404)


if __name__ == "__main__":
    unittest.main()
