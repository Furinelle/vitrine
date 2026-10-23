"""Offline check that the 0015 triggers keep tag_counts equal to the joined count.

Applies every migration to an in-memory SQLite database (foreign keys on, as in
D1) and replays the write shapes used by ingest, admin edits, soft delete and
restore. Run with: python3 tools/test_tag_counts.py
"""
import pathlib
import sqlite3

ROOT = pathlib.Path(__file__).resolve().parent.parent
TRUTH = """
    SELECT wt.tag, COUNT(*) FROM work_tags wt JOIN works w ON w.id = wt.work_id
    WHERE w.deleted_at IS NULL GROUP BY wt.tag
"""


def check(db, step):
    expected = dict(db.execute(TRUTH).fetchall())
    actual = dict(db.execute("SELECT tag, cnt FROM tag_counts WHERE cnt > 0").fetchall())
    assert actual == expected, (step, actual, expected)
    assert not db.execute("SELECT 1 FROM tag_counts WHERE cnt < 0").fetchall(), step


def ingest(db, work_id, tags):
    """Same statement order as ingest.rs: upsert work, replace tags."""
    db.execute(
        """INSERT INTO works (id, source, source_id, source_url, title, author_name, author_url,
             page_count, origin, created_at, deleted_at)
           VALUES (?, 'x', ?, '', '', '', '', 1, 'test', '2026-10-16', NULL)
           ON CONFLICT(id) DO UPDATE SET review_version=works.review_version+1,
             deleted_at=CASE WHEN works.deleted_at IS NOT NULL
               AND EXISTS(SELECT 1 FROM images WHERE work_id=works.id)
               THEN works.deleted_at ELSE NULL END""",
        (work_id, work_id),
    )
    db.execute("DELETE FROM work_tags WHERE work_id = ?", (work_id,))
    for tag in tags:
        db.execute("INSERT OR IGNORE INTO work_tags (work_id, tag) VALUES (?, ?)", (work_id, tag))


def main():
    db = sqlite3.connect(":memory:")
    db.execute("PRAGMA foreign_keys = ON")
    migrations = sorted((ROOT / "migrations").glob("*.sql"))
    for path in migrations:
        if path.name == "0015_tag_counts.sql":
            # Existing rows must be backfilled by the migration itself.
            ingest(db, "x:old", ["a", "b"])
            ingest(db, "x:gone", ["a"])
            db.execute("UPDATE works SET deleted_at='t' WHERE id='x:gone'")
        db.executescript(path.read_text())
    check(db, "backfill")
    assert dict(db.execute("SELECT tag, cnt FROM tag_counts").fetchall()) == {"a": 1, "b": 1}

    ingest(db, "x:1", ["a", "c", "c"])  # duplicate tag is ignored, not double counted
    check(db, "ingest")
    ingest(db, "x:1", ["c", "d"])  # re-ingest replaces tags
    check(db, "re-ingest")

    # Admin tag edit (admin.rs): delete then re-insert only while the work is visible.
    db.execute("DELETE FROM work_tags WHERE work_id=? AND EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL)", ("x:1", "x:1"))
    db.execute("INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT ?, ? WHERE EXISTS(SELECT 1 FROM works WHERE id=? AND deleted_at IS NULL)", ("x:1", "e", "x:1"))
    check(db, "admin edit")

    # Soft delete, re-ingest while hidden (stays hidden when images exist), restore.
    db.execute("INSERT INTO images (id, work_id, page_index, r2_key, content_type, byte_size, created_at) VALUES ('x:1#0', 'x:1', 0, 'k', 'image/jpeg', 1, 't')")
    db.execute("UPDATE works SET deleted_at='t',review_version=review_version+1 WHERE id='x:1' AND deleted_at IS NULL")
    check(db, "soft delete")
    ingest(db, "x:1", ["f"])
    assert db.execute("SELECT deleted_at FROM works WHERE id='x:1'").fetchone()[0] == "t"
    check(db, "re-ingest hidden work")
    db.execute("UPDATE works SET deleted_at=NULL,review_version=review_version+1 WHERE id='x:1'")
    check(db, "restore")
    db.execute("UPDATE works SET title='renamed' WHERE id='x:1'")  # unrelated update
    check(db, "unrelated update")

    # Takedown (lib.rs): drop tags, then hide.
    db.execute("DELETE FROM work_tags WHERE work_id='x:old'")
    db.execute("UPDATE works SET deleted_at='t' WHERE id='x:old'")
    check(db, "takedown")

    # Direct edits by an operator.
    ingest(db, "x:2", ["g"])
    db.execute("UPDATE work_tags SET tag='h' WHERE work_id='x:2' AND tag='g'")
    check(db, "direct tag rename")
    db.execute("UPDATE work_tags SET work_id='x:gone' WHERE work_id='x:2'")
    check(db, "move tag to hidden work")
    db.execute("DELETE FROM works WHERE id='x:1'")  # hard delete cascades to work_tags
    check(db, "hard delete")

    print(f"PASS: tag_counts matches the joined count across {len(migrations)} migrations, "
          "ingest, admin edits, soft delete, restore, takedown and direct edits")


if __name__ == "__main__":
    main()
