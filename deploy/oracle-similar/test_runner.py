"""Local runner checks; no credentials or production requests."""

import importlib.util
import fcntl
import json
import io
import urllib.error
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("similar_compute", ROOT / "tools/similar_compute.py")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def snapshot(**overrides):
    return {
        "ok": True,
        "generation": 7,
        "algorithm": runner.ALGORITHM,
        "fingerprint_count": 2,
        "clip_count": 1,
        "result_fresh": False,
        **overrides,
    }


def page(items, next_cursor=None, **overrides):
    return {
        "ok": True,
        "generation": 7,
        "algorithm": runner.ALGORITHM,
        "items": items,
        "next_cursor": next_cursor,
        **overrides,
    }


class RunnerTests(unittest.TestCase):
    def test_fresh_result_only_polls_status(self):
        with patch.object(runner, "api", return_value=snapshot(result_fresh=True)) as api:
            with patch.object(runner, "run_native") as native:
                runner.compute("test", "https://invalid.example", "/unused", Path("/unused"))
        self.assertEqual(api.call_count, 1)
        native.assert_not_called()

    def test_complete_snapshot_native_publish_and_cleanup(self):
        # A real disposable process verifies the stdin/stdout contract.
        with tempfile.TemporaryDirectory() as temporary:
            state = Path(temporary)
            binary = state / "native"
            binary.write_text(
                "#!/usr/bin/env python3\n"
                "import json,sys\n"
                "d=json.load(sys.stdin)\n"
                "assert [r['r2_key'] for r in d['fingerprints']]==['a','b']\n"
                "assert [r['r2_key'] for r in d['clip']]==['a']\n"
                "json.dump({'generation':d['generation'],'algorithm':d['algorithm'],"
                "'hash_count':len(d['fingerprints']),'clip_count':len(d['clip']),"
                "'response':{'ok':True,'groups':[],'auto_pairs':[]}},sys.stdout)\n"
            )
            binary.chmod(0o700)
            replies = [snapshot(), snapshot(), page([{"r2_key": "a"}], "a"), page([{"r2_key": "b"}]),
                       page([{"r2_key": "a", "embedding": "00"}]), {"ok": True}]
            with patch.object(runner, "api", side_effect=replies) as api:
                runner.compute("test", "https://invalid.example", str(binary), state)
            self.assertIn("after=a", api.call_args_list[3].args[3])
            self.assertIn("kind=clip", api.call_args_list[4].args[3])
            self.assertEqual(api.call_args.args[2:4], ("POST", "/api/catalog/similar/result"))
            self.assertEqual(api.call_args.args[4]["generation"], 7)
            self.assertEqual(list(state.iterdir()), [binary])

    def test_changed_generation_never_computes_or_publishes(self):
        replies = [snapshot(), snapshot(), page([{"r2_key": "a"}], "a"), runner.StaleSnapshot()]
        with patch.object(runner, "api", side_effect=replies) as api:
            with patch.object(runner, "run_native") as native:
                with self.assertRaises(runner.StaleSnapshot):
                    runner.compute("test", "https://invalid.example", "/unused", Path("/unused"))
        native.assert_not_called()
        self.assertTrue(all(call.args[2] == "GET" for call in api.call_args_list))

    def test_rejects_count_version_and_pagination_errors(self):
        for bad_page in (page([], 1), page([{}], 1, generation=8), page([{}]), page([{}], 0)):
            with self.subTest(page=bad_page):
                with patch.object(runner, "api", return_value=bad_page):
                    with self.assertRaises(RuntimeError):
                        runner.export_input("test", "https://invalid.example", snapshot())

    def test_native_failure_never_publishes(self):
        replies = [snapshot(fingerprint_count=0, clip_count=0), snapshot(fingerprint_count=0, clip_count=0), page([]), page([])]
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(runner, "api", side_effect=replies) as api:
                with patch.object(runner, "run_native", side_effect=RuntimeError("failed")):
                    with self.assertRaises(RuntimeError):
                        runner.compute("test", "https://invalid.example", "/unused", Path(temporary))
            self.assertTrue(all(call.args[2] == "GET" for call in api.call_args_list))
            self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_native_counts_can_exclude_malformed_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "native"
            result = {"generation": 7, "algorithm": runner.ALGORITHM,
                      "hash_count": 0, "clip_count": 0, "response": {"ok": True}}
            binary.write_text("#!/usr/bin/env python3\nprint(" + repr(json.dumps(result)) + ")\n")
            binary.chmod(0o700)
            data = {"generation": 7, "algorithm": runner.ALGORITHM,
                    "fingerprints": [{}], "clip": [{}]}
            self.assertEqual(runner.run_native(str(binary), data, Path(temporary)), result)

    def test_lock_prevents_second_process_and_token_read(self):
        with tempfile.TemporaryDirectory() as temporary:
            with (Path(temporary) / "run.lock").open("a") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                result = subprocess.run(
                    [sys.executable, str(ROOT / "tools/similar_compute.py"),
                     "--state-dir", temporary, "--env", str(Path(temporary) / "absent.env")],
                    capture_output=True, text=True, timeout=10,
                )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("another computation is running", result.stdout)

    def test_stale_publication_cleans_files_and_keeps_old_result(self):
        replies = [snapshot(fingerprint_count=0, clip_count=0), snapshot(fingerprint_count=0, clip_count=0), page([]), page([]), runner.StaleSnapshot()]
        result = {"generation": 7, "algorithm": runner.ALGORITHM,
                  "hash_count": 0, "clip_count": 0, "response": {"ok": True}}
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(runner, "api", side_effect=replies) as api:
                with patch.object(runner, "run_native", return_value=result):
                    with self.assertRaises(runner.StaleSnapshot):
                        runner.compute("test", "https://invalid.example", "/unused", Path(temporary))
            self.assertEqual(api.call_args.args[2], "POST")
            self.assertEqual(list(Path(temporary).iterdir()), [])


class PipelineTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.state_dir = Path(self.folder.name)
        api = patch.object(runner, "api", return_value=snapshot())
        self.api = api.start()
        self.addCleanup(api.stop)

    def args(self, enabled=True):
        return SimpleNamespace(clip_scanner="/scan.py" if enabled else None, clip_python="/python",
                               clip_limit=0, clip_threads=4, env="/existing.env", endpoint="https://invalid.example",
                               binary="/native", state_dir=self.state_dir)

    def test_scans_before_grouping(self):
        events = []
        with patch.object(runner.subprocess, "run", side_effect=lambda *a, **k: (events.append("scan") or SimpleNamespace(returncode=0))):
            with patch.object(runner, "compute", side_effect=lambda *a: events.append("group")):
                runner.scan_and_compute("test", self.args())
        self.assertEqual(events, ["scan", "group"])

    def test_partial_scan_still_publishes_but_reports_failure(self):
        for result in (SimpleNamespace(returncode=1), subprocess.TimeoutExpired("scan", 1200)):
            with self.subTest(result=result):
                with patch.object(runner.subprocess, "run", side_effect=result if isinstance(result, Exception) else None, return_value=result):
                    with patch.object(runner, "compute") as compute:
                        with self.assertRaisesRegex(RuntimeError, "CLIP scan incomplete"):
                            runner.scan_and_compute("test", self.args())
                        compute.assert_called_once()

    def test_existing_compute_only_mode(self):
        with patch.object(runner.subprocess, "run") as scan, patch.object(runner, "compute") as compute:
            runner.scan_and_compute("test", self.args(False))
        scan.assert_not_called()
        compute.assert_called_once()


    def test_unchanged_catalog_skips_scanner_counts_and_compute(self):
        self.api.return_value = snapshot(result_fresh=True)
        (self.state_dir / "last-scan.json").write_text(json.dumps({"generation": 7, "algorithm": runner.ALGORITHM}))
        with patch.object(runner.subprocess, "run") as scan, patch.object(runner, "compute") as compute:
            runner.scan_and_compute("test", self.args())
        self.api.assert_called_once_with("test", "https://invalid.example", "GET", "/api/catalog/similar/snapshot")
        scan.assert_not_called()
        compute.assert_not_called()

    def test_scan_records_pre_scan_generation_not_later_writes(self):
        with patch.object(runner.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
            with patch.object(runner, "compute", side_effect=lambda *a: setattr(self.api, "return_value", snapshot(generation=8))):
                runner.scan_and_compute("test", self.args())
        self.assertEqual(json.loads((self.state_dir / "last-scan.json").read_text())["generation"], 7)
        with patch.object(runner.subprocess, "run", return_value=SimpleNamespace(returncode=0)) as scan:
            with patch.object(runner, "compute"):
                runner.scan_and_compute("test", self.args())
        scan.assert_called_once()

    def test_partial_scan_does_not_mark_generation_complete(self):
        with patch.object(runner.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
            with patch.object(runner, "compute"):
                with self.assertRaises(RuntimeError):
                    runner.scan_and_compute("test", self.args())
        self.assertFalse((self.state_dir / "last-scan.json").exists())


class QuotaTests(unittest.TestCase):
    def test_quota_response_is_not_retried(self):
        error = urllib.error.HTTPError("https://invalid.example", 503, "unavailable", {"Retry-After": "1800"},
                                      io.BytesIO(b'{"ok":false,"code":"database_daily_limit"}'))
        with patch.object(runner.urllib.request, "urlopen", side_effect=error) as request:
            with patch.object(runner.time, "sleep") as sleep:
                with self.assertRaises(runner.QuotaExceeded) as raised:
                    runner.api("test", "https://invalid.example", "GET", "/api/catalog/similar/snapshot")
        self.assertEqual(raised.exception.retry_after, 1800)
        self.assertEqual(request.call_count, 1)
        sleep.assert_not_called()

    def test_cooldown_survives_restart_and_skips_token_and_network(self):
        with tempfile.TemporaryDirectory() as folder:
            args = ["runner", "--state-dir", folder, "--env", "/unused"]
            with patch.object(sys, "argv", args), patch.object(runner, "load_token", return_value="test"):
                with patch.object(runner, "scan_and_compute", side_effect=runner.QuotaExceeded(300)):
                    self.assertEqual(runner.main(), 0)
            with patch.object(sys, "argv", args), patch.object(runner, "load_token") as token:
                with patch.object(runner, "scan_and_compute") as pipeline:
                    self.assertEqual(runner.main(), 0)
            token.assert_not_called()
            pipeline.assert_not_called()

    def test_timer_runs_daily_after_quota_reset(self):
        timer = (ROOT / "deploy/oracle-similar/vitrine-similar.timer").read_text()
        self.assertIn("OnCalendar=*-*-* 00:15:00 UTC", timer)
        self.assertNotIn("OnUnitInactiveSec", timer)
        self.assertNotIn("OnBootSec", timer)


if __name__ == "__main__":
    unittest.main()
