"""Offline check of tools/tag_merge.py on SQLite with every migration applied.

Seeds case, width, invisible and Traditional variants, display names of library
tags, a dropped tag and an AI tool tag; runs the generated SQL and checks the
merged names, tag_items, implied tags (character -> work, CP -> characters ->
work, tool -> AI生成), blocks on later writes, exact tag_counts, and that a second
run changes nothing.
Run with: python3 tools/test_tag_merge.py
"""
import json
import pathlib
import sqlite3
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import tag_merge  # noqa: E402

ROOT = tag_merge.ROOT
SEED = {
    "w1": (None, ["鳴潮", "GenshinImpact"]),
    "w2": (None, ["鳴潮", "genshinimpact", "GenshinImpactㅤ"]),  # two variants on one work
    "w3": (None, ["鸣潮", "Genshin_Impact", "digitalartㅤ"]),
    "w4": ("2026-10-01", ["鸣潮", "鸣潮", "genshinimpact"]),  # hidden work still has its tags moved
    "w5": (None, ["原神", "原神芙宁娜", "銀狼", "Miku"]),  # containment alone is not a duplicate
    "w6": (None, ["银狼", "WutheringWaves", "求流量", "NovelAI"]),  # display name, dropped tag, AI tool
}
LIBRARY = {
    "鸣潮": {"kind": "work", "en": "WutheringWaves", "en_aliases": ["WuWa"]},
    "原神": {"kind": "work"},
    "芙宁娜": {"kind": "character", "parent": "原神", "en": "Furina", "zh_aliases": ["原神芙宁娜"]},
    "芙卡洛斯": {"kind": "character", "parent": "原神"},
    "初音未来": {"kind": "character", "ja": "初音ミク", "en": "Miku"},
    "双芙": {"kind": "cp", "members": ["芙宁娜", "芙卡洛斯"]},
    "AI生成": {"kind": "meta", "en": "AIGenerated"},
    "NovelAI": {"kind": "medium", "parent": "AI生成"},
}
DROPS = ["求流量"]


def rows(db):
    return [{"name": n, "visible": v, "total": t} for n, v, t in db.execute(tag_merge.ROWS_SQL)]


def run(db, library):
    aliases, items, _ = tag_merge.load_library(library)
    data = rows(db)
    names = {r["name"] for r in data} | {s for k, v in aliases.items() for s in (k, *v)}
    merges, targets, _ = tag_merge.plan(data, tag_merge.keys_for(sorted(names)), aliases)
    db.executescript(tag_merge.merge_sql(merges, targets, DROPS, items))
    return merges


def counts_exact(db):
    truth = dict(db.execute("SELECT wt.tag, COUNT(*) FROM work_tags wt JOIN works w ON w.id=wt.work_id "
                            "WHERE w.deleted_at IS NULL GROUP BY wt.tag"))
    assert {t: c for t, c in db.execute("SELECT tag, cnt FROM tag_counts") if c} == truth, truth


