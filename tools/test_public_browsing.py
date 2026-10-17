"""Public browsing regressions. Requires an isolated local Wrangler and matching state.
VITRINE_TEST_BASE=http://127.0.0.1:18787 VITRINE_TEST_STATE=/tmp/vitrine-test-state python3 tools/test_public_browsing.py
"""
import json
import urllib.parse
import uuid
import test_image_review as local


def main():
    assert urllib.parse.urlsplit(local.BASE).hostname in {'127.0.0.1', 'localhost', '::1'}
    prefix = 'browse' + uuid.uuid4().hex[:12]
    work_ids = [local.ingest(prefix + str(i), count=1, publication=False,
                           meta_overrides={'title': prefix, 'tags': [prefix + '-low-volume']})[0]['work_id'] for i in range(6)]
    ids_sql = ','.join("'" + work + "'" for work in work_ids)
    local.local_d1(f"UPDATE works SET created_at='2026-10-08T00:00:00Z' WHERE id IN ({ids_sql})")
    query = '/api/works?' + urllib.parse.urlencode({'q': prefix, 'limit': 2})
    first = local.request(query, auth=False)
    assert [w['id'] for w in first['works']] == sorted(work_ids, reverse=True)[:2]
    # Deleting an already-seen row shifts offsets, but must not skip an unseen work.
    local.request('/api/catalog/retract', {'work_id': first['works'][0]['id'], 'decision_id': uuid.uuid4().hex})
    local.ingest(prefix + 'new', count=1, publication=False, meta_overrides={'title': prefix})
    seen = [w['id'] for w in first['works']]
    cursor = first['next_cursor']
    while cursor:
        page = local.request(query + '&cursor=' + urllib.parse.quote(cursor), auth=False)
        seen.extend(w['id'] for w in page['works'])
        cursor = page['next_cursor']
    assert seen == sorted(work_ids, reverse=True), seen
    assert len(set(seen)) == len(seen)
    local.request('/api/works?cursor=not-json', expected=400, auth=False)

    # Month pages count collection months in Beijing time: 16:00 UTC on 30 September is already October.
    tagged = sorted(set(work_ids) - {first['works'][0]['id']})
    local.local_d1(f"UPDATE works SET created_at='2026-09-30T16:00:00.000Z' WHERE id='{tagged[0]}'; "
                   f"UPDATE works SET created_at='2026-09-30T15:59:59.999Z' WHERE id='{tagged[1]}'")
    month_filter = {'tag': prefix + '-low-volume'}
    months = local.request('/api/months?' + urllib.parse.urlencode(month_filter), auth=False)['months']
    assert months == [{'month': '2026-10', 'cnt': 4}, {'month': '2026-09', 'cnt': 1}], months
    page = local.request('/api/works?' + urllib.parse.urlencode({**month_filter, 'month': '2026-10', 'limit': 3, 'offset': 3}), auth=False)['works']
    assert [w['id'] for w in page] == [tagged[0]], page
    september = local.request('/api/works?' + urllib.parse.urlencode({**month_filter, 'month': '2026-09'}), auth=False)['works']
    assert [w['id'] for w in september] == [tagged[1]], september
    local.request('/api/works?month=2026-13', expected=400, auth=False)
    local.request('/api/months?month=26-1', expected=400, auth=False)

    # Tag search/pagination sees tags beyond the old hard-coded first 200 entries.
    survivor = work_ids[0]
    tags = [f'{prefix}-tag-{i:03}' for i in range(205)]
    tags.extend([prefix + '%literal', prefix + '_literal'])
    local.local_d1('INSERT INTO tags(name) VALUES ' + ','.join(f"('{tag}')" for tag in tags) + ';' +
                   'INSERT INTO work_tags(work_id,tag) VALUES ' + ','.join(f"('{survivor}','{tag}')" for tag in tags))
    found, offset = [], 0
    while offset is not None:
        data = local.request('/api/tags?' + urllib.parse.urlencode({'q': prefix + '-tag-', 'limit': 100, 'offset': offset, 'order': 'name'}), auth=False)
        found.extend(t['name'] for t in data['tags'])
        offset = data['next_offset']
    assert found == sorted(tags[:205])
    for literal in ('%', '_'):
        data = local.request('/api/tags?' + urllib.parse.urlencode({'q': prefix + literal}), auth=False)
        assert [t['name'] for t in data['tags']] == [prefix + literal + 'literal']
    def cached(path, expected=None):
        _, body, headers = local.fetch(path, auth=False)
        assert headers['Cache-Control'] == 'no-store'
        if expected:
            assert headers['X-Catalog-Stats-Cache'] == expected, headers
        return json.loads(body)

    cached('/api/tags?limit=1', 'HIT')
    cached('/api/tags?' + urllib.parse.urlencode({'q': prefix.upper(), 'order': 'name', 'offset': 100}), 'HIT')
    cached('/api/sources')
    cached('/api/sources', 'HIT')
    new_tag = prefix + '-cache-invalidation'
    local.local_d1(f"INSERT INTO tags(name,use_count) VALUES ('{new_tag}',999); INSERT INTO work_tags(work_id,tag) VALUES ('{survivor}','{new_tag}')")
    refreshed = cached('/api/tags?' + urllib.parse.urlencode({'q': new_tag}), 'MISS')
    assert [(t['name'], t['cnt'], t['kind']) for t in refreshed['tags']] == [(new_tag, 1, 'unsorted')]
    cached('/api/sources', 'MISS')
    local.request('/api/catalog/retract', {'work_id': survivor, 'decision_id': uuid.uuid4().hex})
    assert not cached('/api/tags?' + urllib.parse.urlencode({'q': new_tag}), 'MISS')['tags']
    cached('/api/sources', 'MISS')
    print('PASS: deterministic cursor and literal search; month counts and month pages in Beijing time; shared full-tag/source caches; direct tag changes and hidden works invalidate counts')


if __name__ == '__main__':
    main()
