# /// script
# dependencies = ["numpy", "pillow"]
# ///
"""Offline check of tools/stitch_strips.py on synthetic pictures: shuffled strips are found and joined
pixel for pixel, repeated columns are dropped, other crops or colourings are told apart as similar
pictures; strips with missing columns, unrelated pages and
framed pages sharing a layout are left alone.
Run with: uv run tools/test_stitch_strips.py
"""
import pathlib
import sys
import tempfile

import numpy as np
from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import stitch_strips  # noqa: E402


def picture(seed, width=1440, height=960):
    rng = np.random.default_rng(seed)
    # Fine detail for the seams over large shapes for the whole-picture comparisons.
    noise = rng.normal(0, 1, (height // 4, width // 4, 3)) + np.asarray(Image.fromarray(
        rng.integers(0, 256, (height // 160, width // 160, 3), np.uint8)).resize((width // 4, height // 4), Image.Resampling.BICUBIC)) / 40
    smooth = np.asarray(Image.fromarray(((noise - noise.min()) / np.ptp(noise) * 255).astype(np.uint8))
                        .resize((width, height), Image.Resampling.BICUBIC), dtype=np.float32)
    return np.clip(smooth + rng.normal(0, 6, smooth.shape), 0, 255).astype(np.uint8)


def work(tmp, name, arrays, fmt):
    pages, paths = [], {}
    for n, array in enumerate(arrays):
        key = f"{name}/{n:02d}.{fmt}"
        path = tmp / key.replace("/", "_")
        Image.fromarray(array).save(path, **({"quality": 92} if fmt == "jpg" else {}))
        pages.append({"r2_key": key, "page_index": n, "byte_size": path.stat().st_size})
        paths[key] = path
    return pages, paths


with tempfile.TemporaryDirectory() as tmp:
    tmp = pathlib.Path(tmp)
    whole = picture(1)
    strips = [whole[:, :480], whole[:, 480:960], whole[:, 960:]]

    # Shuffled JPEG strips plus a smaller copy of the whole picture: joined losslessly, the join is kept.
    small = np.asarray(Image.fromarray(whole).resize((720, 480)))
    pages, paths = work(tmp, "a", [strips[2], strips[0], small, strips[1]], "jpg")
    plan = stitch_strips.plan_work("x:a", pages, paths, tmp)
    [group] = plan["groups"]
    assert group["strips"] == ["a/01.jpg", "a/03.jpg", "a/00.jpg"] and group["overlaps"] == [0, 0], group
    assert group["full"] == "a/02.jpg" and group["keep"] == "stitched" and group["similar"] == [], group
    assert group["stitched"].endswith(".jpg") and plan["new_pages"] == [{"stitched": group["stitched"]}]
    decoded = [stitch_strips.blocky(paths[k]) for k in group["strips"]]
    assert np.array_equal(stitch_strips.blocky(group["stitched"]), np.concatenate(decoded, 1))

    # A recoloured, cropped copy is a similar picture, kept beside the join; an unrelated page is neither.
    variant = np.clip(whole[60:-60, 90:-90].astype(np.int16) * 0.8 + 30, 0, 255).astype(np.uint8)
    pages, paths = work(tmp, "h", [strips[0], strips[1], strips[2], variant, picture(7)], "png")
    plan = stitch_strips.plan_work("x:h", pages, paths, tmp)
    [group] = plan["groups"]
    assert group["full"] is None and [x["r2_key"] for x in group["similar"]] == ["h/03.png"], group
    assert plan["new_pages"] == [{"stitched": group["stitched"]}, {"r2_key": "h/03.png"}, {"r2_key": "h/04.png"}]

    # PNG strips repeating 3 columns: the repeat is dropped and the original comes back exactly.
    pages, paths = work(tmp, "b", [whole[:, :700], whole[:, 697:]], "png")
    [group] = stitch_strips.plan_work("x:b", pages, paths, tmp)["groups"]
    assert group["overlaps"] == [3] and group["full"] is None, group
    assert np.array_equal(np.asarray(Image.open(group["stitched"]).convert("RGB")), whole)

    # Stacked strips join top to bottom.
    pages, paths = work(tmp, "c", [whole[:500], whole[500:]], "png")
    [group] = stitch_strips.plan_work("x:c", pages, paths, tmp)["groups"]
    assert group["axis"] == "y" and np.array_equal(np.asarray(Image.open(group["stitched"]).convert("RGB")), whole)

    # Strips that lost columns at the cut, and unrelated pictures of one height, are not joined.
    pages, paths = work(tmp, "d", [whole[:, :700], whole[:, 706:]], "png")
    assert not (stitch_strips.plan_work("x:d", pages, paths, tmp) or {}).get("groups")
    pages, paths = work(tmp, "e", [picture(2, 720), picture(3, 720), picture(4, 720)], "png")
    assert not (stitch_strips.plan_work("x:e", pages, paths, tmp) or {}).get("groups")

    # Different pictures in one letterbox frame line up at every edge but are not one picture.
    framed = [picture(n, 480) for n in (5, 6)]
    for f in framed:
        f[:, :40], f[:, -40:] = 255, 255
        f[:200], f[-200:] = 0, 0
    pages, paths = work(tmp, "f", framed, "png")
    assert not (stitch_strips.plan_work("x:f", pages, paths, tmp) or {}).get("groups")
    # Frames in shared bars whose backgrounds differ jump in brightness at the would-be seam.
    framed = [np.full((960, 480, 3), shade, np.uint8) for shade in (220, 40)]
    for f in framed:
        f[:200], f[-200:] = 0, 0
        f[400:420] = 128
    pages, paths = work(tmp, "g", framed, "png")
    assert not (stitch_strips.plan_work("x:g", pages, paths, tmp) or {}).get("groups")

print("stitch_strips ok")
