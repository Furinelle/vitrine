"""Local-only large multipart, SHA-256, R2 and idempotency regressions."""
import hashlib
import json
import struct
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zlib

from test_image_review import BASE, HEADERS, fetch, png, request


def main():
    assert urllib.parse.urlparse(BASE).hostname in ('127.0.0.1', 'localhost'), BASE
    source_id = str(uuid.uuid4().int)[:20]
    key = uuid.uuid4().hex
    # Valid PNG ancillary chunks exercise nearly the 100 MiB request allowance.
    files = []
    for mib in (48, 24, 24):
        text = b'Comment\0' + b'x' * (mib * 1024 * 1024)
        chunk = b'tEXt' + text
        files.append(png((50, 60, 90))[:-12] + struct.pack('!I', len(text))
                     + chunk + struct.pack('!I', zlib.crc32(chunk)) + png((50, 60, 90))[-12:])

    def send(payloads, expected=200, idem=key):
        boundary = 'ingest-stream-regression'
        meta = {'source': 'pixiv', 'source_id': source_id, 'title': 'Large ingest regression'}
        parts = [f'--{boundary}\r\nContent-Disposition: form-data; name="meta"\r\n\r\n'.encode()
                 + json.dumps(meta).encode() + b'\r\n']
        for i, data in enumerate(payloads):
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{i}.png"\r\nContent-Type: image/png\r\n\r\n'.encode()
                         + data + b'\r\n')
        parts.append(f'--{boundary}--\r\n'.encode())
        req = urllib.request.Request(BASE + '/api/ingest', b''.join(parts), headers={
            **HEADERS, 'Content-Type': f'multipart/form-data; boundary={boundary}',
            'Idempotency-Key': idem})
        try:
            with urllib.request.urlopen(req, timeout=120) as response:
                status, result = response.status, json.load(response)
        except urllib.error.HTTPError as error:
            status, result = error.code, json.load(error)
        assert status == expected, (status, result)
        return result

    first = send(files)
    assert first['ok'] and not first['idempotent'] and first['pages'] == len(files), first
    for image, data in zip(first['images'], files):
        assert image['bytes'] == len(data)
        assert image['sha256'] == hashlib.sha256(data).hexdigest()
        assert fetch('/media/' + image['r2_key'], auth=False)[1] == data
    replay = send(files)
    assert replay['idempotent'] and replay['images'] == first['images'], replay
    send([png((1, 2, 3))], expected=409)
    send([b'not a PNG'], expected=400, idem=uuid.uuid4().hex)
    send([b'\x89PNG\r\n\x1a'], expected=400, idem=uuid.uuid4().hex)
    assert len(request('/api/catalog?work_id=pixiv:' + source_id)['images']) == len(files)
    print('PASS: 96 MiB ingest, native SHA-256, original R2 bytes, replay/conflict, magic validation')


if __name__ == '__main__':
    main()
