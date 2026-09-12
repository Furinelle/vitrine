"""Run against a local Wrangler instance on port 18787 with INGEST_TOKEN=local-image-review-test."""
import json
import struct
import urllib.error
import urllib.request
import uuid
import zlib

BASE = 'http://127.0.0.1:18787'
HEADERS = {'Authorization': 'Bearer local-image-review-test'}

def request(path, payload=None, expected=200, headers=None):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(BASE + path, data, headers={**HEADERS, 'Content-Type': 'application/json', **(headers or {})})
    try:
        with urllib.request.urlopen(req) as response:
            code, body = response.status, response.read()
    except urllib.error.HTTPError as error:
        code, body = error.code, error.read()
    assert code == expected, (code, body.decode())
    return json.loads(body)

def png(color):
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', 1, 1, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + bytes(color))) + chunk(b'IEND', b'')

def ingest(source_id, count=3, publication=True):
    boundary = 'test-boundary'
    meta = {'source': 'pixiv', 'source_id': source_id, 'title': 'Image review test'}
    if publication:
        meta['telegram_publication'] = {'chat_id': -1001234567890, 'message_ids': [101 + i for i in range(count)], 'publish_state': 'full'}
    body = b'--' + boundary.encode() + b'\r\nContent-Disposition: form-data; name="meta"\r\n\r\n' + json.dumps(meta).encode() + b'\r\n'
    for i in range(count):
        body += f'--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{i}.png"\r\nContent-Type: image/png\r\n\r\n'.encode() + png((i * 50, 60, 90)) + b'\r\n'
    body += f'--{boundary}--\r\n'.encode()
    req = urllib.request.Request(BASE + '/api/ingest', body, headers={**HEADERS, 'Content-Type': f'multipart/form-data; boundary={boundary}', 'Idempotency-Key': uuid.uuid4().hex})
    with urllib.request.urlopen(req) as response:
        assert json.load(response)['ok']
    return request('/api/catalog?work_id=pixiv:' + source_id)['images']

def main():
    source_id = str(uuid.uuid4().int)[:20]
    images = ingest(source_id)
    key = images[1]['r2_key']
    decision = uuid.uuid4().hex
    def act(action, **kwargs):
        return request('/api/catalog/image-review', {'decision_id': decision, 'r2_key': key, 'action': action, **kwargs})
    before = request('/api/catalog?work_id=pixiv:' + source_id)['images']
    prepared = act('prepare')
    assert [t['message_id'] for t in prepared['targets']] == [102]
    assert request('/api/catalog?work_id=pixiv:' + source_id)['images'] == before
    act('delete')
    assert [i['page_index'] for i in request('/api/catalog?work_id=pixiv:' + source_id)['images']] == [0, 2]
    act('delete')  # Retry is idempotent.
    restored = [{'publication_id': t['publication_id'], 'message_id': 204} for t in prepared['targets']]
    act('restore', restored=restored)
    assert request('/api/catalog?work_id=pixiv:' + source_id)['images'] == before
    act('restore', restored=restored)
    request('/api/catalog/image-review', {'decision_id': decision, 'r2_key': key, 'action': 'delete'}, expected=409)
    next_decision = uuid.uuid4().hex
    again = request('/api/catalog/image-review', {'decision_id': next_decision, 'r2_key': key, 'action': 'prepare'})
    assert [t['message_id'] for t in again['targets']] == [204]
    no_mapping = ingest(source_id + '9', 1, False)[0]
    request('/api/catalog/image-review', {'decision_id': uuid.uuid4().hex, 'r2_key': no_mapping['r2_key'], 'action': 'prepare'}, expected=409)
    print('PASS: exact middle-page deletion, sibling preservation, metadata undo, retries, restored Telegram mapping, missing-mapping refusal')

if __name__ == '__main__':
    main()
