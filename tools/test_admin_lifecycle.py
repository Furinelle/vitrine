"""Disposable local admin hide/restore regressions; never run against production.

Use VITRINE_TEST_BASE, VITRINE_TEST_STATE, and the fixture token documented in
test_image_review.py. Start Wrangler with --local-upstream 127.0.0.1.
"""
import subprocess
import urllib.parse
import uuid

import test_image_review as local


def main():
    endpoint = urllib.parse.urlsplit(local.BASE)
    assert endpoint.scheme == "http" and endpoint.hostname in ("127.0.0.1", "localhost", "::1"), "Local test only"
    origin = local.BASE.rstrip("/")
    _, _, headers = local.fetch("/api/admin/login", {"token": "local-image-review-test"},
                                auth=False, headers={"Origin": origin, "CF-Connecting-IP": "192.0.2.173"})
    auth = {"Origin": origin, "Cookie": headers["Set-Cookie"].split(";", 1)[0]}

    def path(work, action=""):
        return "/api/admin/works/" + urllib.parse.quote(work, safe="") + action

    def admin(url, payload=None, expected=200, **kwargs):
        return local.request(url, payload, expected, auth=False, headers=auth, **kwargs)

    def hide(work):
        return admin(path(work, "/hide"), {}, method="POST")

    def restore(work, expected=200):
        return admin(path(work, "/restore"), {}, expected, method="POST")

    def detail(work):
        return admin(path(work))

    images = local.ingest(count=3, publication=False)
    work = images[0]["work_id"]
    local.fetch("/api/admin/works?status=hidden", expected=401, auth=False)
    local.fetch(path(work), expected=401, auth=False)
    for action in ("/hide", "/restore"):
        local.fetch(path(work, action), {}, expected=401, auth=False, method="POST")
        local.fetch(path(work, action), {}, expected=403, auth=False, method="POST",
                    headers={**auth, "Origin": "null", "Referer": origin + "/admin"})
    patch = {"title": "Lifecycle preserved", "author_name": "Local fixture",
             "author_url": "https://example.test/author", "tags": ["lifecycle tag", "preserved"]}
    admin(path(work), patch, method="PATCH")
    before = detail(work)["work"]
    _, prepared = local.review(images[1])
    prepared("prepare", allow_missing_telegram=True)
    hide(work)
    local.public_images(work, expected=404)
    hidden = detail(work)
    assert hidden["work"]["can_restore"] and hidden["work"]["deleted_at"]
    assert hidden["work"]["review_version"] == before["review_version"] + 1
    listing = admin("/api/admin/works?status=hidden&q=" + urllib.parse.quote(work.split(":", 1)[1]))
    assert [item["id"] for item in listing["works"]] == [work]
    assert hide(work)["replayed"]
    assert detail(work)["work"]["review_version"] == hidden["work"]["review_version"]
    for image in images:
        for suffix in ("", "?width=160", "?width=640", "?width=1280"):
            local.fetch("/media/" + image["r2_key"] + suffix, expected=404, auth=False)
        _, body, headers = local.fetch("/media/" + image["r2_key"] + "?width=640", auth=False, headers=auth)
        assert body and headers["Cache-Control"] == "private, no-store"
        assert headers["Cross-Origin-Resource-Policy"] == "same-origin"
    restore(work)
    after = detail(work)["work"]
    assert after["review_version"] == before["review_version"] + 2
    for field in ("title", "author_name", "author_url", "tags"):
        assert after[field] == before[field], (field, before, after)
    assert restore(work)["replayed"]
    assert detail(work)["work"]["review_version"] == after["review_version"]
    assert len(local.public_images(work)["images"]) == 3
    prepared("delete", expected=409)
    for image in images:
        local.fetch("/media/" + image["r2_key"] + "?width=640", auth=False)

    # Work restore must never put an old single-image review snapshot back.
    _, removed = local.review(images[0])
    removed("prepare", allow_missing_telegram=True)
    removed("delete")
    hide(work)
    restore(work)
    assert len(local.public_images(work)["images"]) == 2
    removed("restore", expected=409, restored=[])
    local.fetch("/media/" + images[0]["r2_key"], expected=404, auth=False)

    # A full prune has no live image metadata and cannot be revived here.
    keeper = local.ingest(count=1, publication=False)[0]
    pruned = local.ingest(count=1, publication=False)[0]
    local.request("/api/catalog/prune", {"decision_id": uuid.uuid4().hex,
        "keep_r2_key": keeper["r2_key"], "remove_r2_keys": [pruned["r2_key"]]})
    assert detail(pruned["work_id"])["images"] == []
    assert not detail(pruned["work_id"])["work"]["can_restore"]
    restore(pruned["work_id"], expected=409)

    # Retract a manually hidden work for real, rather than treating it as replay.
    retracted = local.ingest(count=2, publication=False)[0]
    hide(retracted["work_id"])
    local.request("/api/catalog/retract", {"decision_id": uuid.uuid4().hex, "work_id": retracted["work_id"]})
    assert detail(retracted["work_id"])["images"] == []
    restore(retracted["work_id"], expected=409)
    local.fetch("/media/" + retracted["r2_key"], expected=404, auth=False, headers=auth)
    republished = local.ingest(source_id=retracted["work_id"].split(":", 1)[1], count=1, publication=False)
    assert len(local.public_images(retracted["work_id"])["images"]) == 1
    assert not detail(retracted["work_id"])["work"]["deleted_at"]
    local.fetch("/media/" + republished[0]["r2_key"], auth=False)

    # A normal source update must preserve the owner's manual visibility choice.
    updated = local.ingest(count=1, publication=False)[0]
    hide(updated["work_id"])
    old_hidden = detail(updated["work_id"])["work"]
    assert local.ingest(source_id=updated["work_id"].split(":", 1)[1], count=2, publication=False) == []
    local.public_images(updated["work_id"], expected=404)
    current = detail(updated["work_id"])
    assert current["work"]["deleted_at"] == old_hidden["deleted_at"]
    assert current["work"]["review_version"] > old_hidden["review_version"]
    assert len(current["images"]) == 2 and current["work"]["can_restore"]
    current_keys = {image["r2_key"] for image in current["images"]}
    assert updated["r2_key"] not in current_keys
    restore(updated["work_id"])
    assert {image["r2_key"] for image in local.public_images(updated["work_id"])["images"]} == current_keys
    local.fetch("/media/" + updated["r2_key"], expected=404, auth=False, headers=auth)

    # Matching D1 rows are insufficient if the original object has disappeared.
    missing = local.ingest(count=1, publication=False)[0]
    hide(missing["work_id"])
    subprocess.run(local.WRANGLER + ["r2", "object", "delete", "vitrine-media/" + missing["r2_key"],
                                   "--local", "--persist-to", local.STATE], check=True, capture_output=True)
    restore(missing["work_id"], expected=409)
    assert detail(missing["work_id"])["work"]["deleted_at"]
    print("PASS: admin authentication, hidden discovery/media, idempotent hide/restore, preserved metadata, invalidated reviews, hidden-source updates, explicit republishing, prune/retract and missing-original protection")


if __name__ == "__main__":
    main()
