// Exercise the real viewer helpers without network requests or browser timers.
// Run with: node tools/test_viewer.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../public/app.js', import.meta.url), 'utf8');
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fixture() {
  const cached = new Set(), elements = new Map(), motions = [], details = [];
  let reduced = false;
  function element(id) {
    if (elements.has(id)) return elements.get(id);
    const classes = new Set();
    const item = {
      hidden: true, open: false, complete: false, naturalWidth: 0, assignments: [],
      clientWidth: 1000, style: {}, closes: 0,
      classList: {
        add: name => classes.add(name), remove: name => classes.delete(name),
        contains: name => classes.has(name),
        toggle(name, enabled) { enabled ? classes.add(name) : classes.delete(name); },
      },
      setAttribute() {}, removeAttribute(name) { if (name === 'src') this._src = ''; },
      replaceChildren() {}, querySelectorAll() { return []; },
      getBoundingClientRect() { return { x: 0, y: 0, width: 1000, height: 800 }; },
      showModal() { this.open = true; }, close() { this.open = false; this.closes++; },
      decode: () => Promise.resolve(),
      animate() {
        const done = deferred();
        const motion = { finished: done.promise, resolve: done.resolve, cancelled: false,
          cancel() { this.cancelled = true; done.reject(new Error('cancelled')); } };
        motions.push(motion);
        return motion;
      },
      get src() { return this._src || ''; },
      set src(value) {
        this._src = value; this.assignments.push(value);
        this.complete = cached.has(value); this.naturalWidth = this.complete ? 640 : 0;
      },
    };
    elements.set(id, item);
    return item;
  }
  const state = { source: '', tag: '', q: '', month: '', p: 1, months: [], works: [], images: [
    { r2_key: 'x/one/0.jpg' }, { r2_key: 'x/one/1.jpg' },
  ], viewing: { id: 'x:one', title: 'Sample' }, imageIndex: 0 };
  const location = { origin: 'https://gallery.example', search: '' };
  const history = { state: null, backs: 0,
    replaceState(value, _title, url) { this.state = value; location.search = new URL(url, location.origin).search; },
    pushState(value, title, url) { this.replaceState(value, title, url); },
    back() { this.backs++; },
  };
  const preloaded = [];
  const context = vm.createContext({
    URL, URLSearchParams, AbortController, state, location, history, $: element,
    Image: class { set src(value) { preloaded.push(value); } },
    window: { devicePixelRatio: 1 }, matchMedia: () => ({ matches: reduced }),
    getComputedStyle: () => ({ opacity: '0.8', transform: 'matrix(1, 0, 0, 1, 0, 0)' }),
    showWorkInfo() {}, syncFilters() {}, loadHall() {},
    getJson(url, signal) { const done = deferred(); details.push({ ...done, url, signal }); return done.promise; },
  });
  for (const [start, end] of [
    ['function mediaUrl(', 'async function getJson('],
    ['function galleryUrl(', 'function syncFilters('],
    ['function showImage(', 'function showWorkInfo('],
    ['function animateLightbox(', '\n$("search-form")'],
    ['function locationFilters(', 'const state ='],
  ]) {
    assert.ok(source.includes(start) && source.includes(end), `viewer extraction: ${start}`);
    vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), context);
  }
  return { context, state, element, cached, motions, details, history, location, preloaded,
    setReduced(value) { reduced = value; } };
}

{
  const f = fixture(), full = f.element('lb-img'), preview = f.element('lb-preview');
  const cardImage = { complete: true, naturalWidth: 640,
    currentSrc: 'https://gallery.example/media/x/one/0.jpg?width=640' };
  f.cached.add(cardImage.currentSrc);
  const card = { querySelector: () => cardImage,
    getBoundingClientRect: () => ({ x: 10, y: 20, width: 200, height: 300 }) };
  const opening = f.context.openLightbox({ id: 'x:one', cover_url: '/media/x/one/0.jpg' }, card, { push: false });
  assert.equal(preview.src, cardImage.currentSrc);
  assert.equal(preview.hidden, false, 'loaded card preview is available before full decode');
  assert.equal(new URL(full.src).searchParams.get('width'), '1280');
  f.details[0].resolve({ work: { id: 'x:one' }, images: [{ r2_key: 'x/one/0.jpg' }] });
  await opening;
  f.context.showImage(0, false);
  assert.equal(full.assignments.length, 1, 'detail metadata must not restart the same image');
  full.onerror();
  assert.equal(full.assignments.length, 1, 'preview failure must not fetch the original');
  assert.equal(f.element('lb-image-error').hidden, false);
  f.context.showImage(0, false);
  assert.equal(full.hidden, true, 'same-URL metadata refresh preserves the failed image state');
  assert.equal(f.element('lb-image-error').hidden, false);
  f.context.showImage(0, false, true);
  assert.equal(full.assignments.length, 2, 'explicit retry restarts the bounded preview');
  assert.ok(full.assignments.every(url => new URL(url).searchParams.get('width') === '1280'));
  f.motions[0].resolve();
}
{
  const f = fixture();
  f.element('lb-stage').clientWidth = 390;
  f.context.window.devicePixelRatio = 2;
  f.context.showImage(0, false);
  assert.equal(new URL(f.element('lb-img').src).searchParams.get('width'), '640');
}
{
  const f = fixture();
  f.state.images.push({ r2_key: 'x/one/2.jpg' });
  f.context.showImage(1, false);
  assert.deepEqual(f.preloaded.map(url => new URL(url).pathname), ['/media/x/one/2.jpg', '/media/x/one/0.jpg']);
  assert.ok(f.preloaded.every(url => new URL(url).searchParams.get('width') === '1280'), 'neighbours use the viewer size');
}
console.log('PASS: instant card preview, mobile sizing, same-URL reuse, bounded error/retry and neighbour preloading');

