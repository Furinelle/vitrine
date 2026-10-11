"""SQLite query-plan/cost regression using production SQL and disposable fixtures."""
from pathlib import Path
import json
import re
import sqlite3

ROOT = Path(__file__).resolve().parents[1]


def measured(db, sql, params=()):
    steps = 0

    def step():
        nonlocal steps
        steps += 1
        return 0

    db.set_progress_handler(step, 1)
    try:
        rows = db.execute(sql, params).fetchall()
    finally:
        db.set_progress_handler(None, 0)
    return rows, steps


def main():
    source = (ROOT / 'src/similar.rs').read_text()
    snapshot = source.split('pub async fn handle_snapshot', 1)[1].split('pub async fn handle_input', 1)[0]
    snapshot_sql = re.search(r'prepare\(r#"(.*?)"#\)', snapshot, re.S)[1]
    assert 'COUNT(' not in snapshot_sql.upper(), 'Default snapshot must only read singleton metadata'
    inputs = source.split('pub async fn handle_input', 1)[1].split('pub async fn handle_publish', 1)[0]
    queries = re.findall(r'prepare\(r#"(.*?)"#\)', inputs, re.S)
    assert len(queries) == 2
    db = sqlite3.connect(':memory:')
    for migration in sorted((ROOT / 'migrations').glob('*.sql')):
        db.executescript(migration.read_text())
    size = 30000
    db.executemany("INSERT INTO works (id,source,source_id,title,created_at,deleted_at) VALUES (?, 'pixiv', ?, '', '', ?)",
                   ((str(i), str(i), '' if i % 17 == 0 else None) for i in range(size)))
    db.executemany("INSERT INTO images (id,work_id,r2_key,created_at) VALUES (?,?,?, '')",
                   ((str(i), str(i), f'pixiv/{i:05d}/0.png') for i in range(size)))
    db.executemany("INSERT INTO image_fingerprints (r2_key,work_id,content_sha256,strict_key,average_hash,difference_hash,color_key,detail_key,width,height,bytes,format,updated_at) VALUES (?,?, '', '', '', '', '', '', 1,1,1,'png','')",
                   ((f'pixiv/{i:05d}/0.png', str(i)) for i in range(size)))
    db.executemany("INSERT INTO image_clip (r2_key,dim,embedding,updated_at) VALUES (?,512,'','')",
                   ((f'pixiv/{i:05d}/0.png',) for i in range(size)))
    db.execute('ANALYZE')
    _, snapshot_steps = measured(db, snapshot_sql)
    assert snapshot_steps < 100, snapshot_steps
    for sql, table, alias in zip(queries, ('image_fingerprints', 'image_clip'), ('f', 'c')):
        assert 'OFFSET' not in sql.upper()
        after, limit = 'pixiv/28000/0.png', 100
        rows, steps = measured(db, sql, (after, limit))
        keys = [row[0] for row in rows]
        expected = [f'pixiv/{i:05d}/0.png' for i in range(28001, size) if i % 17][:limit]
        assert keys == expected, (table, keys[:2])
        plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + sql, (after, limit))]
        assert any(f'SEARCH {alias} USING INDEX sqlite_autoindex_{table}_1 (r2_key>?)' in line for line in plan), plan
        assert not any('SCAN' in line or 'TEMP B-TREE' in line for line in plan), plan
        assert steps < 10000, (table, steps)
        # The old OFFSET path visits the prefix again for every later page.
        old_sql = sql.replace(f'{alias}.r2_key > ? AND ', '').replace('LIMIT ?', 'LIMIT ? OFFSET ?')
        _, offset_steps = measured(db, old_sql, (limit, 26000))
        assert offset_steps > steps * 100, (table, steps, offset_steps)
        cursor, exported = '', []
        while True:
            page, _ = measured(db, sql, (cursor, 100))
            exported.extend(row[0] for row in page)
            if len(page) < 100:
                break
            cursor = page[-1][0]
        assert exported == [f'pixiv/{i:05d}/0.png' for i in range(size) if i % 17]
        print(f'PASS: {table} primary-key range; deep-page VM steps {steps} vs OFFSET {offset_steps}; all live rows exported once')
    print(f'PASS: default snapshot VM steps {snapshot_steps} at {size} images')
    review = source.split('async fn review_result', 1)[1].split('fn revalidate_candidates', 1)[0]
    review_sql = re.search(r'prepare\(\s*r#"(.*?)"#', review, re.S)[1]
    selected = ['pixiv/29001/0.png', 'pixiv/29003/0.png']
    rows, steps = measured(db, review_sql, (json.dumps(selected),))
    assert {row[0] for row in rows} == set(selected)
    plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + review_sql, (json.dumps(selected),))]
    assert any('SEARCH f USING INDEX' in line and 'r2_key=?' in line for line in plan), plan
    assert any('SEARCH i USING INDEX' in line and 'r2_key=?' in line for line in plan), plan
    assert steps < 200, (steps, plan)
    print(f'PASS: candidate validation uses keyed lookups; {steps} VM steps for two keys in {size} images')

    review_source = (ROOT / 'src/image_review.rs').read_text()
    guard = re.search(r'prepare\("(UPDATE catalog_image_reviews SET state=CASE WHEN \(SELECT generation[^"\n]+)"\)', review_source)[1]
    db.execute("INSERT INTO catalog_image_reviews(decision_id,payload,state,created_at) VALUES ('review-test','{}','prepared','')")
    db.commit()
    def generation():
        return db.execute('SELECT generation FROM similar_generation WHERE id=1').fetchone()[0]

    def guarded_delete(key, expected):
        with db:
            db.execute(guard, (expected, 'review-test'))
            db.execute('DELETE FROM images WHERE r2_key=?', (key,))
            return generation()

    before = generation()
    after_first = guarded_delete(selected[0], before)
    assert after_first > before
    after_second = guarded_delete(selected[1], after_first)
    assert after_second > after_first, 'A batch must advance using its own committed generation'
    target = 'pixiv/29004/0.png'
    db.execute('UPDATE image_clip SET embedding=? WHERE r2_key=?', ('changed', target))
    db.commit()
    try:
        guarded_delete(target, after_second)
    except sqlite3.IntegrityError:
        pass
    else:
        raise AssertionError('A feature update after validation allowed deletion')
    assert db.execute('SELECT 1 FROM images WHERE r2_key=?', (target,)).fetchone()
    assert db.execute("SELECT state FROM catalog_image_reviews WHERE decision_id='review-test'").fetchone()[0] == 'prepared'
    print('PASS: same-transaction generation advances the batch; concurrent feature updates reject and roll back')


if __name__ == '__main__':
    main()
