#!/usr/bin/env python3
# /// script
# dependencies = ["numpy", "pillow"]
# ///
"""Stitch works whose pages are strips cut from one picture back into that picture.

Some works carry a picture cut into side-by-side strips (or stacked ones), with
or without the whole picture as another page, in any page order. `scan` decides
from the pixels which pages continue each other across their edges: a true cut
continues the fine vertical structure of the last column into the next strip's
first one about as well as the picture's own neighbouring columns do, while
strips that lost pixels at the cut or unrelated pages do not. Strips may also
repeat a few columns of their neighbour. Edges with too little detail to tell
are left alone.

Each chain is joined losslessly: JPEG strips with jpegtran's lossless crop/drop
when their blocks line up, otherwise as lossless WebP (PNG above WebP's size
limit); every result is decoded and compared pixel for pixel with the strips. If
another page of the work shows the same picture, the copy with more pixels (then
more bytes) is kept; the strips are dropped either way. Other pages stay.

`apply` re-ingests each changed work through the normal prepare/commit path
with its new page list, the same metadata and the stored fingerprints of kept
pages; new pages get Hanabi's fingerprint (`hanabi/target/release/fingerprint`).
Replaced R2 objects stay in the bucket as `orphan_objects`, not deleted, and
the Telegram mapping is untouched.

    uv run tools/stitch_strips.py scan RUN_DIR [--work WORK_ID ...]
    uv run tools/stitch_strips.py apply RUN_DIR --fingerprint-bin PATH [--env FILE] [--work WORK_ID ...]

Review RUN_DIR/report.html before applying.
"""
import argparse
import base64
import concurrent.futures
import hashlib
import html
import io
import json
import os
import pathlib
import subprocess
import sys
import urllib.parse
import urllib.request

import numpy as np
from PIL import Image, ImageOps

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from similar_compute import ENDPOINT, api, load_token  # noqa: E402

Image.MAX_IMAGE_PIXELS = None
PAGES_SQL = (
    "SELECT i.work_id, i.page_index, i.r2_key, i.content_type, i.byte_size, i.sha256, f.width, f.height "
    "FROM images i JOIN works w ON w.id = i.work_id LEFT JOIN image_fingerprints f ON f.r2_key = i.r2_key "
    "WHERE w.deleted_at IS NULL AND i.work_id IN (SELECT work_id FROM images GROUP BY work_id HAVING COUNT(*) > 1) "
    "ORDER BY i.work_id, i.page_index"
)
# Calibrated on works whose whole picture is also a page: exact cuts keep the seam
# correlation near the picture's own column-to-column one; unrelated pages stay under 0.6 of it.
MIN_SEAM, SEAM_RATIO, MIN_DETAIL, LAYOUT_MARGIN = 0.5, 0.75, 2.0, 0.3
MAX_JUMP, JUMP_RATIO = 10.0, 4.0
MAX_OVERLAP, OVERLAP_CORR, OVERLAP_DIFF = 64, 0.95, 4.0
SAME_ASPECT, SAME_PICTURE = 0.03, 6.0
# Other crops, zooms or colourings of the stitched picture scored 0.89-0.99; other pages of those works
# at most 0.63 and random pairs at most 0.72.
SIMILAR = 0.82
WEBP_MAX = 16383


def d1(sql):
    out = subprocess.run(["npx", "wrangler", "d1", "execute", "vitrine", "--remote", "--json", "--command", sql],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out[out.index("\n[") + 1:] if not out.startswith("[") else out)[0]["results"]


def in_list(values):
    return ",".join("'" + v.replace("'", "''") + "'" for v in values)


def download(media, keys):
    def get(key):
        path = media / key.replace("/", "_")
        if not path.exists():
            # The zone WAF rejects urllib's default agent.
            request = urllib.request.Request(ENDPOINT + "/media/" + urllib.parse.quote(key),
                                             headers={"User-Agent": "vitrine-stitch-strips/1"})
            with urllib.request.urlopen(request, timeout=120) as response:
                data = response.read()
            path.with_suffix(".part").write_bytes(data)
            path.with_suffix(".part").rename(path)
        return key, path
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        return dict(pool.map(get, keys))


