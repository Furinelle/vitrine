"""Admin tag page API: library edits imply tags on old and new works, merge/rename moves works,
keys and CP members, block removes and skips a tag, bad edits are refused.
VITRINE_TEST_BASE=http://127.0.0.1:18787 VITRINE_TEST_STATE=/tmp/vitrine-test-state python3 tools/test_tag_admin.py
"""
import urllib.parse
import uuid
import test_image_review as local


def work_tags(work_id):
    return sorted(local.request('/api/works/' + urllib.parse.quote(work_id, safe=''), auth=False)['work']['tags'])


def main():
    assert urllib.parse.urlsplit(local.BASE).hostname in {'127.0.0.1', 'localhost', '::1'}
    stem = uuid.uuid4().hex[:8]
    work, char, alias, pair, blocked = (f'作品{stem}', f'角色{stem}', f'别名{stem}', f'组合{stem}', f'屏蔽{stem}')

    def ingest(suffix, tags):
        return local.ingest(f'tagadmin{stem}{suffix}', count=1, publication=False, meta_overrides={'tags': tags})[0]['work_id']

    origin = local.BASE.rstrip('/')
    local.fetch('/api/admin/tags/items', {'items': []}, expected=401, auth=False, headers={'Origin': origin})
    _, _, headers = local.fetch('/api/admin/login', {'token': 'local-image-review-test'}, auth=False,
                                headers={'Origin': origin, 'CF-Connecting-IP': '192.0.2.175'})
    cookie = {'Origin': origin, 'Cookie': headers['Set-Cookie'].split(';', 1)[0]}

    def admin(path, payload, expected=200):
        return local.request('/api/admin/tags' + path, payload, expected, auth=False, headers=cookie, method='POST')

    def listed():
        rows = local.request('/api/admin/tags', auth=False, headers=cookie)['tags']
        return {t['name']: t for t in rows if stem in t['name']}

    w1, w2, w3 = ingest('a', [char]), ingest('b', [alias, blocked]), ingest('c', [work])
    admin('/items', {'items': [{'name': work, 'kind': 'work', 'en': f'Work{stem}'},
                               {'name': char, 'kind': 'character', 'parent': work, 'ja': f'キャラ{stem}'}]})
    assert work_tags(w1) == sorted([char, work]), work_tags(w1)
    assert work_tags(ingest('d', [char])) == sorted([char, work])
    found = {w['id'] for w in local.request('/api/works?' + urllib.parse.urlencode({'tag': f'work{stem}'}), auth=False)['works']}
    assert {w1, w3} <= found, found
    t = listed()
    assert (t[char]['kind'], t[char]['parent'], t[char]['ja']) == ('character', work, f'キャラ{stem}'), t[char]

    # A pairing brings its members, and through them the work.
    admin('/items', {'items': [{'name': alias, 'kind': 'character'}]})
    admin('/items', {'items': [{'name': pair, 'kind': 'cp', 'members': [char, alias]}]}, expected=404)
    ingest('e', [pair])
    admin('/items', {'items': [{'name': pair, 'kind': 'cp', 'members': [char, alias]}]})
    assert work_tags(ingest('f', [pair])) == sorted([pair, char, alias, work])

    # Merge: works move, the old spelling resolves to the target, member lists follow.
    moved = admin('/merge', {'from': alias, 'into': char})
    assert moved['moved'] == 3, moved  # w2, plus e and f through the pairing
    assert work_tags(w2) == sorted([char, work, blocked]), work_tags(w2)
    assert work_tags(ingest('g', [alias.upper()])) == sorted([char, work])
    t = listed()
    assert alias not in t and t[pair]['members'] == [char], t[pair]

    # Rename: a new name takes over the works, the library entry and references to it.
    renamed = char + '新'
    admin('/merge', {'from': char, 'into': renamed})
    t = listed()
    assert char not in t and (t[renamed]['parent'], t[renamed]['ja']) == (work, f'キャラ{stem}'), t.get(renamed)
    assert t[pair]['members'] == [renamed], t[pair]
    assert work_tags(w1) == sorted([renamed, work]), work_tags(w1)

    # Block: gone from every work and skipped on later ingest.
    assert admin('/block', {'name': blocked})['removed'] == 1
    assert work_tags(w2) == sorted([renamed, work])
    assert work_tags(ingest('h', [blocked, work])) == [work]

    # Refused: implication cycles, a display name that is another tag's spelling, unknown kinds.
    admin('/items', {'items': [{'name': work, 'kind': 'work', 'parent': renamed}]}, expected=409)
    admin('/items', {'items': [{'name': work, 'kind': 'work', 'en': renamed}]}, expected=409)
    admin('/items', {'items': [{'name': work, 'kind': 'nonsense'}]}, expected=400)
    admin('/merge', {'from': work, 'into': blocked}, expected=409)

    # Unsorted takes a tag out of the library; implied tags already on works stay.
    admin('/items', {'items': [{'name': renamed, 'kind': 'unsorted'}]})
    assert listed()[renamed]['kind'] == 'unsorted'
    assert work_tags(ingest('i', [renamed])) == [renamed]
    assert work_tags(w1) == sorted([renamed, work])
    print('PASS: admin tag edits imply tags on old and new works; merge, rename and block move keys, '
          'members and works; cycles and conflicting names are refused')


if __name__ == '__main__':
    main()
