"""Local-only HTTP security regressions; use the image-review local fixture.

VITRINE_TEST_BASE=http://127.0.0.1:18787 python3 tools/test_public_security.py
Requires local migrations, INGEST_TOKEN=local-image-review-test, and a local
Wrangler started with --local-upstream 127.0.0.1 --upstream-protocol http.
"""
import http.client
import json
import urllib.parse
import uuid

import test_image_review as fixture

BASE = urllib.parse.urlsplit(fixture.BASE)
assert BASE.scheme == 'http' and BASE.hostname in {'127.0.0.1', 'localhost', '::1'}, 'Local test only'
ORIGIN = f'{BASE.scheme}://{BASE.netloc}'


def fetch(path, payload=None, expected=200, headers=None, **kwargs):
    return fixture.fetch(path, payload, expected, headers=headers, auth=False, **kwargs)


def main():
    for path in ('/', '/admin', '/api/health', '/api/not-a-route'):
        _, _, headers = fetch(path, expected=404 if path.endswith('not-a-route') else 200)
        assert headers['X-Content-Type-Options'] == 'nosniff', path
        assert headers['X-Frame-Options'] == 'DENY', path
        assert "frame-ancestors 'none'" in headers['Content-Security-Policy'], path
        assert "script-src 'self'" in headers['Content-Security-Policy'], path
        assert headers['Strict-Transport-Security'] == 'max-age=31536000', path

    fetch('/api/catalog', expected=401)
    fetch('/api/catalog/fingerprints', {'padding': 'x' * (1024 * 1024 + 1)}, expected=401, method='PUT')
    fetch('/api/admin/similar', expected=401)
    fetch('/api/ingest', {}, expected=401, method='POST')
    fetch('/api/works?q=' + 'a' * 513, expected=400)
    # A malicious Origin must never be rescued by a legitimate-looking Referer.
    for origin in ('null', 'https://evil.example'):
        fetch('/api/admin/login', {'token': 'local-image-review-test'}, expected=403,
              headers={'Origin': origin, 'Referer': ORIGIN + '/admin'})

    _, _, headers = fetch('/api/admin/login', {'token': 'local-image-review-test'},
                          headers={'Origin': ORIGIN})
    cookie = headers['Set-Cookie']
    assert 'HttpOnly' in cookie and 'SameSite=Lax' in cookie and 'Path=/' in cookie
    admin = {'Cookie': cookie.split(';', 1)[0], 'Origin': ORIGIN}
    session = json.loads(fetch('/api/admin/session', headers=admin)[1])
    assert session['authenticated'] is True
    assert json.loads(fetch('/api/admin/session', headers={'Cookie': admin['Cookie'] + '0'})[1])['authenticated'] is False

    image = fixture.ingest(source_id=str(uuid.uuid4().int)[:20], count=1, publication=False)[0]
    work = urllib.parse.quote(image['work_id'], safe='')
    details = json.loads(fetch('/api/works/' + work)[1])
    assert details['work']['id'] == image['work_id'] and 'origin' not in details['work']
    listing = json.loads(fetch('/api/works?limit=100')[1])
    assert all('origin' not in item for item in listing['works'])

    patch = '/api/admin/works/' + work
    for origin in ('null', 'https://evil.example'):
        fetch(patch, {}, expected=403, method='PATCH',
              headers={**admin, 'Origin': origin, 'Referer': ORIGIN + '/admin'})
        fetch('/api/catalog/image-review', {}, expected=403, method='POST',
              headers={**admin, 'Origin': origin, 'Referer': ORIGIN + '/admin'})
    fetch(patch, {}, method='PATCH', headers=admin)
    fetch(patch, {}, expected=403, method='PATCH', headers={'Cookie': admin['Cookie']})

    # No Content-Length: stream counting must reject oversize chunked JSON too.
    connection = http.client.HTTPConnection(BASE.hostname, BASE.port, timeout=20)
    connection.request('PUT', '/api/catalog/fingerprints',
                       body=iter([b' ' * 65536] * 17), encode_chunked=True,
                       headers={**fixture.HEADERS, 'Content-Type': 'application/json'})
    response = connection.getresponse()
    assert response.status == 413, (response.status, response.read()[:200])
    response.read()
    connection.close()

    # Loopback fixtures deliberately use originals. Deployment must verify
    # real resized dimensions and bytes separately.
    for width in (160, 640, 1280):
        _, body, headers = fetch('/media/' + image['r2_key'] + f'?width={width}')
        assert body and headers['Content-Type'].startswith('image/')
        assert headers['Cache-Control'] == 'private, max-age=3600'
    fetch('/media/' + image['r2_key'] + '?width=9999', expected=400)
    _, _, headers = fetch('/media/' + image['r2_key'])
    etag = headers['ETag']
    fetch(patch + '/hide', {}, method='POST', headers=admin)
    fetch('/api/works/' + work, expected=404)
    for suffix in ('', '?width=160', '?width=640', '?width=1280'):
        fetch('/media/' + image['r2_key'] + suffix, expected=404, headers={'If-None-Match': etag})

    _, _, headers = fetch('/api/admin/logout', {}, method='POST', headers=admin)
    assert 'Max-Age=0' in headers['Set-Cookie']
    # Cross-origin preflight cannot authorize credential-bearing API mutations.
    _, _, headers = fetch('/api/health', method='OPTIONS', expected=204)
    assert 'POST' not in headers.get('Access-Control-Allow-Methods', '')
    assert 'Authorization' not in headers.get('Access-Control-Allow-Headers', '')
    print('PASS: headers, auth/cookie, strict CSRF, chunked limits, private metadata, guarded media variants and hiding')


if __name__ == '__main__':
    main()