def highpass(x):
    return x - np.convolve(np.pad(x, 15, mode="edge"), np.ones(31) / 31, "valid")


def corr(x, y):
    """Correlation of the fine structure along two columns; None when either is too flat to judge."""
    a, b = highpass(x), highpass(y)
    if min(a.std(), b.std()) < MIN_DETAIL:
        return None
    return float(np.corrcoef(a, b)[0, 1])


def seam(a, b):
    """How `b` continues `a` to the right: ("seam", score, overlap columns), ("flat",) or None."""
    if a.shape[0] != b.shape[0] or min(a.shape[1], b.shape[1]) < 3:
        return None
    inside = [c for c in (corr(a[:, -2], a[:, -1]), corr(b[:, 0], b[:, 1])) if c is not None]
    if not inside:
        return ("flat",)
    neighbour = sum(inside) / len(inside)
    # Pages sharing a layout (letterbox bars, app chrome, card frames) line up at every edge,
    # while the two left edges of true strips lie far apart in the picture.
    layout = max((c for c in (corr(a[:, 0], b[:, 0]), corr(a[:, -1], b[:, -1])) if c is not None), default=0.0)
    # A repeated column matches clearly better than a neighbouring one would.
    for k in range(1, min(MAX_OVERLAP, a.shape[1] - 1, b.shape[1] - 1) + 1):
        c = corr(a[:, -k], b[:, 0])
        if (c is not None and c >= max(OVERLAP_CORR, neighbour + (1 - neighbour) / 2, layout + LAYOUT_MARGIN)
                and np.abs(a[:, -k:] - b[:, :k]).mean() <= OVERLAP_DIFF):
            return "seam", c, k
    across = corr(a[:, -1], b[:, 0])
    if across is None:
        return ("flat",)
    # Shared bars can carry the fine structure; the brightness must still not jump at the seam.
    step = (np.abs(a[:, -2] - a[:, -1]).mean() + np.abs(b[:, 0] - b[:, 1]).mean()) / 2
    if np.abs(a[:, -1] - b[:, 0]).mean() > MAX_JUMP + JUMP_RATIO * step:
        return None
    if across >= max(MIN_SEAM, SEAM_RATIO * neighbour, layout + LAYOUT_MARGIN):
        return "seam", across, 0
    return None


def chains(grays):
    """Ordered chains of indices whose pages continue each other, plus flat edges seen."""
    edges, flat = [], []
    for i, a in enumerate(grays):
        for j, b in enumerate(grays):
            if i != j and (result := seam(a, b)):
                (edges.append((result[1], i, j, result[2])) if result[0] == "seam" else flat.append((i, j)))
    nxt, prv, overlap = {}, {}, {}
    for _, i, j, k in sorted(edges, reverse=True):
        if i in nxt or j in prv:
            continue
        head = i
        while head in prv:
            head = prv[head]
        if head == j:
            continue
        nxt[i], prv[j], overlap[(i, j)] = j, i, k
    out = []
    for start in (i for i in nxt if i not in prv):
        chain = [start]
        while chain[-1] in nxt:
            chain.append(nxt[chain[-1]])
        out.append((chain, [overlap[(a, b)] for a, b in zip(chain, chain[1:])]))
    return out, flat


def oriented(image, axis):
    return image if axis == "x" else image.transpose(Image.Transpose.TRANSPOSE)


def composite(images, overlaps, axis):
    """The joined picture: each strip continues where the previous one's repeated columns begin."""
    arrays = [np.asarray(oriented(im, axis)) for im in images]
    parts = [a[:, :a.shape[1] - k] for a, k in zip(arrays, overlaps)] + [arrays[-1]]
    joined = np.concatenate(parts, axis=1)
    return joined if axis == "x" else joined.transpose(1, 0, *range(2, joined.ndim))


