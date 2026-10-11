"""Admin deletion regressions against a fresh disposable local Wrangler instance.

Use the same VITRINE_TEST_BASE / VITRINE_TEST_STATE and test token as
test_image_review.py. No Telegram request is made: new deletions have no
publication, and mapped deletions are prepared through the catalog API first.
"""
from concurrent.futures import ThreadPoolExecutor
import json
import os
import re
import sqlite3
from pathlib import Path
import struct
import subprocess
import threading
import urllib.parse
import uuid

import test_image_review as local


def check_noop_upserts():
    root = Path(__file__).resolve().parents[1]
    source = (root / "src/similar.rs").read_text()
    db = sqlite3.connect(":memory:")
    for migration in sorted((root / "migrations").glob("*.sql")):
        db.executescript(migration.read_text())
    for work in ("first", "second"):
        db.execute("INSERT INTO works(id,source,source_id,created_at) VALUES (?, 'pixiv', ?, '')", (work, work))
    db.execute("INSERT INTO similar_cache(id,clip_count,hash_count,payload,computed_at) VALUES (1,0,0,'retained','')")
    fixtures = {
        "image_fingerprints": {
            "r2_key": "pixiv/first/0.png", "work_id": "first", "solid_color": 0,
            "content_sha256": "a", "strict_key": "a", "average_hash": "0000000000000000",
            "difference_hash": "0000000000000000", "color_key": "color", "detail_key": "detail",
            "width": 200, "height": 200, "bytes": 123, "format": "png", "regions_json": "[]", "updated_at": "first",
        },
        "image_clip": {"r2_key": "pixiv/first/0.png", "dim": 512, "embedding": "00", "updated_at": "first"},
    }
    for table, row in fixtures.items():
        sql = re.search(r'r#"\s*(INSERT INTO ' + table + r'\s*\(.*?)(?:"#)', source, re.S)[1]
        columns = [name.strip() for name in re.search(r'INSERT INTO \w+\s*\((.*?)\)', sql, re.S)[1].split(",")]

        def put(values):
            db.execute(sql, [values[column] for column in columns])

        def generation():
            return db.execute("SELECT generation FROM similar_generation WHERE id=1").fetchone()[0]

        put(row)
        old_generation, old_changes = generation(), db.total_changes
        put({**row, "updated_at": "later"})
        assert db.total_changes == old_changes, f"{table} no-op wrote data"
        assert generation() == old_generation, f"{table} no-op changed generation"
        assert db.execute(f"SELECT updated_at FROM {table}").fetchone()[0] == "first"
        for field in columns:
            if field in ("r2_key", "updated_at"):
                continue
            before = generation()
            value = "second" if field == "work_id" else (
                "[{}]" if field == "regions_json" else row[field] + 1 if isinstance(row[field], int) else row[field] + "changed")
            put({**row, field: value})
            assert generation() == before + 1, f"{table}.{field} change failed to invalidate generation"
            put(row)
        assert db.execute("SELECT payload FROM similar_cache WHERE id=1").fetchone()[0] == "retained"
    print("PASS: actual UPSERT SQL makes content-identical writes zero-change; every fingerprint/CLIP field invalidates generation")


