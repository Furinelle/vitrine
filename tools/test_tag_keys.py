"""Ingest/admin/filter regressions for tag_keys. Requires an isolated local Wrangler and matching state.
VITRINE_TEST_BASE=http://127.0.0.1:18787 VITRINE_TEST_STATE=/tmp/vitrine-test-state python3 tools/test_tag_keys.py
"""
import urllib.parse
import uuid
import test_image_review as local


def work_tags(work_id):
    return sorted(local.request('/api/works/' + urllib.parse.quote(work_id, safe=''), auth=False)['work']['tags'])


def main():
    assert urllib.parse.urlsplit(local.BASE).hostname in {'127.0.0.1', 'localhost', '::1'}
    stem = 'Key' + uuid.uuid4().hex[:8]
    first = local.ingest(stem + 'a', count=1, publication=False, meta_overrides={'tags': [stem]})[0]['work_id']
    # Case, full-width, invisible filler and a same-key duplicate within one work.
    wide = ''.join(chr(ord(c) + 0xFEE0) for c in stem.lower())
    second = local.ingest(stem + 'b', count=1, publication=False,
                          meta_overrides={'tags': [stem.upper(), wide + 'ㅤ', stem.lower(), '鳴潮' + stem]})[0]['work_id']
    assert work_tags(first) == [stem], work_tags(first)
    assert work_tags(second) == sorted([stem, '鳴潮' + stem]), work_tags(second)
    third = local.ingest(stem + 'c', count=1, publication=False, meta_overrides={'tags': ['鸣潮' + stem.lower()]})[0]['work_id']
    assert work_tags(third) == ['鳴潮' + stem], work_tags(third)

    counts = {t['name']: t['cnt'] for t in local.request('/api/tags?' + urllib.parse.urlencode({'q': stem}), auth=False)['tags']}
    assert counts == {stem: 2, '鳴潮' + stem: 2}, counts

    # Filters written with any variant reach the stored spelling.
    def ids(tag):
        return {w['id'] for w in local.request('/api/works?' + urllib.parse.urlencode({'tag': tag}), auth=False)['works']}
    assert ids(stem.lower()) == ids(wide) == {first, second}
    assert ids('鸣潮' + stem) == {second, third}
    assert ids(f'{stem.upper()},鸣潮{stem}') == {first, second, third}

    # Admin edits resolve the same way.
    origin = local.BASE.rstrip('/')
    _, _, headers = local.fetch('/api/admin/login', {'token': 'local-image-review-test'}, auth=False,
                                headers={'Origin': origin, 'CF-Connecting-IP': '192.0.2.174'})
    cookie = {'Origin': origin, 'Cookie': headers['Set-Cookie'].split(';', 1)[0]}
    local.request('/api/admin/works/' + urllib.parse.quote(third, safe=''), {'tags': [stem.swapcase()]},
                  auth=False, headers=cookie, method='PATCH')
    assert work_tags(third) == [stem], work_tags(third)
    # Linked bilingual tags: either side brings the other, through ingest and admin edits.
    zh, en = '游戏' + stem, stem + 'Game'
    # tools/tag_merge.py keys both names and links them; the key is the lowercase ASCII name.
    local.local_d1(f"INSERT INTO tag_links (tag, also) VALUES ('{zh}', '{en}'), ('{en}', '{zh}'); "
                   f"INSERT INTO tag_keys (key, tag) VALUES ('{en.lower()}', '{en}'), ('游戏{stem.lower()}', '{zh}')")
    linked = local.ingest(stem + 'd', count=1, publication=False, meta_overrides={'tags': [en.upper()]})[0]['work_id']
    assert work_tags(linked) == sorted([en, zh]), work_tags(linked)
    local.request('/api/admin/works/' + urllib.parse.quote(third, safe=''), {'tags': [zh]},
                  auth=False, headers=cookie, method='PATCH')
    assert work_tags(third) == sorted([zh, en]), work_tags(third)
    # Blocked tags (migration 0018) are skipped on ingest, variants included, without touching counts.
    blocked = 'Blocked' + stem
    local.local_d1(f"INSERT INTO tag_blocks (name) VALUES ('{blocked}'); "
                   f"INSERT INTO tag_keys (key, tag) VALUES ('{blocked.lower()}', '{blocked}')")
    skipped = local.ingest(stem + 'e', count=1, publication=False, meta_overrides={'tags': [blocked.upper(), zh]})[0]['work_id']
    assert work_tags(skipped) == sorted([zh, en]), work_tags(skipped)
    found = local.request('/api/tags?' + urllib.parse.urlencode({'q': blocked}), auth=False)['tags']
    assert not found, found
    # Tag library (migration 0019): kind, parent and display names come back with the tag list,
    # and any display name finds it.
    local.local_d1(f"INSERT INTO tag_items (name, kind, parent, ja, en) VALUES ('{zh}', 'work', NULL, NULL, 'Lib{stem}')")
    lib = local.request('/api/tags?' + urllib.parse.urlencode({'q': 'lib' + stem.lower()}), auth=False)['tags']
    assert [(t['name'], t['kind'], t['en']) for t in lib] == [(zh, 'work', 'Lib' + stem)], lib
    print('PASS: ingest, admin edits and tag filters fold case, width, invisible and Traditional variants onto one stored tag; linked tags travel together; blocked tags are skipped')


if __name__ == '__main__':
    main()