def jpegtran_join(paths, sizes, overlaps, axis, out):
    """Lossless JPEG join; the caller compares the result's pixels before using it."""
    offsets = [0]
    for (w, h), k in zip(sizes, overlaps):
        offsets.append(offsets[-1] + (w if axis == "x" else h) - k)
    total = offsets[-1] + (sizes[-1][0] if axis == "x" else sizes[-1][1])
    w, h = sizes[0]
    canvas = f"{total}x{h}+0+0" if axis == "x" else f"{w}x{total}+0+0"
    tmp = out.with_suffix(".tmp.jpg")
    subprocess.run(["jpegtran", "-copy", "all", "-crop", canvas, "-outfile", str(tmp), str(paths[0])],
                   check=True, capture_output=True)
    for path, offset in zip(paths[1:], offsets[1:]):
        drop = f"+{offset}+0" if axis == "x" else f"+0+{offset}"
        subprocess.run(["jpegtran", "-copy", "all", "-drop", drop, str(path), "-outfile", str(out), str(tmp)],
                       check=True, capture_output=True)
        out.replace(tmp)
    tmp.replace(out)


def blocky(path):
    ppm = subprocess.run(["djpeg", "-nosmooth", "-ppm", str(path)], capture_output=True, check=True).stdout
    return np.asarray(Image.open(io.BytesIO(ppm)).convert("RGB"))


def stitch(paths, overlaps, axis, out_base):
    """Writes the joined picture losslessly and returns its path, or None if it cannot be."""
    images = [Image.open(p) for p in paths]
    if any(im.mode == "CMYK" for im in images):
        return None
    mode = "RGBA" if any("A" in im.getbands() or "transparency" in im.info for im in images) else "RGB"
    expected = composite([im.convert(mode) for im in images], overlaps, axis)
    if all(im.format == "JPEG" for im in images) and mode == "RGB":
        out = out_base.with_suffix(".jpg")
        try:
            jpegtran_join(paths, [im.size for im in images], overlaps, axis, out)
            # Smoothed chroma upsampling blends colour across the new seams; without it every
            # pixel comes from its own blocks, so equal coefficients decode to equal pixels.
            if np.array_equal(blocky(out), composite([Image.fromarray(blocky(p)) for p in paths], overlaps, axis)):
                return out
        except subprocess.CalledProcessError:
            pass
        out.unlink(missing_ok=True)
    picture = Image.fromarray(expected, mode)
    icc = images[0].info.get("icc_profile")
    if max(picture.size) <= WEBP_MAX:
        out = out_base.with_suffix(".webp")
        picture.save(out, lossless=True, exact=True, method=4, icc_profile=icc)
    else:
        out = out_base.with_suffix(".png")
        picture.save(out, icc_profile=icc)
    assert np.array_equal(np.asarray(Image.open(out).convert(mode)), expected), out
    return out


def same_picture(joined, page):
    """Mean grey difference (0-255) after scaling both to a 256-pixel-wide thumbnail."""
    size = (256, max(1, round(256 * page.height / page.width)))
    a = np.asarray(joined.convert("L").resize(size, Image.Resampling.BOX), dtype=np.float32)
    b = np.asarray(page.convert("L").resize(size, Image.Resampling.BOX), dtype=np.float32)
    return float(np.abs(a - b).mean())


