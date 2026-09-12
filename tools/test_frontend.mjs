// URL handling is a trust boundary: catalog metadata must never create executable links.
// Run with: node tools/test_frontend.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../public/app.js', import.meta.url), 'utf8');
const context = vm.createContext({ URL, location: { origin: 'https://gallery.example' } });
vm.runInContext(source.slice(source.indexOf('function safeLink('), source.indexOf('async function getJson(')), context);
for (const value of ['javascript:alert(1)', 'data:text/html,test', '//evil.example', '', null]) {
  assert.equal(context.safeLink(value), '');
}
assert.equal(context.safeLink('https://www.pixiv.net/artworks/123'), 'https://www.pixiv.net/artworks/123');
assert.equal(context.coverUrl('https://evil.example/media/a.png'), '');
assert.equal(context.coverUrl('/media/pixiv/123/a.png'), 'https://gallery.example/media/pixiv/123/a.png');
assert.equal(context.mediaUrl('pixiv/123/a #?.png'), '/media/pixiv/123/a%20%23%3F.png');
console.log('PASS: only HTTP source links and same-origin media, escaped object keys');