{
  const f = fixture(), full = f.element('lb-img'), decoding = deferred();
  f.context.showImage(0, false);
  full.decode = () => decoding.promise;
  const oldError = full.onerror;
  const oldLoad = full.onload();
  f.context.showImage(1, false);
  oldError();
  assert.equal(full.hidden, false, 'an old error cannot hide the next image');
  assert.equal(f.element('lb-image-error').hidden, true);
  decoding.resolve();
  await oldLoad;
  assert.equal(full.classList.contains('ready'), false, 'old decode cannot reveal the next image');
  full.decode = () => Promise.resolve();
  await full.onload();
  assert.equal(full.classList.contains('ready'), true);
  assert.equal(f.element('lb-image-loading').hidden, true);
}
console.log('PASS: late image decode cannot overwrite a newer image request');

{
  const f = fixture(), full = f.element('lb-img');
  f.context.showImage(0, false);
  full.decode = () => Promise.reject(new Error('invalid image'));
  await full.onload();
  assert.equal(f.element('lb-image-loading').hidden, true, 'decode failure cannot leave an endless loader');
  assert.equal(f.element('lb-image-error').hidden, false);
  assert.equal(full.assignments.length, 1, 'decode failure must not fetch the original');
}

{
  const f = fixture(), dialog = f.element('lightbox'), request = new AbortController();
  dialog.open = true; f.state.viewerRequest = request; f.state.imageRequest = {};
  const closing = f.context.dismissLightbox();
  assert.equal(request.signal.aborted, true);
  assert.equal(f.state.imageRequest, null);
  assert.equal(dialog.open, true, 'dialog stays visible during its exit animation');
  f.motions[0].resolve();
  await closing;
  assert.equal(dialog.open, false);
  assert.equal(dialog.closes, 1);
}
{
  const f = fixture(), dialog = f.element('lightbox');
  dialog.open = true;
  const closing = f.context.dismissLightbox(), exit = f.motions[0];
  const opening = f.context.openLightbox({ id: 'x:new' }, null, { push: false });
  assert.equal(exit.cancelled, true);
  await closing;
  assert.equal(dialog.open, true);
  assert.equal(dialog.closes, 0, 'cancelled exit cannot close the reopened viewer');
  assert.equal(dialog.classList.contains('is-closing'), false);
  f.details[0].resolve({ work: { id: 'x:new' }, images: [{ r2_key: 'x/new/0.jpg' }] });
  await opening;
  f.motions.at(-1).resolve();
  await Promise.resolve();
}
{
  const f = fixture(), dialog = f.element('lightbox'), request = new AbortController();
  dialog.open = true; f.state.viewerRequest = request;
  f.history.state = { galleryViewer: true }; f.location.search = '?work=x%3Aone';
  f.context.closeLightbox();
  assert.equal(request.signal.aborted, true, 'closing aborts details before history changes');
  assert.equal(f.history.backs, 1);
  assert.equal(dialog.open, true);
  f.location.search = ''; f.context.syncRoute();
  assert.equal(dialog.open, true);
  f.motions[0].resolve();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(dialog.open, false, 'history navigation finishes the animated dismissal');
}
{
  const f = fixture(), dialog = f.element('lightbox');
  dialog.open = true;
  const closing = f.context.dismissLightbox(), exit = f.motions[0];
  f.location.search = '?work=x%3Aone';
  f.context.syncRoute();
  assert.equal(exit.cancelled, true, 'forward navigation to the same work cancels its exit');
  await closing;
  assert.equal(dialog.open, true);
  assert.equal(dialog.closes, 0);
  f.details[0].resolve({ work: { id: 'x:one' }, images: [{ r2_key: 'x/one/0.jpg' }] });
  await new Promise(resolve => setImmediate(resolve));
  f.motions.at(-1).resolve();
}
{
  const f = fixture(), dialog = f.element('lightbox');
  dialog.open = true; f.setReduced(true);
  await f.context.dismissLightbox();
  assert.equal(dialog.open, false);
  assert.equal(f.motions.length, 0, 'reduced motion closes without an animation wait');
}
console.log('PASS: animated dismissal, immediate cancellation, reopen, history route and reduced motion');
