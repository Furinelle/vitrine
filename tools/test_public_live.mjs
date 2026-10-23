// Read-only production acceptance. No credentials, uploads or content mutations.
import assert from "node:assert/strict";
import sharp from "sharp";

const base = "https://gallery.fontaine.blue";
const headers = { "User-Agent": "Mozilla/5.0 VitrinePublicAcceptance/1.0" };
const get = (path, extra = {}) => fetch(new URL(path, base), {
  headers: { ...headers, ...extra }, signal: AbortSignal.timeout(30_000),
});
const json = async (path) => {
  const response = await get(path);
  assert.equal(response.status, 200, path);
  return response.json();
};

const home = await get("/");
assert.equal(home.status, 200);
assert.match(home.headers.get("content-security-policy"), /script-src 'self'/);
assert.equal(home.headers.get("x-frame-options"), "DENY");
assert.equal((await get("/api/catalog")).status, 401);
const first = await json("/api/works?limit=2");
assert.equal(first.works.length, 2);
assert.ok(first.next_cursor);
assert.ok(first.works.every(work => !("origin" in work)));
const next = await json("/api/works?limit=2&cursor=" + encodeURIComponent(first.next_cursor));
assert.ok(next.works.every(work => first.works.every(prior => prior.id !== work.id)));
const tags = await json("/api/tags?order=name&offset=200&limit=20");
assert.equal(tags.tags.length, 20);
const cover = first.works[0].cover_url;
assert.ok(cover.startsWith("/media/"));
const sizes = [];
for (const width of [160, 640, 1280]) {
  const path = cover + "?width=" + width;
  const response = await get(path, { Accept: "image/webp" });
  assert.equal(response.status, 200);
  assert.ok(response.headers.get("cf-resized"), "Resize silently fell back to the original");
  assert.equal(response.headers.get("cache-control"), "private, max-age=3600");
  const bytes = Buffer.from(await response.arrayBuffer());
  const metadata = await sharp(bytes).metadata();
  assert.equal(metadata.format, "webp");
  assert.ok(metadata.width > 0 && metadata.width <= width);
  sizes.push({ requested: width, width: metadata.width, bytes: bytes.length });
  if (width === 640) {
    const etag = response.headers.get("etag");
    assert.ok(etag);
    assert.equal((await get(path, { Accept: "image/webp", "If-None-Match": etag })).status, 304);
  }
}
assert.equal((await get("/cdn-cgi/image/width=640,quality=80,format=auto" + cover)).status, 403);
console.log("PASS: live headers, auth, cursor, full tag index, resized dimensions, ETag and legacy-image block", sizes);
