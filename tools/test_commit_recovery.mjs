// Exercise the actual WASM Worker with local D1/R2 and post-commit fault injection.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Miniflare, convertV4MiniflareOptions } from 'miniflare';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const temp = await mkdtemp(path.join(tmpdir(), 'vitrine-commit-recovery-'));
const token = 'disposable-local-test';
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a2ioAAAAASUVORK5CYII=', 'base64');
const wrapper = `
import Vitrine from './index.js';
export default { async fetch(request, env, ctx) {
  const mode = request.headers.get('x-test-d1-fault');
  let attempted = false;
  const database = new Proxy(env.DB, { get(target, property) {
    if (property === 'batch' && mode) return async statements => {
      attempted = true;
      if (mode === 'before-commit' || mode === 'receipt-read-failure') {
        throw new Error('test: result unknown before commit');
      }
      await target.batch(statements);
      if (mode === 'wrong-audit') {
        await target.prepare("UPDATE ingest_audits SET fingerprint='different-attempt' WHERE source_id=?")
          .bind(request.headers.get('x-test-source-id')).run();
      }
      throw new Error(mode === 'constraint-message' ? 'UNIQUE constraint failed: test lost confirmation' : 'test: confirmation lost after commit');
    };
    if (property === 'prepare' && attempted) return sql => {
      if ((mode === 'audit-read-failure' && sql.includes('FROM ingest_audits')) ||
          (mode === 'receipt-read-failure' && sql.includes('FROM idempotency_receipts'))) {
        const failRead = statement => new Proxy(statement, { get(prepared, method) {
          if (method === 'first') return async () => { throw new Error('test: reconciliation unavailable'); };
          if (method === 'bind') return (...args) => failRead(prepared.bind(...args));
          const value = Reflect.get(prepared, method, prepared);
          return typeof value === 'function' && method !== 'constructor' ? value.bind(prepared) : value;
        }});
        return failRead(target.prepare(sql));
      }
      return target.prepare(sql);
    };
    const value = Reflect.get(target, property, target);
    return typeof value === 'function' && property !== 'constructor' ? value.bind(target) : value;
  }});
  return Vitrine.prototype.fetch.call({ env: { ...env, DB: database }, ctx }, request);
}};`;
const mf = new Miniflare(convertV4MiniflareOptions({
  modulesRoot: temp,
  modules: [
    { type: 'ESModule', path: path.join(temp, 'wrapper.mjs'), contents: wrapper },
    { type: 'ESModule', path: path.join(temp, 'index.js'), contents: await readFile(path.join(root, 'build/index.js'), 'utf8') },
    { type: 'CompiledWasm', path: path.join(temp, 'index_bg.wasm'), contents: await readFile(path.join(root, 'build/index_bg.wasm')) },
  ],
  compatibilityDate: '2026-07-01',
  d1Databases: { DB: 'local-commit-recovery' },
  r2Buckets: { MEDIA: 'local-commit-recovery' },
  bindings: { INGEST_TOKEN: token, TELEGRAM_BOT_TOKEN: '' },
  resourcePersistencePath: path.join(temp, 'state'),
  telemetry: { enabled: false },
  outboundService: () => new Response('External network disabled', { status: 403 }),
}));

