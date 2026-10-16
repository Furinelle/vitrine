"""Offline check of tools/tag_merge.py on SQLite with every migration applied.

Seeds case, width, invisible and Traditional variants (including a work that
carries two variants and a hidden work), runs the generated SQL and checks the
merged names, counts, tag_counts and tag_keys; a second run must merge nothing.
Run with: python3 tools/test_tag_merge.py
"""
import pathlib
import sqlite3
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import tag_merge  # noqa: E402

ROOT = tag_merge.ROOT
SEED = {
    "w1": (None, ["鳴潮", "GenshinImpact"]),
    "w2": (None, ["鳴潮", "genshinimpact", "GenshinImpactㅤ"]),  # two variants on one work
    "w3": (None, ["鸣潮", "Genshin_Impact", "digitalartㅤ"]),
    "w4": ("2026-10-01", ["鸣潮", "鸣潮", "genshinimpact"]),  # hidden work still has its tags moved
    "w5": (None, ["原神", "原神芙宁娜", "銀狼"]),  # containment is not a duplicate
    "w6": (None, ["银狼"]),  # tie on works: Simplified wins
}


def rows(db):
    return [{"name": n, "visible": v, "total": t} for n, v, t in db.execute(tag_merge.ROWS_SQL)]


def run(db):
    data = rows(db)
    merges, targets, report = tag_merge.plan(data, tag_merge.keys_for([r["name"] for r in data]))
    db.executescript(tag_merge.merge_sql(merges, targets))
    return merges


def main():
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

    merges = run(db)
    # 鳴潮 has more visible works than 鸣潮; genshinimpact ties GenshinImpact on visible works
    # and wins on all works (the hidden w4 counts there).
    assert merges == {"鸣潮": "鳴潮", "GenshinImpact": "genshinimpact", "GenshinImpact\u3164": "genshinimpact",
                      "Genshin_Impact": "genshinimpact", "digitalart\u3164": "digitalart", "銀狼": "银狼"}, merges
    tags = dict(db.execute("SELECT name, use_count FROM tags WHERE name IN ('鳴潮','genshinimpact','digitalart','银狼')"))
    assert tags == {"鳴潮": 4, "genshinimpact": 4, "digitalart": 1, "银狼": 2}, tags
    assert {n for (n,) in db.execute("SELECT name FROM tags")} == {"鳴潮", "genshinimpact", "digitalart", "原神", "原神芙宁娜", "银狼"}
    pairs = set(db.execute("SELECT work_id, tag FROM work_tags"))
    assert ("w2", "genshinimpact") in pairs and ("w4", "鳴潮") in pairs and len(pairs) == 13, pairs
    truth = dict(db.execute("SELECT wt.tag, COUNT(*) FROM work_tags wt JOIN works w ON w.id=wt.work_id "
                            "WHERE w.deleted_at IS NULL GROUP BY wt.tag"))
    assert dict(db.execute("SELECT tag, cnt FROM tag_counts")) == truth, truth
    keys = dict(db.execute("SELECT key, tag FROM tag_keys"))
    assert keys["鸣潮"] == "鳴潮" and keys["genshinimpact"] == "genshinimpact" and len(keys) == 6, keys
    assert not db.execute("SELECT name FROM sqlite_master WHERE name='tag_merge'").fetchall()

    assert run(db) == {}
    print("PASS: tag_merge folds case/width/invisible/Traditional variants, keeps counts and tag_counts exact, "
          "keys every tag, and is idempotent")


if __name__ == "__main__":
    main()
