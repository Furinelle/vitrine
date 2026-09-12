"""Local HTTP regressions; INGEST_TOKEN=local-image-review-test.

Run Wrangler on 18787 with migrations applied, then:
VITRINE_TEST_STATE=/path/to/wrangler-state python3 tools/test_image_review.py
Only fake local D1/R2 data is used. The state directory must match Wrangler's --persist-to.
"""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zlib

BASE = 'http://127.0.0.1:18787'
HEADERS = {'Authorization': 'Bearer local-image-review-test'}
STATE = os.environ.get('VITRINE_TEST_STATE', '.wrangler/state')
WRANGLER = ['node', 'node_modules/wrangler/bin/wrangler.js']
CHAT = -1001234567890


def fetch(path, payload=None, expected=200, headers=None, method=None, auth=True):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(BASE + path, data, method=method, headers={
        **(HEADERS if auth else {}), 'Content-Type': 'application/json', **(headers or {})})
    try:
        with urllib.request.urlopen(req) as response:
            code, body, response_headers = response.status, response.read(), response.headers
    except urllib.error.HTTPError as error:
        code, body, response_headers = error.code, error.read(), error.headers
    assert code in (expected if isinstance(expected, tuple) else (expected,)), (path, code, body[:500])
    return code, body, response_headers


def request(path, payload=None, expected=200, **kwargs):
    return json.loads(fetch(path, payload, expected, **kwargs)[1])


def png(color):
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', 1, 1, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + bytes(color))) + chunk(b'IEND', b'')


def ingest(source_id=None, count=3, publication=True, ids=None):
    source_id = source_id or str(uuid.uuid4().int)[:20]
    boundary = 'test-boundary'
    meta = {'source': 'pixiv', 'source_id': source_id, 'title': 'Image review test'}
    if publication:
        meta['telegram_publication'] = {'chat_id': CHAT, 'message_ids': ids or [101 + i for i in range(count)], 'publish_state': 'full'}
    body = b'--' + boundary.encode() + b'\r\nContent-Disposition: form-data; name="meta"\r\n\r\n' + json.dumps(meta).encode() + b'\r\n'
    for i in range(count):
        body += f'--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{i}.png"\r\nContent-Type: image/png\r\n\r\n'.encode() + png((i * 50, 60, 90)) + b'\r\n'
    body += f'--{boundary}--\r\n'.encode()
    req = urllib.request.Request(BASE + '/api/ingest', body, headers={**HEADERS, 'Content-Type': f'multipart/form-data; boundary={boundary}', 'Idempotency-Key': uuid.uuid4().hex})
    with urllib.request.urlopen(req) as response:
        assert json.load(response)['ok']
    return request('/api/catalog?work_id=pixiv:' + source_id)['images']


def review(image):
    decision = uuid.uuid4().hex
    def act(action, expected=200, **kwargs):
        return request('/api/catalog/image-review', {
            'decision_id': decision, 'r2_key': image['r2_key'], 'action': action, **kwargs}, expected)
    return decision, act


def restore_targets(prepared, message=204):
    return [{'publication_id': t['publication_id'], 'message_id': message} for t in prepared['targets']]


def publication(work_id, ids, chat=CHAT, expected=200):
    return request('/api/catalog/publications', {
        'work_id': work_id, 'chat_id': chat, 'message_ids': ids, 'publish_state': 'full'}, expected, method='PUT')


def catalog(work_id):
    return request('/api/catalog?work_id=' + work_id)['images']


def public_images(work_id, expected=200):
    return request('/api/works/' + urllib.parse.quote(work_id, safe=''), expected=expected, auth=False)


def local_d1(sql):
    subprocess.run(WRANGLER + ['d1', 'execute', 'DB', '--local', '--persist-to', STATE, '--command', sql], check=True, capture_output=True)