def main():
    library = pathlib.Path(tempfile.mkdtemp()) / "library.json"
    library.write_text(json.dumps(LIBRARY, ensure_ascii=False))
    db = sqlite3.connect(":memory:")
    db.execute("PRAGMA foreign_keys = ON")
    for path in sorted((ROOT / "migrations").glob("*.sql")):
        db.executescript(path.read_text())
    for work, (deleted, tags) in SEED.items():
        db.execute("INSERT INTO works (id, source, source_id, created_at, deleted_at) VALUES (?, 'x', ?, 't', ?)",
                   (work, work, deleted))
        for tag in tags:
            db.execute("INSERT OR IGNORE INTO tags (name) VALUES (?)", (tag,))
            db.execute("INSERT OR IGNORE INTO work_tags (work_id, tag) VALUES (?, ?)", (work, tag))

    db.execute("INSERT INTO tag_blocks (name) VALUES ('银狼')")  # blocked by an earlier run, unblocked now
    merges = run(db, library)
    # Display names (WutheringWaves, Miku) and listed spellings fold into the stored Chinese name;
    # genshinimpact ties GenshinImpact on visible works and wins on all works (the hidden w4 counts there).
    assert merges == {"鳴潮": "鸣潮", "WutheringWaves": "鸣潮", "Miku": "初音未来", "原神芙宁娜": "芙宁娜",
                      "GenshinImpact": "genshinimpact", "GenshinImpactㅤ": "genshinimpact",
                      "Genshin_Impact": "genshinimpact", "digitalartㅤ": "digitalart", "銀狼": "银狼"}, merges
    pairs = set(db.execute("SELECT work_id, tag FROM work_tags"))
    assert pairs == {
        ("w1", "鸣潮"), ("w1", "genshinimpact"), ("w2", "鸣潮"), ("w2", "genshinimpact"),
        ("w3", "鸣潮"), ("w3", "genshinimpact"), ("w3", "digitalart"), ("w4", "鸣潮"), ("w4", "genshinimpact"),
        ("w5", "原神"), ("w5", "芙宁娜"), ("w5", "银狼"), ("w5", "初音未来"),
        ("w6", "银狼"), ("w6", "鸣潮"), ("w6", "NovelAI"), ("w6", "AI生成"),
    }, pairs
    assert dict(db.execute("SELECT name, use_count FROM tags WHERE name IN ('鸣潮','genshinimpact','AI生成')")) == {
        "鸣潮": 5, "genshinimpact": 4, "AI生成": 1}
    assert not db.execute("SELECT 1 FROM tags WHERE name IN ('求流量','WutheringWaves','Miku')").fetchall()
    items = {r[0]: r[1:] for r in db.execute("SELECT name, kind, parent, ja, en FROM tag_items")}
    assert db.execute("SELECT members FROM tag_items WHERE name='双芙'").fetchone() == ('["芙宁娜", "芙卡洛斯"]',)
    assert items["初音未来"] == ("character", None, "初音ミク", "Miku") and items["芙宁娜"] == ("character", "原神", None, "Furina")
    assert len(items) == len(LIBRARY)
    keys = dict(db.execute("SELECT key, tag FROM tag_keys"))
    assert keys["wutheringwaves"] == keys["wuwa"] == "鸣潮" and keys["miku"] == keys["初音ミク"] == "初音未来", keys
    assert keys["furina"] == "芙宁娜" and keys["aigenerated"] == "AI生成", keys
    counts_exact(db)
    assert not db.execute("SELECT name FROM sqlite_master WHERE name='tag_merge'").fetchall()

    # Later writes: a CP brings both characters and their work (one trigger level, closure rows),
    # an AI tool brings AI生成, a blocked tag is skipped; tag_counts stays exact.
    db.execute("INSERT INTO works (id, source, source_id, created_at) VALUES ('w7', 'x', 'w7', 't')")
    for tag in ("双芙", "求流量", "NovelAI"):
        db.execute("INSERT OR IGNORE INTO tags (name) VALUES (?)", (tag,))
        db.execute("INSERT OR IGNORE INTO work_tags (work_id, tag) VALUES ('w7', ?)", (tag,))
    assert {t for (t,) in db.execute("SELECT tag FROM work_tags WHERE work_id='w7'")} == {
        "双芙", "芙宁娜", "芙卡洛斯", "原神", "NovelAI", "AI生成"}
    assert not db.execute("SELECT 1 FROM tags WHERE name='求流量'").fetchall()
    counts_exact(db)

    before = set(db.execute("SELECT work_id, tag FROM work_tags"))
    assert run(db, library) == {}
    assert set(db.execute("SELECT work_id, tag FROM work_tags")) == before

    # Admin-page edits in the database flow back into the JSON files: changed fields, a merged-away
    # entry leaves, listed spellings stay, the drop list mirrors tag_blocks.
    db.execute("UPDATE tag_items SET en = 'Focalors', parent = '芙宁娜' WHERE name = '芙卡洛斯'")
    db.execute("DELETE FROM tag_items WHERE name = '初音未来'")
    db.execute("INSERT INTO tag_blocks (name) VALUES ('后台屏蔽')")
    db.row_factory = sqlite3.Row
    exported = [dict(r) for r in db.execute(tag_merge.EXPORT_SQL)]
    lib, fixes = tag_merge.export(exported, LIBRARY, {"fixes": {}, "keep": [], "drop": DROPS})
    assert lib["芙卡洛斯"] == {"kind": "character", "parent": "芙宁娜", "en": "Focalors"}, lib["芙卡洛斯"]
    assert "初音未来" not in lib and lib["芙宁娜"]["zh_aliases"] == ["原神芙宁娜"]
    assert lib["双芙"]["members"] == ["芙宁娜", "芙卡洛斯"]
    assert fixes["drop"] == sorted(["求流量", "后台屏蔽"]), fixes
    print("PASS: tag_merge folds spellings and display names into the stored name, writes tag_items, implies "
          "work/CP/AI tags on old and new works, drops and blocks tags, keeps tag_counts exact, and is idempotent")


if __name__ == "__main__":
    main()
