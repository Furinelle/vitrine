"""Crop matching on synthetic pictures. Needs numpy and Pillow (the Oracle CLIP venv);
skipped elsewhere."""

import importlib.util
import tempfile
import unittest
from pathlib import Path

try:
    import numpy as np
    from PIL import Image, ImageFilter
except ImportError:
    np = None

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(np, "numpy and Pillow are installed only in the CLIP virtualenv")
class CropMatchTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("crop_match", ROOT / "tools/crop_match.py")
        self.match = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.match)
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.addCleanup(self.directory.cleanup)

    def picture(self, seed, size):
        noise = np.random.default_rng(seed).random((12, 16, 3)) * 255
        image = Image.fromarray(noise.astype("uint8")).resize(size, Image.BICUBIC)
        return image.filter(ImageFilter.GaussianBlur(4))

    def cached(self, name, image):
        path = self.root / f"{name}.png"
        image.convert("L").resize(
            (round(image.width * 192 / max(image.size)), round(image.height * 192 / max(image.size)))
        ).save(path)
        return path

    def test_a_rescaled_crop_is_found_and_an_unrelated_picture_is_not(self):
        full = self.picture(1, (1600, 1200))
        crop = full.crop((500, 300, 1100, 1100)).resize((450, 600))
        full_path, crop_path = self.cached("full", full), self.cached("crop", crop)
        other_path = self.cached("other", self.picture(2, (600, 800)))
        score, cover = self.match.best_placement(crop_path, full_path)
        self.assertGreater(score, 0.9)
        self.assertAlmostEqual(cover, 600 * 800 / (1600 * 1200), delta=0.05)
        self.assertLess(self.match.best_placement(other_path, full_path)[0], 0.6)
        # verify() names the crop and the source whichever way the pair is ordered.
        _, _, (score, cover), crop_key, full_key = self.match.verify(("f", "c", full_path, crop_path))
        self.assertEqual((crop_key, full_key), ("c", "f"))
        self.assertTrue(self.match.is_crop(score, cover))
        # A small template needs the higher score; a large one passes from the lower line.
        self.assertFalse(self.match.is_crop(0.85, 0.05))
        self.assertTrue(self.match.is_crop(0.85, 0.2))

    def test_candidates_need_close_embeddings_and_different_shapes(self):
        def unit(x):
            vector = np.zeros(512, dtype="<f4")
            vector[0], vector[1] = x, np.sqrt(1 - x * x)
            return vector.tobytes().hex()

        rows = [
            ("full", "x:1", 2000, 1000, 1.0),
            ("crop", "x:2", 400, 600, 0.95),   # other work, close, different shape
            ("page", "x:1", 300, 300, 0.80),   # same work, looser line
            ("far", "x:3", 300, 300, 0.80),    # other work below the cross-work line
            ("twin", "x:4", 2000, 1000, 1.0),  # same shape and size: a duplicate, not a crop
        ]
        fingerprints = [{"r2_key": k, "work_id": w, "width": wd, "height": h} for k, w, wd, h, _ in rows]
        clip = [{"r2_key": k, "embedding": unit(x)} for k, _, _, _, x in rows]
        pairs = {tuple(sorted(pair)) for pair in self.match.candidates(fingerprints, clip)}
        self.assertIn(("crop", "full"), pairs)
        self.assertIn(("full", "page"), pairs)
        self.assertNotIn(("far", "full"), pairs)
        self.assertNotIn(("full", "twin"), pairs)


if __name__ == "__main__":
    unittest.main()
