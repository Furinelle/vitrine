"""Disposable local curated-set regressions (migration 0022); never run against production.

Use VITRINE_TEST_BASE, VITRINE_TEST_STATE, and the fixture token documented in
test_image_review.py. Start Wrangler with --local-upstream 127.0.0.1.
"""
import urllib.parse
import uuid

import test_image_review as local


def main():
    endpoint = urllib.parse.urlsplit(local.BASE)
    assert endpoint.scheme == "http" and endpoint.hostname in ("127.0.0.1", "localhost", "::1"), "Local test only"
    origin = local.BASE.rstrip("/")
    _, _, headers = local.fetch("/api/admin/login", {"token": "local-image-review-test"},
                                auth=False, headers={"Origin": origin, "CF-Connecting-IP": "192.0.2.174"})
    auth = {"Origin": origin, "Cookie": headers["Set-Cookie"].split(";", 1)[0]}

    def admin(url, payload=None, expected=200, **kwargs):
        return local.request(url, payload, expected, auth=False, headers=auth, **kwargs)

    def decide(images, verdict, expected=200, collection="furina"):
        return admin("/api/admin/curation", {"collection": collection, "r2_keys": [i["r2_key"] for i in images],
                                             "verdict": verdict}, expected, method="POST")

    tag = uuid.uuid4().hex[:8]
    furina = local.ingest(count=3, publication=False, meta_overrides={"tags": ["芙宁娜", tag]})
    by_alias = local.ingest(count=1, publication=False, meta_overrides={"tags": [f"Furina-{tag}"]})
    other = local.ingest(count=1, publication=False, meta_overrides={"tags": [f"other-{tag}"]})
    r18 = local.ingest(count=1, publication=False, meta_overrides={"tags": ["芙宁娜"], "is_r18": True})
    ours = {images[0]["work_id"] for images in (furina, by_alias, other, r18)}

    def candidates():
        return {i["r2_key"]: i for i in admin("/api/admin/curation?collection=furina")["images"] if i["work"]["id"] in ours}

    def published():
        works = local.request("/api/curation/furina", auth=False)["works"]
        return {w["id"]: [p["url"].rsplit("/", 1)[1] for p in w["pages"]] for w in works if w["id"] in ours}

    # Admin endpoints need the cookie; unknown sets and bad bodies are rejected.
    local.fetch("/api/admin/curation?collection=furina", expected=401, auth=False)
    local.fetch("/api/admin/curation", {"collection": "furina", "r2_keys": ["x"], "verdict": "keep"},
                expected=401, auth=False, method="POST")
    admin("/api/admin/curation?collection=nobody", expected=404)
    local.fetch("/api/curation/nobody", expected=404, auth=False)
    decide(furina[:1], "maybe", 400)
    decide([], "keep", 400)
    decide(furina[:1], "keep", 404, collection="nobody")

    # Candidates: every page of tagged works (name or alias match), never R18 or untagged works.
    listed = candidates()
    assert set(listed) == {i["r2_key"] for i in furina + by_alias}, listed.keys()
    assert all(i["verdict"] is None for i in listed.values())
    assert published() == {}, "nothing is published before review"

    decide([furina[0], furina[2], by_alias[0]], "keep")
    decide([furina[1]], "exclude")
    assert {k: i["verdict"] for k, i in candidates().items()} == {
        furina[0]["r2_key"]: "keep", furina[1]["r2_key"]: "exclude",
        furina[2]["r2_key"]: "keep", by_alias[0]["r2_key"]: "keep"}
    pages = published()
    assert pages[furina[0]["work_id"]] == ["00.png", "02.png"], pages
    assert pages[by_alias[0]["work_id"]] == ["00.png"]

    # Clearing a verdict sends the image back to review and unpublishes it; re-deciding overwrites.
    decide([furina[2]], None)
    assert candidates()[furina[2]["r2_key"]]["verdict"] is None
    decide([furina[1]], "keep")
    assert published()[furina[0]["work_id"]] == ["00.png", "01.png"]

    # Deleting an image from the gallery drops it from the published set.
    _, act = local.review(furina[0])
    act("prepare", allow_missing_telegram=True)
    act("delete", allow_missing_telegram=True)
    assert published()[furina[0]["work_id"]] == ["01.png"]
    assert furina[0]["r2_key"] not in candidates()
    print("PASS: curated set lists tagged candidates, publishes only kept pages and follows gallery deletions")


if __name__ == "__main__":
    main()
