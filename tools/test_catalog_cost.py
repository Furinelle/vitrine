"""Local-only cursor/digest protocol and unchanged-fingerprint write regression."""
import hashlib
import json
import urllib.parse

import test_image_review as local


def main():
    assert urllib.parse.urlsplit(local.BASE).hostname in ('127.0.0.1', 'localhost', '::1')
    local.request('/api/catalog?after=&fingerprints=1', expected=401, auth=False)
    images = local.ingest(count=2, publication=False)
    after = ''
    exported = []
    while True:
        page = local.request('/api/catalog?' + urllib.parse.urlencode({'after': after, 'limit': 2, 'fingerprints': 1}))
        assert all(row['catalog_id'] > after for row in page['images'])
        exported.extend(page['images'])
        cursor = page['next_cursor']
        if cursor is None:
            break
        assert cursor == page['images'][-1]['catalog_id'] and cursor > after
        after = cursor
    keys = [row['catalog_id'] for row in exported]
    assert keys == sorted(set(keys))
    assert all(row['r2_key'] in [x['r2_key'] for x in exported] for row in images)
    target = next(x for x in exported if x['r2_key'] == images[0]['r2_key'])
    assert target['fingerprint_digest'] is None
    payload = {'r2_key': target['r2_key'], 'solid_color': False, 'content_sha256': target['sha256'],
               'strict_key': 'strict', 'average_hash': '0000000000000001', 'difference_hash': '0000000000000002',
               'color_key': 'color', 'detail_key': 'detail', 'width': 100, 'height': 200, 'bytes': 1234,
               'format': 'JPEG', 'regions': [{'width': 50, 'height': 200, 'average_hash': '0000000000000003',
                                             'difference_hash': '0000000000000004', 'color_key': 'region'}]}
    local.request('/api/catalog/fingerprints', {'images': [payload]}, method='PUT')
    before = local.request('/api/catalog/similar/snapshot')
    prior = max((key for key in keys if key < target['catalog_id']), default='')
    page = local.request('/api/catalog?' + urllib.parse.urlencode({'after': prior, 'limit': 1, 'fingerprints': 1}))
    actual = page['images'][0]
    assert actual['catalog_id'] == target['catalog_id']
    expected = hashlib.sha256(json.dumps(payload, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
    assert actual['fingerprint_digest'] == expected, (actual, expected)
    local.request('/api/catalog/fingerprints', {'images': [payload]}, method='PUT')
    assert local.request('/api/catalog/similar/snapshot') == before
    for bad in ('after=x&offset=1', 'after=x&work_id=pixiv:1', 'after=' + 'x' * 2049):
        local.request('/api/catalog?' + bad, expected=400)
    print('PASS: authenticated full cursor export, cross-language fingerprint digest, unchanged PUT leaves version unchanged')


if __name__ == '__main__':
    main()