def variant_score(a, b, side=96):
    """Best correlation of one picture laid over the other, allowing other crops (up to half the
    width), zoom, shift and any change of brightness or contrast; 1 means the same picture."""
    best = -1.0
    for big, small in ((a, b), (b, a)):
        base = np.asarray(big.convert("L").resize((side, max(1, round(side * big.height / big.width))),
                                                  Image.Resampling.BOX), dtype=np.float32)
        # The middle of the smaller picture, so crops that are shifted against each other still fit.
        inner = small.crop((small.width // 10, small.height // 10, small.width * 9 // 10, small.height * 9 // 10))
        for zoom in (1.0, 1.04, 1.08, 1.12, 1.17, 1.22, 1.28, 1.36, 1.45, 1.55, 1.7, 1.85, 2.0):
            w = round(side * 0.8 / zoom)
            h = round(w * inner.height / inner.width)
            if h > base.shape[0] or h < 8:
                continue
            t = np.asarray(inner.convert("L").resize((w, h), Image.Resampling.BOX), dtype=np.float32)
            t = (t - t.mean()) / (t.std() + 1e-6)
            windows = np.lib.stride_tricks.sliding_window_view(base, (h, w))
            mean = windows.mean(axis=(2, 3), keepdims=True)
            std = windows.std(axis=(2, 3), keepdims=True) + 1e-6
            best = max(best, float(((windows - mean) / std * t).mean(axis=(2, 3)).max()))
    return best


def plan_work(work_id, pages, paths, out_dir):
    images = [Image.open(paths[p["r2_key"]]) for p in pages]
    if any(im.getexif().get(0x0112, 1) != 1 for im in images):
        return {"work_id": work_id, "skip": "EXIF orientation"}
    groups, used, notes = [], set(), []
    for axis in ("x", "y"):
        grays = [np.asarray(oriented(im, axis).convert("L"), dtype=np.float32) for im in images]
        found, flat = chains(grays)
        notes += [f"{axis} 方向第 {pages[i]['page_index']}→{pages[j]['page_index']} 页边缘缺少细节，未判断"
                  for i, j in flat]
        for chain, overlaps in found:
            if used & set(chain):
                return {"work_id": work_id, "skip": "横竖两个方向都能拼接（网格切图）"}
            used |= set(chain)
            groups.append({"axis": axis, "strips": chain, "overlaps": overlaps})
    if not groups:
        # Only worth a look when nothing joined: the flat edges may hide a cut on a plain background.
        return {"work_id": work_id, "notes": notes} if notes else None
    joined_all = []
    for n, group in enumerate(groups):
        chain = group["strips"]
        out = stitch([paths[pages[i]["r2_key"]] for i in chain], group["overlaps"], group["axis"],
                     out_dir / f"{work_id.replace(':', '_')}_{n}")
        if out is None:
            return {"work_id": work_id, "skip": "CMYK 图片无法无损拼接"}
        joined = Image.open(out)
        joined_all.append(joined)
        group.update(stitched=str(out), size=list(joined.size), bytes=out.stat().st_size, full=None, similar=[])
        for i, page in enumerate(images):
            if i in used or abs(page.width / page.height - joined.width / joined.height) > SAME_ASPECT * joined.width / joined.height:
                continue
            diff = same_picture(joined, page)
            if diff <= SAME_PICTURE and (group["full"] is None or diff < group["diff"]):
                group.update(full=i, diff=round(diff, 2))
        if group["full"] is not None:
            used.add(group["full"])
            full = images[group["full"]]
            mine = (joined.width * joined.height, group["bytes"])
            theirs = (full.width * full.height, pages[group["full"]]["byte_size"])
            group["keep"] = "stitched" if mine > theirs else "full"
        else:
            group["keep"] = "stitched"
    # Other crops or colourings of the picture are not the same image: they stay for the similar-image review.
    for i, page in enumerate(images):
        if i not in used:
            score, n = max((variant_score(joined, page), n) for n, joined in enumerate(joined_all))
            if score >= SIMILAR:
                groups[n]["similar"].append({"r2_key": pages[i]["r2_key"], "score": round(score, 3)})
    # New page list: each group's kept picture takes the place of its first page; other pages stay.
    new_pages, placed = [], set()
    owner = {i: n for n, g in enumerate(groups) for i in g["strips"] + ([g["full"]] if g["full"] is not None else [])}
    for i, page in enumerate(pages):
        if i not in owner:
            new_pages.append({"r2_key": page["r2_key"]})
        elif owner[i] not in placed:
            placed.add(owner[i])
            g = groups[owner[i]]
            new_pages.append({"stitched": g["stitched"]} if g["keep"] == "stitched" else {"r2_key": pages[g["full"]]["r2_key"]})
    for g in groups:
        g["strips"] = [pages[i]["r2_key"] for i in g["strips"]]
        g["full"] = None if g["full"] is None else pages[g["full"]]["r2_key"]
    return {"work_id": work_id, "pages": [p["r2_key"] for p in pages], "groups": groups, "new_pages": new_pages}


def scan(run, only):
    run.mkdir(parents=True, exist_ok=True)
    (run / "media").mkdir(exist_ok=True)
    (run / "stitched").mkdir(exist_ok=True)
    rows = d1(PAGES_SQL)
    works = {}
    for row in rows:
        works.setdefault(row["work_id"], []).append(row)
    if only:
        works = {w: works[w] for w in only}
    else:
        # Strips share their height (or width when stacked); a page without stored size is always looked at.
        def may_join(pages):
            sides = [(p["height"], p["width"]) for p in pages]
            return None in sum(sides, ()) or len({h for h, _ in sides}) < len(sides) or len({w for _, w in sides}) < len(sides)
        works = {w: ps for w, ps in works.items() if may_join(ps)}
    print(f"{len(works)} 个多页作品，下载原图…", file=sys.stderr)
    paths = download(run / "media", [p["r2_key"] for ps in works.values() for p in ps])
    results = []
    for n, (work_id, pages) in enumerate(sorted(works.items()), 1):
        if n % 100 == 0:
            print(f"  {n}/{len(works)}", file=sys.stderr)
        if (result := plan_work(work_id, pages, paths, run / "stitched")):
            results.append(result)
    (run / "plan.json").write_text(json.dumps(results, ensure_ascii=False, indent=1))
    report(run, results)
    changed = [r for r in results if r.get("groups")]
    print(f"可拼接 {len(changed)} 个作品；跳过 {sum('skip' in r for r in results)} 个；"
          f"边缘缺少细节待人工看 {sum(bool(r.get('notes')) and not r.get('groups') for r in results)} 个。"
          f"报告：{run / 'report.html'}")


def report(run, results):
    def img(path, label, kind):
        # Thumbnails go inside the page so the report opens anywhere on its own.
        thumb = Image.open(path).convert("RGB")
        thumb.thumbnail((600, 200))
        data = io.BytesIO()
        thumb.save(data, "JPEG", quality=75)
        return (f'<figure class="{kind}"><img loading="lazy" src="data:image/jpeg;base64,{base64.b64encode(data.getvalue()).decode()}">'
                f'<figcaption>{html.escape(label)}</figcaption></figure>')
    media = lambda key: run / "media" / key.replace("/", "_")  # noqa: E731
    name = lambda key: key.rsplit("/", 1)[-1]  # noqa: E731
    sections = {"full": [], "alone": []}
    counts = {"strips": 0, "full": 0, "similar": 0, "other": 0}
    for r in results:
        if not r.get("groups"):
            continue
        cards, sorted_keys = [], set()
        for g in r["groups"]:
            figs = [img(media(k), f"局部图 {name(k)}", "strip") for k in g["strips"]]
            figs.append(img(g["stitched"], f"拼接结果 {g['size'][0]}×{g['size'][1]} {g['bytes'] // 1024} KiB"
                            + ("（保留）" if g["keep"] == "stitched" else "（舍弃）"), "stitched"))
            if g["full"]:
                figs.append(img(media(g["full"]), f"相同完整图 {name(g['full'])} 差异 {g['diff']}"
                                + ("（保留）" if g["keep"] == "full" else "（舍弃）"), "full"))
            figs += [img(media(x["r2_key"]), f"相似图 {name(x['r2_key'])} 相似度 {x['score']}（保留）", "similar")
                     for x in g["similar"]]
            sorted_keys |= set(g["strips"]) | {g["full"]} | {x["r2_key"] for x in g["similar"]}
            counts["strips"] += len(g["strips"])
            counts["full"] += bool(g["full"])
            counts["similar"] += len(g["similar"])
            overlap = f"，重叠 {g['overlaps']} 列" if any(g["overlaps"]) else ""
            cards.append(f'<p>{"左右" if g["axis"] == "x" else "上下"}拼接 {len(g["strips"])} 张{overlap}</p>'
                         f'<div class="row">{"".join(figs)}</div>')
        others = [k for k in r["pages"] if k not in sorted_keys]
        counts["other"] += len(others)
        if others:
            cards.append('<div class="row">' + "".join(img(media(k), f"其他页面 {name(k)}（保留）", "other") for k in others) + "</div>")
        section = "full" if any(g["full"] for g in r["groups"]) else "alone"
        sections[section].append(f'<section><h3>{html.escape(r["work_id"])}　{len(r["pages"])} 页 → {len(r["new_pages"])} 页</h3>'
                                  + "".join(cards) + "</section>")
    unsure = [r for r in results if not r.get("groups")]
    summary = (f'<p>可拼接作品 {len(sections["full"]) + len(sections["alone"])} 个：局部图 {counts["strips"]} 张，'
               f'相同完整图 {counts["full"]} 张（与拼接结果比画质，留一张），相似图 {counts["similar"]} 张（裁切、缩放或调色不同，'
               f'保留并交给相似图审批），其他页面 {counts["other"]} 张（保留）。</p>')
    body = (summary + f'<h2>一、可拼接，且有相同的完整图（{len(sections["full"])}）</h2>' + "".join(sections["full"])
            + f'<h2>二、可拼接，没有完整图（{len(sections["alone"])}）</h2>' + "".join(sections["alone"])
            + f'<details><summary>三、未处理（{len(unsure)}）：边缘缺少细节无法判断，或跳过</summary><ul>'
            + "".join(f'<li>{html.escape(r["work_id"])}：{html.escape(r.get("skip") or "；".join(r["notes"]))}</li>' for r in unsure)
            + "</ul></details>")
    (run / "report.html").write_text(
        '<!doctype html><meta charset="utf-8"><title>Strip stitching</title><style>'
        'body{font:14px system-ui;margin:16px;background:#fff;color:#111}section{border-top:1px solid #ddd;padding:8px 0}'
        '.row{display:flex;gap:8px;align-items:flex-start;overflow-x:auto}figure{margin:0}'
        'img{height:200px;border:3px solid #ccc}figcaption{font-size:12px}.stitched img{border-color:#2a7}'
        '.full img{border-color:#27c}.similar img{border-color:#e90}.other img{border-color:#999}</style>' + body)


def fingerprint_input(row):
    return {"solid_color": bool(row["solid_color"]), "content_sha256": row["content_sha256"],
            "strict_key": row["strict_key"], "average_hash": row["average_hash"],
            "difference_hash": row["difference_hash"], "color_key": row["color_key"],
            "detail_key": row["detail_key"], "width": row["width"], "height": row["height"],
            "bytes": row["bytes"], "format": row["format"], "regions": json.loads(row["regions_json"])}


def put(url, headers, data):
    request = urllib.request.Request(url, data=data, method="PUT", headers=headers)
    with urllib.request.urlopen(request, timeout=300) as response:
        response.read()


def apply(run, only, token, fingerprint_bin):
    plan = [r for r in json.loads((run / "plan.json").read_text()) if r.get("groups")]
    if only:
        plan = [r for r in plan if r["work_id"] in only]
    log = run / "applied.jsonl"
    done = {json.loads(line)["work_id"] for line in log.read_text().splitlines()} if log.exists() else set()
    for r in plan:
        work_id = r["work_id"]
        if work_id in done:
            continue
        current = d1(f"SELECT r2_key, sha256, content_type FROM images WHERE work_id = {in_list([work_id])} ORDER BY page_index")
        if [c["r2_key"] for c in current] != r["pages"]:
            print(f"{work_id}: 页面在扫描后变过，跳过；请重新 scan", file=sys.stderr)
            continue
        work = d1(f"SELECT * FROM works WHERE id = {in_list([work_id])} AND deleted_at IS NULL")
        if not work:
            print(f"{work_id}: 作品已下线，跳过", file=sys.stderr)
            continue
        work = work[0]
        tags = [t["tag"] for t in d1(f"SELECT tag FROM work_tags WHERE work_id = {in_list([work_id])} ORDER BY rowid")]
        kept = [p["r2_key"] for p in r["new_pages"] if "r2_key" in p]
        stored = {f["r2_key"]: f for f in d1(f"SELECT * FROM image_fingerprints WHERE r2_key IN ({in_list(kept)})")} if kept else {}
        types = {c["r2_key"]: c for c in current}
        files, blobs = [], []
        for n, page in enumerate(r["new_pages"]):
            if "r2_key" in page:
                data = (run / "media" / page["r2_key"].replace("/", "_")).read_bytes()
                content_type = types[page["r2_key"]]["content_type"]
                fp = stored.get(page["r2_key"])
                fingerprint = fingerprint_input(fp) if fp and fp["content_sha256"] == hashlib.sha256(data).hexdigest() else None
            else:
                path = pathlib.Path(page["stitched"])
                data = path.read_bytes()
                content_type = {".jpg": "image/jpeg", ".webp": "image/webp", ".png": "image/png"}[path.suffix]
                fingerprint = json.loads(subprocess.run([fingerprint_bin, str(path)], capture_output=True,
                                                        text=True, check=True).stdout)
            digest = hashlib.sha256(data).hexdigest()
            name = f"{n:02d}" + {"image/jpeg": ".jpg", "image/webp": ".webp", "image/png": ".png"}.get(content_type, ".jpg")
            file = {"name": name, "content_type": content_type, "size": len(data), "sha256": digest}
            if fingerprint:
                file["fingerprint"] = fingerprint
            files.append(file)
            blobs.append(data)
        meta = json.dumps({k: work[k] for k in ("source", "source_id", "source_url", "title", "author_name",
                                                 "author_url", "origin")} | {"tags": tags},
                          ensure_ascii=False)
        key = "stitch-" + hashlib.sha256((work_id + "".join(f["sha256"] for f in files)).encode()).hexdigest()[:32]
        prepared = idempotent_api(token, "/api/ingest/prepare", {"meta": meta, "files": files}, key)
        if "uploads" in prepared:
            for upload, data in zip(prepared["uploads"], blobs):
                put(upload["url"], upload["headers"], data)
            idempotent_api(token, "/api/ingest/commit", {"meta": meta, "files": files, "ingest_id": prepared["ingest_id"]}, key)
        with log.open("a") as f:
            f.write(json.dumps({"work_id": work_id, "replaced": r["pages"], "idempotency_key": key}) + "\n")
        print(f"{work_id}: {len(r['pages'])} 页 → {len(files)} 页", file=sys.stderr)


def idempotent_api(token, path, body, key):
    request = urllib.request.Request(ENDPOINT + path, data=json.dumps(body, separators=(",", ":")).encode(), method="POST",
                                     headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json",
                                              "Idempotency-Key": key, "User-Agent": "vitrine-stitch-strips/1"})
    with urllib.request.urlopen(request, timeout=120) as response:
        result = json.loads(response.read())
    if result.get("ok") is not True:
        raise RuntimeError(f"{path}: {result}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("scan")
    s.add_argument("run", type=pathlib.Path)
    s.add_argument("--work", action="append", default=[])
    a = sub.add_parser("apply")
    a.add_argument("run", type=pathlib.Path)
    a.add_argument("--work", action="append", default=[])
    a.add_argument("--env", help="env file containing HANABI_GALLERY_TOKEN; otherwise use the environment")
    a.add_argument("--fingerprint-bin", required=True, help="hanabi's target/release/fingerprint")
    args = parser.parse_args()
    if args.command == "scan":
        scan(args.run, args.work)
    else:
        token = load_token(args.env) if args.env else os.environ.get("HANABI_GALLERY_TOKEN", "")
        if not token:
            sys.exit("missing HANABI_GALLERY_TOKEN")
        apply(args.run, args.work, token, args.fingerprint_bin)


if __name__ == "__main__":
    main()