try {
  const db = await mf.getD1Database('DB');
  const bucket = await mf.getR2Bucket('MEDIA');
  // SQLite's parser keeps multi-line triggers together; no project config/secrets.
  const statements = JSON.parse(execFileSync('python3', ['-c', `
import json, pathlib, sqlite3, sys
statements = []
for migration in sorted(pathlib.Path(sys.argv[1]).glob('*.sql')):
    pending = ''
    for line in migration.read_text().splitlines(True):
        pending += line
        if sqlite3.complete_statement(pending):
            statements.append(pending)
            pending = ''
    assert not pending.strip(), migration
print(json.dumps(statements))
`, path.join(root, 'migrations')], { encoding: 'utf8' }));
  for (const sql of statements) await db.prepare(sql).run();

  async function ingest(sourceId, key = sourceId, fault = '') {
    const form = new FormData();
    form.set('meta', JSON.stringify({ source: 'pixiv', source_id: sourceId, title: 'Local recovery test' }));
    form.append('files', new Blob([png], { type: 'image/png' }), 'test.png');
    const encoded = new Request('http://local.test/api/ingest', { method: 'POST', body: form });
    const response = await mf.dispatchFetch(encoded.url, {
      method: 'POST', body: await encoded.arrayBuffer(),
      headers: {
        Authorization: `Bearer ${token}`, 'Content-Type': encoded.headers.get('Content-Type'),
        ...(key ? { 'Idempotency-Key': key } : {}),
        'x-test-d1-fault': fault, 'x-test-source-id': sourceId,
      },
    });
    return { status: response.status, body: await response.json() };
  }
  async function assertMedia(key) {
    const response = await mf.dispatchFetch(`http://local.test/media/${key}`);
    assert.equal(response.status, 200);
    assert.deepEqual(Buffer.from(await response.arrayBuffer()), png);
  }
  async function imageCount(sourceId) {
    return db.prepare('SELECT COUNT(*) AS n FROM images WHERE work_id=?').bind(`pixiv:${sourceId}`).first('n');
  }
  async function keys(sourceId) {
    return (await bucket.list({ prefix: `pixiv/${sourceId}/` })).objects.map(object => object.key);
  }

  for (const [id, mode, key] of [
    ['101', 'lost-confirmation', '101'],
    ['102', 'constraint-message', '102'],
    ['103', 'lost-confirmation', ''],
  ]) {
    const first = await ingest(id, key, mode);
    assert.equal(first.status, 200, JSON.stringify(first));
    assert.equal(first.body.ok, true);
    assert.equal(await imageCount(id), 1);
    await assertMedia(first.body.images[0].r2_key);
    if (key) {
      const retry = await ingest(id, key);
      assert.equal(retry.status, 200);
      assert.equal(retry.body.idempotent, true);
      assert.deepEqual(retry.body.images, first.body.images);
      await assertMedia(retry.body.images[0].r2_key);
    }
  }

  // A failed reconciliation read must never delete already committed media.
  assert.equal((await ingest('104', '104', 'audit-read-failure')).status, 500);
  const recovered = await ingest('104');
  assert.equal(recovered.status, 200);
  assert.equal(recovered.body.idempotent, true);
  await assertMedia(recovered.body.images[0].r2_key);

  // A success row for a different attempt cannot certify this no-key request.
  assert.equal((await ingest('105', '', 'wrong-audit')).status, 500);
  assert.equal(await imageCount('105'), 1);
  await assertMedia((await keys('105'))[0]);

  for (const [id, mode] of [['106', 'before-commit'], ['107', 'receipt-read-failure']]) {
    assert.equal((await ingest(id, id, mode)).status, 500);
    assert.equal(await imageCount(id), 0);
    assert.equal((await keys(id)).length, 1); // uncertain orphan is retained
    const retry = await ingest(id);
    assert.equal(retry.status, 200);
    assert.equal((await keys(id)).length, 2);
    await assertMedia(retry.body.images[0].r2_key);
  }

  // A real SQLite abort rolls back the whole D1 batch, including its receipt.
  await db.prepare(`CREATE TRIGGER fail_test_ingest BEFORE INSERT ON ingest_audits
    WHEN NEW.source_id='108' BEGIN SELECT RAISE(ABORT, 'test actual rollback'); END`).run();
  assert.equal((await ingest('108')).status, 500);
  assert.equal(await imageCount('108'), 0);
  assert.equal(await db.prepare("SELECT COUNT(*) AS n FROM works WHERE id='pixiv:108'").first('n'), 0);
  assert.equal(await db.prepare("SELECT COUNT(*) AS n FROM idempotency_receipts WHERE idempotency_key='108'").first('n'), 0);
  assert.equal((await keys('108')).length, 1);
  await db.prepare('DROP TRIGGER fail_test_ingest').run();
  const retried = await ingest('108');
  assert.equal(retried.status, 200);
  await assertMedia(retried.body.images[0].r2_key);

  const keep = (await ingest('201')).body.images[0].r2_key;
  const remove = (await ingest('202')).body.images[0].r2_key;
  const other = (await ingest('203')).body.images[0].r2_key;
  async function prune(keepKey, removeKeys, decision = 'local-decision') {
    const response = await mf.dispatchFetch('http://local.test/api/catalog/prune', {
      method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ decision_id: decision, keep_r2_key: keepKey, remove_r2_keys: removeKeys }),
    });
    return { status: response.status, body: await response.json() };
  }
  assert.equal((await prune(keep, [remove])).status, 200);
  // Put a sentinel at the old key to prove conflicts have no R2 delete side effect.
  await bucket.put(remove, png);
  for (const conflict of [await prune(keep, [other]), await prune(other, [remove])]) {
    assert.equal(conflict.status, 409);
    assert.equal(conflict.body.ok, false);
    assert.ok(await bucket.get(remove));
    assert.equal(await imageCount('203'), 1);
    await assertMedia(other);
  }
  const replay = await prune(keep, [remove]);
  assert.equal(replay.status, 200);
  assert.equal(replay.body.replayed, true);
  assert.equal(await bucket.get(remove), null);
  await bucket.put(remove, png);
  await db.prepare("UPDATE catalog_prune_receipts SET removed_json='not JSON' WHERE decision_id='local-decision'").run();
  assert.equal((await prune(keep, [remove])).status, 500);
  assert.ok(await bucket.get(remove));
  const extra = (await ingest('204')).body.images[0].r2_key;
  assert.equal((await prune(keep, [other, extra], 'unordered-plan')).status, 200);
  assert.equal((await prune(keep, [extra, other], 'unordered-plan')).body.replayed, true);
  console.log('PASS: commit confirmation loss, safe failed reads/rollback/retry, audit identity, prune plan replay/conflict');
} finally {
  await mf.dispose();
  await rm(temp, { recursive: true, force: true });
}
