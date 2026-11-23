#!/usr/bin/env python3
"""Write src/junk_prompts.json: CLIP text embeddings that score gallery images as junk.

The similarity binary compares each stored image embedding against these prompts
(softmax at CLIP's logit scale); the summed probability of the junk prompts is the
image's junk score. Run with the Oracle CLIP virtualenv after editing the prompts:

    /home/ubuntu/vitrine-clip/venv/bin/python junk_prompts.py > junk_prompts.json
"""

import json
import struct
import sys

# Must stay the model that produced image_clip (tools/clip_embed.py).
MODEL_NAME = "ViT-B-32"
PRETRAINED = "openai"

KEEP = [
    "an anime illustration",
    "a digital painting of a character",
    "fan art of a video game character",
    "a cute chibi drawing",
    "an anime character with food",
    "a manga comic page",
    "a pencil sketch of a character",
    "a 3D render of an anime character",
    "a collage of anime illustrations",
    "a cosplay photo",
    "an illustration with a title text",
    "a black and white manga panel",
    "an anime girl portrait",
    "a game character splash art",
]
# Prompt and the admin label shown for it.
JUNK = [
    ("a screenshot of a phone app user interface", "App 截图"),
    ("a screenshot of a settings menu", "设置界面"),
    ("a screenshot of a chat or social media post", "聊天或社交截图"),
    ("an image that is mostly text", "文字为主"),
    ("a tutorial image with text and screenshots", "教程图"),
    ("a completely black image", "全黑"),
    ("a blank white image", "空白"),
    ("a QR code", "二维码"),
    ("a spreadsheet or table", "表格"),
    ("an image of Chinese text", "中文文字图"),
    ("a meme with a caption", "配字梗图"),
    ("a screenshot of a video game with UI", "游戏截图"),
    ("a photo of real people", "真人照片"),
    ("a photo of a figure or plush toy", "手办或周边照片"),
    ("a watermark or logo on a plain background", "水印或标志"),
]


def main() -> int:
    import open_clip
    import torch

    model, _, _ = open_clip.create_model_and_transforms(MODEL_NAME, pretrained=PRETRAINED)
    model.eval()
    tokenizer = open_clip.get_tokenizer(MODEL_NAME)
    prompts = KEEP + [prompt for prompt, _ in JUNK]
    with torch.no_grad():
        text = model.encode_text(tokenizer(prompts))
        text = text / text.norm(dim=-1, keepdim=True)
    rows = [struct.pack("<" + "f" * len(row), *row.tolist()).hex() for row in text]
    json.dump(
        {
            "model": f"{MODEL_NAME}/{PRETRAINED}",
            "keep": [{"prompt": prompt, "embedding": row} for prompt, row in zip(KEEP, rows)],
            "junk": [
                {"prompt": prompt, "label": label, "embedding": row}
                for (prompt, label), row in zip(JUNK, rows[len(KEEP):])
            ],
        },
        sys.stdout,
        ensure_ascii=False,
        indent=1,
    )
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