def main():
    url = urllib.parse.urlsplit(local.BASE)
    assert url.hostname in ("127.0.0.1", "localhost", "::1"), "Local test endpoint required"
    check_noop_upserts()
    origin = local.BASE.rstrip("/")
    _, _, headers = local.fetch("/api/admin/login", {"token": "local-image-review-test"},
                                auth=False, headers={"Origin": origin})
    cookie = headers["Set-Cookie"].split(";", 1)[0]

    binary = Path(os.environ.get("VITRINE_SIMILAR_BIN", Path(__file__).resolve().parents[1] / "target/debug/vitrine-similar"))

    def snapshot(counts=False):
        return local.request("/api/catalog/similar/snapshot" + ("?counts=1" if counts else ""))

    def status(expected_cache=None):
        _, body, headers = local.fetch("/api/admin/fingerprints/status", auth=False, headers={"Cookie": cookie})
        if expected_cache:
            assert headers["X-Similar-Stats-Cache"] == expected_cache, headers
        assert headers["Cache-Control"] == "no-store"
        return json.loads(body)

    def ensure_fresh():
        current = snapshot()
        if current["result_fresh"]:
            return None
        current = snapshot(counts=True)
        data = {"generation": current["generation"], "algorithm": current["algorithm"]}
        for kind, count in (("fingerprints", "fingerprint_count"), ("clip", "clip_count")):
            data[kind] = []
            after = ""
            while True:
                query = urllib.parse.urlencode({"generation": current["generation"], "kind": kind,
                                                "after": after, "limit": 1})
                page = local.request("/api/catalog/similar/input?" + query)
                assert page["generation"] == current["generation"]
                assert page["algorithm"] == current["algorithm"]
                assert len(page["items"]) <= 1
                assert all(value["r2_key"] > after for value in page["items"])
                data[kind].extend(page["items"])
                if page["next_cursor"] is None:
                    break
                assert page["items"] and page["next_cursor"] == page["items"][-1]["r2_key"]
                after = page["next_cursor"]
            assert len(data[kind]) == current[count]
            keys = [value["r2_key"] for value in data[kind]]
            assert keys == sorted(set(keys)), "Cursor export skipped or repeated an input"
        process = subprocess.run([str(binary)], input=json.dumps(data), capture_output=True,
                                 text=True, timeout=30, check=True)
        result = json.loads(process.stdout)
        assert result["generation"] == current["generation"]
        assert result["algorithm"] == current["algorithm"]
        assert local.request("/api/catalog/similar/result", result)["generation"] == current["generation"]
        assert snapshot()["result_fresh"]
        return result

    def admin_get(refresh=True):
        if refresh:
            ensure_fresh()
        return local.request("/api/admin/similar", auth=False, headers={"Cookie": cookie})

    def delete(items, expected=200, refresh=True):
        if refresh:
            ensure_fresh()
        return local.request("/api/admin/similar/delete", {"items": items}, expected,
                             auth=False, headers={"Origin": origin, "Cookie": cookie})

    for path, payload in (("/api/catalog/similar/snapshot", None),
                          ("/api/catalog/similar/input?generation=1&kind=fingerprints", None),
                          ("/api/catalog/similar/result", {})):
        local.request(path, payload, expected=401, auth=False)

    light = snapshot()
    assert admin_get(refresh=False)["pending"], "No cached scan must remain pending"
    assert "fingerprint_count" not in light and "clip_count" not in light
    assert local.request("/api/catalog/similar/snapshot?counts=0") == light
    coverage = status("MISS")
    assert coverage["hashed"] == 0 and coverage["clipped"] == 0, "Fresh isolated D1 state required"
    assert status("HIT") == coverage
    _, counted_body, counted_headers = local.fetch("/api/catalog/similar/snapshot?counts=1")
    counted = json.loads(counted_body)
    assert counted_headers["X-Similar-Stats-Cache"] == "HIT"
    assert counted["generation"] == coverage["generation"] == light["generation"]
    assert counted["fingerprint_count"] == counted["clip_count"] == 0
    assert counted_headers["Cache-Control"] == "no-store"
    for invalid in ({"offset": "1"}, {"offset": "bad"}, {"offset": "-1"}, {"after": "x" * 1025}):
        query = urllib.parse.urlencode({"generation": light["generation"], "kind": "fingerprints", **invalid})
        local.request("/api/catalog/similar/input?" + query, expected=400)
    query = urllib.parse.urlencode({"generation": light["generation"], "kind": "fingerprints",
                                  "offset": "0", "after": "x" * 1024})
    empty = local.request("/api/catalog/similar/input?" + query)
    assert not empty["items"] and empty["next_cursor"] is None

    keeper = local.ingest(count=1, publication=False)[0]
    new_coverage = status("MISS")
    assert new_coverage["generation"] > coverage["generation"]
    assert new_coverage["online"] == coverage["online"] + 1
    assert status("HIT") == new_coverage
    print("PASS: lightweight snapshots, explicit shared counts, generation-bound statistics cache and cursor validation")

    def item(image, keep=None):
        return {"r2_key": image["r2_key"], "decision_id": "sim-" + str(uuid.uuid4()),
                "keep_r2_keys": [target["r2_key"] for target in (keep if keep is not None else [keeper])]}

    images = local.ingest(publication=False)
    first = item(images[0])
    missing = {**item(keeper), "r2_key": "pixiv/missing/0.png"}
    result = delete([first, missing])
    assert result["ok"] and result["partial"]
    assert result["deleted"] == [first["r2_key"]]
    assert result["failed"][0]["decision_id"] == missing["decision_id"]
    assert len(local.catalog(images[0]["work_id"])) == 2
    assert not snapshot()["result_fresh"]
    pending = admin_get(refresh=False)
    assert pending["ok"] and not pending["pending"] and pending["stale"]
    assert not pending["groups"] and not pending["auto_pairs"]
    blocked = item(images[1])
    stale_batch = delete([first, blocked], refresh=False)
    assert stale_batch["partial"] and stale_batch["deleted"] == [first["r2_key"]]
    assert stale_batch["results"][0]["result"]["replayed"]
    assert stale_batch["failed"][0]["decision_id"] == blocked["decision_id"]
    assert "pending" in stale_batch["failed"][0]["error"]
    assert len(local.catalog(images[0]["work_id"])) == 2, "Stale cache allowed a new deletion"
    replayed = delete([first, missing])
    assert replayed["deleted"] == result["deleted"]
    assert replayed["results"][0]["result"]["replayed"]
    assert len(local.catalog(images[0]["work_id"])) == 2

    # A reused ID must never select another image, nor repeat a cancelled delete.
    conflict = delete([{**first, "r2_key": images[1]["r2_key"]}])
    assert not conflict["deleted"] and conflict["failed"]
    local.request("/api/catalog/image-review", {**first, "action": "restore", "restored": []})
    cancelled = delete([first])
    assert not cancelled["deleted"] and cancelled["failed"]
    assert len(local.catalog(images[0]["work_id"])) == 3

    delete([{"r2_key": images[1]["r2_key"]}], expected=400)
    delete([first, first], expected=400)
    delete([{**first, "decision_id": "sim-../secret"}], expected=400)
    delete([{**first, "keep_r2_keys": [first["r2_key"]]}], expected=400)
    no_keeper = delete([item(images[1], [])])
    assert not no_keeper["deleted"] and no_keeper["failed"]

    # Sibling deletion invalidates the image-count preflight, but a deleted receipt
    # still confirms this item's gallery result and retains its exact old targets.
    mapped = local.ingest()
    selected = item(mapped[1])
    prepared = local.request("/api/catalog/image-review", {**selected, "action": "prepare"})
    local.request("/api/catalog/image-review", {**selected, "action": "delete"})
    _, sibling = local.review(mapped[0])
    sibling("prepare")
    sibling("delete")
    assert not snapshot()["result_fresh"]
    replayed = delete([{key: value for key, value in selected.items() if key != "keep_r2_keys"}], refresh=False)
    assert replayed["deleted"] == [selected["r2_key"]]
    assert replayed["results"][0]["result"]["replayed"]
    assert replayed["telegram_pending"][0]["message_id"] == prepared["targets"][0]["message_id"]
    assert replayed["telegram_pending"][0]["decision_id"] == selected["decision_id"]
    assert not replayed["telegram_failed"]
    print("PASS: admin partial deletion, stable replay, ID conflicts, undo protection, retained Telegram targets")

    def fingerprint(image, sha, bits="0000000000000000", width=200):
        return {"r2_key": image["r2_key"], "solid_color": False, "content_sha256": sha,
                "strict_key": sha, "average_hash": bits, "difference_hash": bits,
                "color_key": "color", "detail_key": "detail", "width": width,
                "height": 200, "bytes": 123, "format": "png", "regions": []}

    def put_fingerprints(*values):
        result = local.request("/api/catalog/fingerprints", {"images": list(values)}, method="PUT")
        assert result["upserted"] == len(values), "No-op fingerprint batch acknowledgement changed"

    def pair_present(data, a, b):
        keys = {a["r2_key"], b["r2_key"]}
        return any({pair["keep"]["r2_key"], pair["remove"]["r2_key"]} == keys
                   for pair in data["auto_pairs"])

    def review_pair_present(data, a, b):
        keys = {a["r2_key"], b["r2_key"]}
        return any({image["r2_key"] for image in group["items"]} == keys for group in data["groups"])

    a, b = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
    sha = "a" * 64
    put_fingerprints(fingerprint(a, sha), fingerprint(b, sha))
    pending = admin_get(refresh=False)
    assert pending["ok"] and not pending["pending"] and pending["stale"]
    assert not pending["groups"] and not pending["auto_pairs"], "Old scans must not discover new pairs"
    published = ensure_fresh()
    before_noop = snapshot()
    put_fingerprints(fingerprint(a, sha), fingerprint(b, sha))
    assert snapshot() == before_noop and before_noop["result_fresh"], "Identical fingerprint batch invalidated cache"
    assert pair_present(admin_get(), a, b)
    assert pair_present(admin_get(), a, b)  # Exercise the cached response too.
    old_page = local.request("/api/catalog/similar/input?" + urllib.parse.urlencode({
        "generation": published["generation"], "kind": "fingerprints", "limit": 1}))
    assert old_page["next_cursor"]
    put_fingerprints(fingerprint(b, "b" * 64, "ffffffffffffffff"))
    changed = snapshot()
    assert changed["generation"] > published["generation"] and not changed["result_fresh"]
    local.request("/api/catalog/similar/input?" + urllib.parse.urlencode({
        "generation": published["generation"], "kind": "fingerprints", "after": old_page["next_cursor"]}),
        expected=409)
    local.request("/api/catalog/similar/result", published, expected=409)
    assert snapshot()["result_generation"] == published["generation"], "Stale publication replaced cache"
    filtered = admin_get(refresh=False)
    assert filtered["stale"] and not pair_present(filtered, a, b), "Changed fingerprints retained an old strict pair"
    rejected = delete([item(b, [a])], refresh=False)
    assert not rejected["deleted"] and rejected["failed"], "An invalidated strict pair permitted a new deletion"
    assert local.catalog(b["work_id"])
    current = ensure_fresh()
    local.request("/api/catalog/similar/result", published, expected=409)
    after_reject = snapshot()
    assert after_reject["result_fresh"] and after_reject["result_generation"] == current["generation"]
    assert not pair_present(admin_get(), a, b), "Same-count fingerprint update reused a stale cache"
    for changes in ({"solid_color": True}, {"regions": [{"width": 100, "height": 100,
                     "average_hash": "0000000000000000", "difference_hash": "0000000000000000", "color_key": "region"}]}):
        before_field = snapshot()
        put_fingerprints({**fingerprint(b, "b" * 64, "ffffffffffffffff"), **changes})
        after_field = snapshot()
        assert after_field["generation"] == before_field["generation"] + 1 and not after_field["result_fresh"]
        ensure_fresh()
    put_fingerprints(fingerprint(b, "b" * 64, "ffffffffffffffff"))
    ensure_fresh()
    print("PASS: authenticated offload, no-op fingerprint batches retain cache, changed fields and stale generations invalidate")

    # Hide one image and add a different online image without changing hash count.
    before_hide = status("HIT")
    local.request("/api/admin/works/" + urllib.parse.quote(a["work_id"], safe="") + "/hide", {},
                  auth=False, headers={"Origin": origin, "Cookie": cookie})
    after_hide = status("MISS")
    assert after_hide["generation"] > before_hide["generation"]
    assert after_hide["online"] == before_hide["online"] - 1
    assert after_hide["hashed"] == before_hide["hashed"] - 1
    assert status("HIT") == after_hide
    assert snapshot(counts=True)["fingerprint_count"] == after_hide["hashed"]
    replacement = local.ingest(count=1, publication=False)[0]
    put_fingerprints(fingerprint(replacement, "b" * 64, "ffffffffffffffff"))
    swapped = admin_get()
    assert pair_present(swapped, b, replacement), "Same-count membership change reused a stale cache"
    assert all(a["r2_key"] not in {pair["keep"]["r2_key"], pair["remove"]["r2_key"]}
               for pair in swapped["auto_pairs"])

    # CLIP input updates must invalidate cache with unchanged row count as well.
    c, d = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
    put_fingerprints(fingerprint(c, "c" * 64), fingerprint(d, "d" * 64, "aaaaaaaaaaaaaaaa"))

    def embedding(image, axis):
        values = [0.0] * 512
        values[axis] = 1.0
        return {"r2_key": image["r2_key"], "dim": 512,
                "embedding": struct.pack("<512f", *values).hex()}

    local.request("/api/catalog/clip", {"images": [embedding(b, 1), embedding(replacement, 2),
                  embedding(c, 0), embedding(d, 0)]}, method="PUT")
    semantic = admin_get()
    before_noop = snapshot()
    repeated = local.request("/api/catalog/clip", {"images": [embedding(b, 1), embedding(replacement, 2),
                  embedding(c, 0), embedding(d, 0)]}, method="PUT")
    assert repeated["upserted"] == 4, "No-op CLIP batch acknowledgement changed"
    assert snapshot() == before_noop and before_noop["result_fresh"], "Identical CLIP batch invalidated cache"
    assert review_pair_present(semantic, c, d)
    assert not pair_present(semantic, c, d), "High CLIP score entered automatic cleanup"
    assert all(image["kind"] != "dup" for group in semantic["groups"] for image in group["items"])

    # The vectors remain identical: a solid-color flag must override their CLIP
    # score, including legacy payloads falsely marked as the current generation.
    put_fingerprints({**fingerprint(c, "c" * 64), "solid_color": True})
    filtered = admin_get(refresh=False)
    assert filtered["stale"] and not review_pair_present(filtered, c, d)
    local.local_d1("UPDATE similar_cache SET payload=json_set(payload,'$.generation',"
                   "(SELECT generation FROM similar_generation WHERE id=1)) WHERE id=1")
    assert snapshot()["result_fresh"], "Fixture must exercise a fresh legacy payload"
    filtered = admin_get(refresh=False)
    assert not filtered["pending"] and not filtered["stale"]
    assert all(image["r2_key"] != c["r2_key"] for group in filtered["groups"] for image in group["items"])
    assert all(c["r2_key"] not in {pair["keep"]["r2_key"], pair["remove"]["r2_key"]}
               for pair in filtered["auto_pairs"]), "Fresh legacy cache returned a solid-color candidate"
    put_fingerprints(fingerprint(c, "c" * 64))
    print("PASS: identical CLIP vectors cannot return a solid-color member, even from a fresh legacy cache")

    # Even identical input must not reuse a previous algorithm's classification.
    local.local_d1("UPDATE similar_cache SET payload=json_set(payload,'$.algorithm','legacy-policy','$.response.groups',json('[]'),'$.response.auto_pairs',json('[]')) WHERE id=1")
    assert not snapshot()["result_fresh"]
    assert admin_get(refresh=False)["pending"]
    assert review_pair_present(admin_get(), c, d), "Old algorithm cache survived the policy version change"
    before_changed_clip = snapshot()
    local.request("/api/catalog/clip", {"images": [embedding(d, 3)]}, method="PUT")
    after_changed_clip = snapshot()
    assert after_changed_clip["generation"] == before_changed_clip["generation"] + 1 and not after_changed_clip["result_fresh"]
    filtered = admin_get(refresh=False)
    assert filtered["stale"] and not review_pair_present(filtered, c, d), "Changed CLIP retained an old semantic group"
    rejected = delete([item(d, [c])], refresh=False)
    assert not rejected["deleted"] and rejected["failed"], "An invalidated CLIP group permitted a new deletion"
    assert local.catalog(d["work_id"])
    assert not review_pair_present(admin_get(), c, d), "Same-count CLIP update reused a stale cache"
    print("PASS: same-count hash/CLIP changes and online membership changes invalidate similar cache")

    # A new unrelated work must not hide still-valid proposals from the previous
    # daily scan. Approval rechecks only those proposals, without publishing again.
    strict_images = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
    semantic_images = [local.ingest(count=1, publication=False)[0] for _ in range(3)]
    strict_sha = uuid.uuid4().hex
    put_fingerprints(*(fingerprint(image, strict_sha, "5555555555555555") for image in strict_images),
                     *(fingerprint(image, uuid.uuid4().hex, bits, width=201 + index)
                       for index, (image, bits) in enumerate(zip(semantic_images,
                           ("3333333333333333", "6666666666666666", "9999999999999999")))))
    local.request("/api/catalog/clip", {"images": [embedding(image, 5 + index)
                  for index, image in enumerate(strict_images)] +
                  [embedding(image, 4) for image in semantic_images]}, method="PUT")
    proposals = admin_get()
    strict_keys = {image["r2_key"] for image in strict_images}
    semantic_keys = {image["r2_key"] for image in semantic_images}
    pair = next(pair for pair in proposals["auto_pairs"]
                if {pair["keep"]["r2_key"], pair["remove"]["r2_key"]} == strict_keys)
    group = next(group for group in proposals["groups"]
                 if {image["r2_key"] for image in group["items"]} == semantic_keys)
    assert proposals["engine"] == "clip"
    scanned_generation = snapshot()["result_generation"]
    local.ingest(count=1, publication=False)
    retained = admin_get(refresh=False)
    assert not retained["pending"] and retained["stale"] and retained["computed_at"]
    assert pair_present(retained, *strict_images)
    assert any({image["r2_key"] for image in candidate["items"]} == semantic_keys
               for candidate in retained["groups"])
    assert admin_get(refresh=False) == retained, "Cached bounded revalidation changed the proposal"

    strict_by_key = {image["r2_key"]: image for image in strict_images}
    approved = delete([item(strict_by_key[pair["remove"]["r2_key"]],
                            [strict_by_key[pair["keep"]["r2_key"]]])], refresh=False)
    assert not approved["failed"] and approved["deleted"] == [pair["remove"]["r2_key"]]
    assert not pair_present(admin_get(refresh=False), *strict_images), "Deleted group members remained reviewable"

    semantic_by_key = {image["r2_key"]: image for image in semantic_images}
    keep = next(semantic_by_key[image["r2_key"]] for image in group["items"] if image["keep"])
    plans = [item(semantic_by_key[image["r2_key"]], [keep])
             for image in group["items"] if not image["keep"]]
    approved = delete(plans, refresh=False)
    assert not approved["failed"] and set(approved["deleted"]) == {plan["r2_key"] for plan in plans}
    generations = [result["result"]["generation"] for result in approved["results"]]
    assert len(generations) == 2 and generations[1] > generations[0], "Same-batch version progression failed"
    assert local.catalog(keep["work_id"]), "A stale approval deleted the last keeper"
    assert snapshot()["result_generation"] == scanned_generation, "Approval unexpectedly republished a full scan"
    before_replay = snapshot()["generation"]
    replayed = delete(plans, refresh=False)
    assert not replayed["failed"] and all(result["result"]["replayed"] for result in replayed["results"])
    assert snapshot()["generation"] == before_replay, "Receipt replay advanced the verified generation"
    print("PASS: unrelated ingest retains strict/CLIP candidates; stale approvals and two-item batch work without republishing")

    hidden_images = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
    hidden_sha = uuid.uuid4().hex
    put_fingerprints(*(fingerprint(image, hidden_sha, "7777777777777777") for image in hidden_images))
    local.request("/api/catalog/clip", {"images": [embedding(image, 20 + index)
                  for index, image in enumerate(hidden_images)]}, method="PUT")
    assert pair_present(admin_get(), *hidden_images)
    local.request("/api/admin/works/" + urllib.parse.quote(hidden_images[0]["work_id"], safe="") + "/hide", {},
                  auth=False, headers={"Origin": origin, "Cookie": cookie})
    filtered = admin_get(refresh=False)
    assert filtered["stale"] and not pair_present(filtered, *hidden_images), "Hidden members remained reviewable"
    rejected = delete([item(hidden_images[1], [hidden_images[0]])], refresh=False)
    assert not rejected["deleted"] and rejected["failed"]
    assert local.catalog(hidden_images[1]["work_id"])
    print("PASS: hidden or deleted cached members cannot authorize stale approvals")

    # Old UI plans retain their exact keepers even after the first delete turns a
    # pair into a singleton. Neither delayed nor simultaneous opposing plans may
    # remove both originals. All fixtures are gallery-only: no Telegram traffic.
    for _ in range(8):
        left, right = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
        pair_sha = uuid.uuid4().hex
        put_fingerprints(fingerprint(left, pair_sha), fingerprint(right, pair_sha))
        plans = [item(left, [right]), item(right, [left])]
        ensure_fresh()
        barrier = threading.Barrier(2)

        def opposing(plan):
            barrier.wait()
            return delete([plan], refresh=False)

        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = [pool.submit(opposing, plan) for plan in plans]
            results = [future.result() for future in futures]
        assert sum(len(result["deleted"]) for result in results) == 1, results
        remaining = [image for image in (left, right) if local.catalog(image["work_id"])]
        assert len(remaining) == 1, "Concurrent opposing plans removed both images"
        survivor = remaining[0]
        old_plan = next(plan for plan in plans if plan["r2_key"] == survivor["r2_key"])
        delayed = delete([old_plan])
        assert not delayed["deleted"] and delayed["failed"]
        assert local.catalog(survivor["work_id"]), "Delayed old UI plan removed the last keeper"

    # Keeper guards are persisted in the existing receipt. Changing the keeper's
    # version between prepare and delete must roll back the selected deletion.
    left, right = [local.ingest(count=1, publication=False)[0] for _ in range(2)]
    plan = item(left, [right])
    stale = delete([{**plan, "keep_r2_keys": ["pixiv/missing/keeper.png"]}])
    assert not stale["deleted"] and stale["failed"]
    assert local.catalog(left["work_id"])
    local.request("/api/catalog/image-review", {**plan, "action": "prepare", "allow_missing_telegram": True})
    captured = json.dumps([[{"r2_key": right["r2_key"], "work_id": right["work_id"], "review_version": 0}]])
    captured = captured.replace("'", "''")
    local.local_d1(f"UPDATE catalog_image_reviews SET payload=json_set(payload,'$.similar_survivors',json('{captured}')) WHERE decision_id='{plan['decision_id']}'")
    local.local_d1(f"UPDATE works SET review_version=review_version+1 WHERE id='{right['work_id']}'")
    conflicted = delete([plan])
    assert not conflicted["deleted"] and conflicted["failed"]
    assert local.catalog(left["work_id"])
    unchanged = local.request("/api/catalog/image-review", {**plan, "action": "prepare"})
    assert unchanged["state"] == "prepared", "Keeper conflict failed to roll back its receipt"
    print("PASS: simultaneous/delayed opposing plans preserve one image; keeper version conflicts roll back D1 mutations")


if __name__ == "__main__":
    main()
