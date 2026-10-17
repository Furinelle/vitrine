"""Offline check of tools/tag_curate.py: decisions are re-checked against the database, the SQL they
produce runs on SQLite with every migration applied, and the rehearsal accepts the result.
Run with: python3 tools/test_tag_curate.py
"""
import json
import pathlib
import sqlite3
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import tag_curate  # noqa: E402
import tag_merge  # noqa: E402

SEED = {  # work: tags
    "w1": ["原神", "芙宁娜", "Furinaa"],
    "w2": ["原神", "芙宁娜", "新角色"],
    "w3": ["原神", "画师某某", "新版本"],
    "w4": ["原神", "热门杂项"], "w5": ["热门杂项"], "w6": ["热门杂项"],
    "w7": ["新组合", "芙宁娜"],
}
LIBRARY = [("原神", "work", None, None, "GenshinImpact"), ("芙宁娜", "character", "原神", None, "Furina")]
DECISIONS = [
    {"tag": "Furinaa", "action": "merge", "target": "芙宁娜"},
    {"tag": "新角色", "action": "item", "name": "新角色", "kind": "character", "parent": "原神", "en": "NewOne", "ja": "Furina"},
    {"tag": "新版本", "action": "item", "name": "新版本名", "kind": "lore", "parent": "原神"},
    {"tag": "画师某某", "action": "drop"},
    {"tag": "热门杂项", "action": "drop"},                                      # on 3 works: refused
    {"tag": "新组合", "action": "item", "kind": "cp", "members": ["芙宁娜", "不存在"]},  # unknown member: refused
    {"tag": "芙宁娜", "action": "item", "kind": "character", "parent": "芙宁娜"},       # already filed: refused
    {"tag": "原神", "action": "merge", "target": "原神"},                          # filed and self: refused
]


def database():
    db = sqlite3.connect(":memory:")
    for path in sorted((tag_merge.ROOT / "migrations").glob("*.sql")):
        db.executescript(path.read_text())
    for work, tags in SEED.items():
        db.execute("INSERT INTO works (id, source, source_id, created_at) VALUES (?, 'x', ?, 't')", (work, work))
        for tag in tags:
            db.execute("INSERT OR IGNORE INTO tags (name) VALUES (?)", (tag,))
            db.execute("INSERT INTO work_tags (work_id, tag) VALUES (?, ?)", (work, tag))
    db.executemany("INSERT INTO tag_items (name, kind, parent, ja, en) VALUES (?, ?, ?, ?, ?)", LIBRARY)
    names = [n for (n,) in db.execute("SELECT name FROM tags")] + ["新版本名", "NewOne"]
    keys = tag_merge.keys_for(sorted(set(names)))
    db.executemany("INSERT OR IGNORE INTO tag_keys (key, tag) VALUES (?, ?)", [(keys[n][0], n) for n in names if n in SEED_TAGS])
    db.execute("INSERT OR IGNORE INTO tag_keys (key, tag) VALUES ('furina', '芙宁娜')")
    db.execute("INSERT INTO tag_links SELECT tag, also FROM tag_closure")
    return db


SEED_TAGS = {t for tags in SEED.values() for t in tags}


def main():
    db = database()
    counts = dict(db.execute("SELECT tag, cnt FROM tag_counts WHERE cnt > 0"))
    library = {n: {"kind": k, **({"parent": p} if p else {})} for n, k, p, _, _ in LIBRARY}
    spellings = {s for d in DECISIONS for s in (d.get("tag"), d.get("target"), d.get("name"), d.get("ja"), d.get("en")) if s}
    keys = tag_merge.keys_for(sorted(spellings | set(counts) | set(library)))
    taken = dict(db.execute("SELECT key, tag FROM tag_keys"))
    unsorted = {t for t in counts if t not in library}
    statements, applied, refused = tag_curate.plan([d for d in DECISIONS if d["tag"] in unsorted or d["tag"] in ("芙宁娜", "原神")],
                                                   {t: c for t, c in counts.items()}, library, set(), keys, taken)
    assert sorted(d["tag"] for d in applied) == ["Furinaa", "新版本", "新角色", "画师某某"], applied
    reasons = {d["tag"]: d["refused"] for d in refused}
    assert set(reasons) == {"热门杂项", "新组合", "芙宁娜", "原神"}, reasons
    assert "3 件" in reasons["热门杂项"]
    new_one = next(d for d in applied if d["tag"] == "新角色")
    assert new_one["ja"] is None and "芙宁娜" in new_one["note"], new_one  # "Furina" already spells 芙宁娜

    # The SQL runs; the rehearsal on a dump of the database accepts it.
    with tempfile.TemporaryDirectory() as tmp:
        dump, sql = pathlib.Path(tmp) / "backup.sql", pathlib.Path(tmp) / "apply.sql"
        dump.write_text("\n".join(db.iterdump()))
        sql.write_text("\n".join(statements))
        assert tag_curate.rehearse(dump, sql) == 2  # 热门杂项 and 新组合 stay unsorted
    db.executescript("\n".join(statements))
    tags = lambda work: sorted(t for (t,) in db.execute("SELECT tag FROM work_tags WHERE work_id = ?", (work,)))  # noqa: E731
    assert tags("w1") == ["原神", "芙宁娜"], tags("w1")
    assert tags("w3") == ["原神", "新版本名"], tags("w3")
    items = {r[0]: r[1:] for r in db.execute("SELECT name, kind, parent, ja, en FROM tag_items")}
    assert items["新角色"] == ("character", "原神", None, "NewOne") and items["新版本名"] == ("lore", "原神", None, None)
    assert "新版本" not in items and db.execute("SELECT 1 FROM tag_blocks WHERE name = '画师某某'").fetchone()
    keys_now = dict(db.execute("SELECT key, tag FROM tag_keys"))
    assert keys_now[keys["Furinaa"][0]] == "芙宁娜" and keys_now[keys["新版本"][0]] == "新版本名"
    assert keys_now[keys["NewOne"][0]] == "新角色"
    # Later writes follow the new library: the old spelling lands on the merged tag and brings the work.
    db.execute("INSERT INTO works (id, source, source_id, created_at) VALUES ('w8', 'x', 'w8', 't')")
    db.execute("INSERT INTO work_tags (work_id, tag) VALUES ('w8', '新角色')")
    assert tags("w8") == ["原神", "新角色"]
    print("PASS: curation re-checks each decision (drops of tags in use, unknown members, filed tags, taken "
          "display names), renames, merges, files and blocks through the admin page's SQL, and rehearses it")


if __name__ == "__main__":
    main()
