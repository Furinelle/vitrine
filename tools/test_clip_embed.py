#!/usr/bin/env python3
"""Deterministic scanner checks; no network, model, Torch, NumPy or PIL needed."""

import contextlib
import io
import math
import struct
import threading
import unittest
import urllib.error
from concurrent.futures import Future
from unittest.mock import patch

import clip_embed as clip


class ClipEmbedTests(unittest.TestCase):
    def scan(self, api, keys=None, **options):
        with patch.object(clip, "api", side_effect=api), \
             patch.object(clip, "load_model", return_value=(None,) * 4) as model, \
             patch.object(clip, "request_bytes", side_effect=keys or [b"image"] * 10), \
             patch.object(clip, "embed_image", return_value=[2.0] + [0.0] * 511), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            code = clip.scan("private-token", options.get("limit", 0), options.get("batch_size", 2), 1, True)
        return code, model

    def test_normalized_finite_512_float32_encoding(self):
        values = struct.unpack("<512f", bytes.fromhex(clip.pack_embedding([3.0, 4.0] + [0.0] * 510)))
        self.assertAlmostEqual(math.sqrt(sum(value * value for value in values)), 1.0)
        self.assertAlmostEqual(values[0], 0.6)
        for invalid in ([0.0] * 512, [math.nan] * 512, [math.inf] * 512, [1.0] * 511):
            with self.assertRaises(ValueError):
                clip.pack_embedding(invalid)

    def test_missing_empty_does_not_load_model(self):
        code, model = self.scan([{"ok": True, "missing": []}])
        self.assertEqual(code, 0)
        model.assert_not_called()

    def test_limit_batches_and_server_verification(self):
        calls = []
        def api(token, method, path, data=None):
            calls.append((method, path, data))
            if method == "PUT":
                return {"ok": True, "upserted": len(data["images"])}
            return {"ok": True, "missing": ["a", "b", "c", "d"] if len(calls) == 1 else ["d"]}
        code, model = self.scan(api, limit=3)
        self.assertEqual(code, 0)
        self.assertEqual([len(data["images"]) for method, _, data in calls if method == "PUT"], [2, 1])
        self.assertTrue(all("/api/catalog?" not in path for _, path, _ in calls))
        model.assert_called_once_with(1)

    def test_uploaded_but_still_missing_is_failure(self):
        code, _ = self.scan([{"ok": True, "missing": ["a"]}, {"ok": True, "upserted": 1},
                             {"ok": True, "missing": ["a"]}])
        self.assertEqual(code, 1)

    def test_failed_batch_stops_and_retains_prior_upload(self):
        code, _ = self.scan([{"ok": True, "missing": ["a", "b", "c", "d"]},
                             {"ok": True, "upserted": 2}, RuntimeError("request failed"),
                             {"ok": True, "missing": ["c", "d"]}])
        self.assertEqual(code, 1)

    def test_upload_count_mismatch_fails(self):
        code, _ = self.scan([{"ok": True, "missing": ["a"]}, {"ok": True, "upserted": 0},
                             {"ok": True, "missing": ["a"]}])
        self.assertEqual(code, 1)

    def test_removed_media_404_only_succeeds_if_removed_from_missing(self):
        for remaining, expected in (([], 0), (["a"], 1)):
            code, _ = self.scan([{"ok": True, "missing": ["a"]}, {"ok": True, "missing": remaining}],
                                keys=[clip.HTTPFailure(404)])
            self.assertEqual(code, expected)

    def test_corrupt_image_does_not_block_other_uploads(self):
        code, _ = self.scan([{"ok": True, "missing": ["a", "b"]}, {"ok": True, "upserted": 1},
                             {"ok": True, "missing": ["a"]}], keys=[RuntimeError("bad image"), b"image"])
        self.assertEqual(code, 1)

    def test_transient_http_and_timeout_retry_without_body(self):
        req = clip.urllib.request.Request("https://example.invalid")
        response = io.BytesIO(b"ok")
        error = urllib.error.HTTPError(req.full_url, 503, "error", {}, io.BytesIO(b"private-token"))
        with patch.object(clip.urllib.request, "urlopen", side_effect=[error, TimeoutError(), response]) as opened, \
             patch.object(clip.time, "sleep") as sleep:
            self.assertEqual(clip.request_bytes(req, 10, 60), b"ok")
            self.assertEqual(opened.call_count, 3)
            self.assertEqual(sleep.call_count, 2)
        error = urllib.error.HTTPError(req.full_url, 401, "error", {}, io.BytesIO(b"private-token"))
        with patch.object(clip.urllib.request, "urlopen", side_effect=error) as opened:
            with self.assertRaisesRegex(clip.HTTPFailure, "^HTTP 401$"):
                clip.request_bytes(req, 10, 60)
            self.assertEqual(opened.call_count, 1)

    def test_retries_are_bounded_and_size_is_limited(self):
        req = clip.urllib.request.Request("https://example.invalid")
        with patch.object(clip.urllib.request, "urlopen", side_effect=TimeoutError()) as opened, \
             patch.object(clip.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "after 3 attempts"):
                clip.request_bytes(req, 10, 60)
            self.assertEqual(opened.call_count, 3)
        with patch.object(clip.urllib.request, "urlopen", return_value=io.BytesIO(b"too long")):
            with self.assertRaisesRegex(RuntimeError, "response exceeds"):
                clip.request_bytes(req, 3, 60)

    def test_invalid_api_and_missing_lists_rejected(self):
        with patch.object(clip, "request_bytes", return_value=b'{"ok":false,"error":"secret"}'):
            with self.assertRaisesRegex(RuntimeError, "invalid API response"):
                clip.api("token", "GET", "/x")
        for keys in (None, "a", [None], [""]):
            with patch.object(clip, "api", return_value={"ok": True, "missing": keys}):
                with self.assertRaisesRegex(RuntimeError, "invalid missing"):
                    clip.missing_keys("token")

    def test_key_url_is_encoded(self):
        self.assertEqual(clip.media_url("pixiv/x y/a?#.jpg"), clip.ENDPOINT + "/media/pixiv/x%20y/a%3F%23.jpg")

    def test_prefetch_never_submits_more_than_four_and_keeps_order(self):
        submitted = []
        class Pool:
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def submit(self, fn, key):
                submitted.append(key)
                future = Future()
                future.set_result(key.encode())
                return future
        with patch.object(clip, "ThreadPoolExecutor", return_value=Pool()) as pool:
            seen = []
            with contextlib.closing(clip.prefetch_images(["a", "b", "c", "d", "e", "f"])) as downloads:
                for index, (key, future) in enumerate(downloads):
                    self.assertLessEqual(len(submitted), index + 4)
                    self.assertEqual(future.result(), key.encode())
                    seen.append(key)
            self.assertEqual(seen, ["a", "b", "c", "d", "e", "f"])
            pool.assert_called_once_with(max_workers=4)

    def test_downloads_overlap_but_exceptions_keep_input_order(self):
        started = threading.Barrier(5)
        release = threading.Event()
        def download(key):
            started.wait(timeout=5)
            release.wait(timeout=5)
            if key == "a":
                raise clip.HTTPFailure(404)
            return key.encode()
        with patch.object(clip, "download_image", side_effect=download), \
             contextlib.closing(clip.prefetch_images(["a", "b", "c", "d"])) as downloads:
            key, future = next(downloads)
            try:
                started.wait(timeout=5)  # All four workers entered before any can finish.
                self.assertEqual(key, "a")
            finally:
                release.set()
            with self.assertRaises(clip.HTTPFailure):
                future.result(timeout=5)
            for expected in ("b", "c", "d"):
                key, future = next(downloads)
                self.assertEqual((key, future.result(timeout=5)), (expected, expected.encode()))
            with self.assertRaises(StopIteration):
                next(downloads)


if __name__ == "__main__":
    unittest.main()