def main():
    images = ingest()
    image = images[1]
    work_id, key = image['work_id'], image['r2_key']
    decision, act = review(image)
    prepared = act('prepare')
    assert [t['message_id'] for t in prepared['targets']] == [102]
    assert catalog(work_id) == images
    detail = public_images(work_id)['images']
    assert [i['r2_key'] for i in detail] == [i['r2_key'] for i in images]
    assert set(detail[0]) == {'r2_key', 'content_type', 'page_index', 'byte_size'}
    _, original, headers = fetch('/media/' + key, auth=False)
    assert headers['Cache-Control'] == 'private, no-cache'
    assert original.startswith(b'\x89PNG')
    fetch('/media/' + key, expected=304, headers={'If-None-Match': headers['ETag']}, auth=False)
    act('delete')
    assert [i['page_index'] for i in catalog(work_id)] == [0, 2]
    fetch('/media/' + key, expected=404, headers={'If-None-Match': headers['ETag']}, auth=False)
    fetch('/media/review-trash/' + decision + '/' + key, expected=404, auth=False)
    # This fails deterministically if deletion resumes physically deleting originals.
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / 'original.png'
        subprocess.run(WRANGLER + ['r2', 'object', 'get', 'shirogane-media/' + key, '--local', '--persist-to', STATE, '--file', str(output)], check=True, capture_output=True)
        assert output.read_bytes() == original
    act('delete')
    restored = restore_targets(prepared)
    act('restore', restored=restored)
    assert catalog(work_id) == images
    assert fetch('/media/' + key, auth=False)[1] == original
    act('restore', restored=restored)
    act('delete', expected=409)
    _, again = review(image)
    assert again('prepare')['targets'][0]['message_id'] == 204

    cancelled = ingest()[0]
    _, cancel = review(cancelled)
    cancel('prepare')
    assert cancel('restore')['state'] == 'restored'
    cancel('delete', expected=409)
    assert len(catalog(cancelled['work_id'])) == 3
    fetch('/media/' + cancelled['r2_key'], auth=False)

    # Anchor changes retain the original publication identity for PUT and ingest.
    first = ingest()[0]
    _, first_act = review(first)
    first_prepared = first_act('prepare')
    first_id = first_prepared['targets'][0]['publication_id']
    first_act('delete')
    assert publication(first['work_id'], [102, 103])['publication_id'] == first_id
    publication(first['work_id'], [101, 102, 103], expected=409)
    first_act('restore', restored=restore_targets(first_prepared))
    assert publication(first['work_id'], [204, 102, 103])['publication_id'] == first_id
    ingest(first['work_id'].split(':')[1], ids=[204, 102, 103])
    assert publication(first['work_id'], [204, 102, 103])['publication_id'] == first_id

    # A newly added channel must invalidate the entire prepared mapping set.
    added_images = ingest()
    _, added = review(added_images[1])
    added('prepare')
    publication(added_images[1]['work_id'], [401, 402, 403], chat=CHAT - 1)
    added('delete', expected=409)
    assert catalog(added_images[1]['work_id']) == added_images

    # Every whole-work operation, including a retract replay after last-image deletion,
    # invalidates old prepare/restore requests before Hanabi republishes messages.
    keep = ingest()[0]
    for operation in ('retract', 'prune-works', 'reingest', 'prune', 'last-retract'):
        old = ingest(count=1 if operation == 'last-retract' else 3)[0]
        _, old_act = review(old)
        old_prepared = old_act('prepare')
        old_act('delete')
        payload = {'decision_id': uuid.uuid4().hex}
        if operation in ('retract', 'last-retract'):
            request('/api/catalog/retract', {**payload, 'work_id': old['work_id']})
        elif operation == 'prune-works':
            request('/api/catalog/prune-works', {**payload, 'keep_work_id': keep['work_id'], 'remove_work_ids': [old['work_id']]})
        elif operation == 'prune':
            sibling = catalog(old['work_id'])[0]
            request('/api/catalog/prune', {**payload, 'keep_r2_key': keep['r2_key'], 'remove_r2_keys': [sibling['r2_key']]})
        else:
            ingest(old['work_id'].split(':')[1])
        remaining = catalog(old['work_id'])
        old_act('prepare', expected=409)
        old_act('restore', expected=409, restored=restore_targets(old_prepared))
        assert catalog(old['work_id']) == remaining
        fetch('/media/' + old['r2_key'], expected=404, auth=False)
        if operation in ('retract', 'last-retract', 'prune-works'):
            public_images(old['work_id'], expected=404)

    single = ingest(count=1)[0]
    _, single_act = review(single)
    single_prepared = single_act('prepare')
    single_act('delete')
    public_images(single['work_id'], expected=404)
    subprocess.run(WRANGLER + ['r2', 'object', 'delete', 'shirogane-media/' + single['r2_key'], '--local', '--persist-to', STATE], check=True, capture_output=True)
    single_act('restore', restored=restore_targets(single_prepared))
    assert len(public_images(single['work_id'])['images']) == 1
    fetch('/media/' + single['r2_key'], auth=False)

    # Concurrent delete retries cannot physically remove the restored original.
    for _ in range(4):
        current = ingest()[1]
        _, race = review(current)
        race_prepared = race('prepare')
        race('delete')
        with ThreadPoolExecutor(max_workers=8) as pool:
            futures = [pool.submit(race, 'delete', expected=(200, 409)) for _ in range(7)]
            futures.append(pool.submit(race, 'restore', expected=(200, 409), restored=restore_targets(race_prepared)))
            for future in futures:
                future.result()
        race('restore', restored=restore_targets(race_prepared))
        assert race('prepare')['state'] == 'restored'
        fetch('/media/' + current['r2_key'], auth=False)

    for _ in range(4):
        current = ingest(count=1)[0]
        _, race = review(current)
        race_prepared = race('prepare')
        race('delete')
        with ThreadPoolExecutor(max_workers=2) as pool:
            restore = pool.submit(race, 'restore', expected=(200, 409), restored=restore_targets(race_prepared))
            retract = pool.submit(request, '/api/catalog/retract', {
                'decision_id': uuid.uuid4().hex, 'work_id': current['work_id']}, expected=(200, 409))
            restore.result()
            retract_result = retract.result()
        if retract_result.get('ok'):
            public_images(current['work_id'], expected=404)
        else:
            request('/api/catalog/retract', {'decision_id': uuid.uuid4().hex, 'work_id': current['work_id']})
            public_images(current['work_id'], expected=404)

    for _ in range(6):
        current = ingest()[1]
        _, race = review(current)
        race_prepared = race('prepare')
        race('delete')
        prune_payload = {'decision_id': uuid.uuid4().hex, 'keep_work_id': keep['work_id'], 'remove_work_ids': [current['work_id']]}
        with ThreadPoolExecutor(max_workers=2) as pool:
            prune = pool.submit(request, '/api/catalog/prune-works', prune_payload, expected=(200, 409))
            restore = pool.submit(race, 'restore', expected=(200, 409), restored=restore_targets(race_prepared))
            prune_result, restore_result = prune.result(), restore.result()
        if prune_result.get('ok'):
            public_images(current['work_id'], expected=404)
            if restore_result.get('ok'):
                assert 204 in prune_result['telegram_targets'][0]['message_ids'], prune_result
        else:
            # Failed guards roll back the receipt as well, so the same plan is retryable.
            retried = request('/api/catalog/prune-works', prune_payload)
            assert 204 in retried['telegram_targets'][0]['message_ids'], retried

    left, right = ingest()[0], ingest()[0]
    with ThreadPoolExecutor(max_workers=2) as pool:
        opposing = [pool.submit(request, '/api/catalog/prune-works', {
            'decision_id': uuid.uuid4().hex, 'keep_work_id': kept['work_id'], 'remove_work_ids': [removed['work_id']]}, expected=(200, 409))
            for kept, removed in ((left, right), (right, left))]
        assert sum(bool(future.result().get('ok')) for future in opposing) == 1
    assert bool(catalog(left['work_id'])) != bool(catalog(right['work_id']))

    # Legacy snapshots cannot establish ordering against pre-migration whole-work changes.
    legacy = ingest()[0]
    legacy_id, legacy_act = review(legacy)
    legacy_act('prepare')
    local_d1(f"UPDATE catalog_image_reviews SET payload=json_remove(payload,'$.review_version') WHERE decision_id='{legacy_id}'")
    legacy_act('delete', expected=409)
    assert len(catalog(legacy['work_id'])) == 3

    no_mapping = ingest(count=1, publication=False)[0]
    _, missing = review(no_mapping)
    missing('prepare', expected=409)
    public_images('pixiv:999999999999999999999999999999', expected=404)
    public_images('invalid', expected=400)
    public_images('pixiv:../secret', expected=400)
    print('PASS: exact delete/undo, immutable R2, hidden backups, ETag/304, public album detail, stable anchors, complete mapping set, whole-work invalidation, legacy refusal, concurrent delete/restore')


if __name__ == '__main__':
    main()
