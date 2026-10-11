// URL handling is a trust boundary: catalog metadata must never create executable links.
// Run with: node tools/test_frontend.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../public/app.js', import.meta.url), 'utf8');
const context = vm.createContext({ URL, URLSearchParams, location: { origin: 'https://gallery.example' } });
vm.runInContext(source.slice(source.indexOf('function safeLink('), source.indexOf('async function getJson(')), context);
for (const value of ['javascript:alert(1)', 'data:text/html,test', '//evil.example', '', null]) {
  assert.equal(context.safeLink(value), '');
}
assert.equal(context.safeLink('https://www.pixiv.net/artworks/123'), 'https://www.pixiv.net/artworks/123');
assert.equal(context.coverUrl('https://evil.example/media/a.png'), '');
assert.equal(context.coverUrl('/media/pixiv/123/a.png'), 'https://gallery.example/media/pixiv/123/a.png');
assert.equal(context.thumbUrl('https://evil.example/media/a.png'), '');
assert.equal(context.thumbUrl('/media/pixiv/123/a.png'), 'https://gallery.example/media/pixiv/123/a.png?width=640');
assert.equal(context.thumbUrl('/media/pixiv/123/a.png', 160), 'https://gallery.example/media/pixiv/123/a.png?width=160');
assert.equal(context.mediaUrl('pixiv/123/a #?.png'), '/media/pixiv/123/a%20%23%3F.png');
console.log('PASS: only HTTP source links and same-origin media, escaped object keys');

// Filtering from a dialog removes its opener; focus must land on stable content.
const events = [];
const dialog = { open: true, close() { this.open = false; events.push('close'); } };
let activateTag;
context.$ = (id) => ({ lightbox: dialog, collection: { focus() { events.push('focus'); } } })[id];
context.button = (_className, _text, action) => { activateTag = action; return { dataset: {} }; };
context.dismissLightbox = async () => dialog.close();
context.applyFilters = ({ tag }) => { assert.equal(tag, 'art'); events.push('filter'); };
context.state = { viewerRequest: null };
vm.runInContext(source.slice(source.indexOf('function tagButton('), source.indexOf('const coverObserver =')), context);
context.tagButton('art');
await activateTag();
assert.deepEqual(events, ['close', 'filter', 'focus']);
events.length = 0;
await activateTag();
assert.deepEqual(events, ['filter']);
console.log('PASS: dialog tag filtering restores stable focus; sidebar tag filtering leaves focus alone');

// Filter metadata renders independently while preserving unsubmitted search/tag text.
function filtersFixture() {
  const elements = new Map(), pending = new Map();
  function element(id) {
    if (!elements.has(id)) elements.set(id, {
      children: [], dataset: {}, value: '', open: false,
      classList: { toggle() {} }, setAttribute() {}, removeAttribute() {}, addEventListener() {},
      append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; },
    });
    return elements.get(id);
  }
  const state = { sources: [], tags: [], works: [], source: '', tag: '', q: '' };
  const filters = vm.createContext({
    state, names: { x: 'X' }, $: element,
    document: { createElement: () => element(Symbol()), querySelectorAll: () => [] },
    Option: class { constructor(text, value) { this.text = text; this.value = value; } },
    applyFilters() {}, tagButton: name => ({ dataset: { tag: name } }),
    getJson(url) { return new Promise((resolve, reject) => pending.set(url, { resolve, reject })); },
  });
  for (const [start, end] of [
    ['function node(', 'function safeLink('],
    ['function syncFilters(', 'function applyFilters('],
    ['async function loadFilters(', 'async function loadTagBrowser('],
  ]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), filters);
  element('q').value = '未提交搜索';
  element('tag').value = '未提交标签';
  return { filters, state, element, pending };
}
{
  const f = filtersFixture(), loading = f.filters.loadFilters();
  assert.deepEqual([...f.pending.keys()], ['/api/sources', '/api/tags'], 'both metadata requests start together');
  f.state.source = 'x';
  f.pending.get('/api/sources').resolve({ sources: [{ source: 'x', cnt: 12 }] });
  await Promise.resolve();
  assert.equal(f.element('sources').children.length, 2, 'slow tags must not block source navigation');
  assert.equal(f.element('collection-count').textContent, '12 件收藏', 'source counts render before tags settle');
  assert.equal(f.element('source').value, 'x', 'late metadata uses the current filter');
  assert.equal(f.element('popular-tags').children.length, 0);
  assert.equal(f.element('q').value, '未提交搜索');
  assert.equal(f.element('tag').value, '未提交标签');
  f.pending.get('/api/tags').resolve({ tags: [{ name: 'art', cnt: 4 }] });
  await loading;
  assert.equal(f.element('popular-tags').children[0].dataset.tag, 'art');
  assert.equal(f.element('q').value, '未提交搜索', 'tag completion also preserves the search draft');
  assert.equal(f.element('tag').value, '未提交标签');
}
{
  const f = filtersFixture(), loading = f.filters.loadFilters();
  f.pending.get('/api/sources').reject(new Error('source service unavailable'));
  f.pending.get('/api/tags').resolve({ tags: [{ name: 'still-available', cnt: 2 }] });
  await loading;
  assert.equal(f.element('taglist').children[0].value, 'still-available');
  assert.equal(f.element('popular-tags').children[0].dataset.tag, 'still-available');
  assert.equal(f.element('q').value, '未提交搜索');
  assert.equal(f.element('tag').value, '未提交标签');
}
console.log('PASS: independent metadata rendering, failure isolation, current filters and preserved drafts');

// Exercise actual card rendering and viewport loading without fetching images.
function cardsFixture(withObserver = true) {
  const observed = new Set();
  let intersect, observerOptions, disconnected = 0, unobserved = 0;
  function element(tag) {
    const classes = new Set();
    return {
      tagName: tag, children: [], dataset: {}, attributes: {}, assignments: [], isConnected: true,
      classList: { add(name) { classes.add(name); }, contains(name) { return classes.has(name); } },
      setAttribute(name, value) { this.attributes[name] = value; },
      addEventListener() {},
      append(...items) { for (const item of items) this.children.push(...(item.tagName === 'fragment' ? item.children : [item])); },
      replaceChildren(...items) { this.children = []; this.append(...items); },
      get src() { return this.assignments.at(-1) || ''; },
      set src(value) { this.assignments.push(value); },
    };
  }
  const grid = element('section');
  const cards = vm.createContext({
    URL, URLSearchParams, location: { origin: 'https://gallery.example' },
    names: { x: 'X' }, $: () => grid, galleryUrl: id => `/?work=${id}`,
    applyFilters() {}, openLightbox() {},
    document: { createElement: element, createDocumentFragment: () => element('fragment') },
    IntersectionObserver: withObserver ? class {
      constructor(callback, options) { intersect = callback; observerOptions = options; }
      observe(image) { observed.add(image); }
      unobserve(image) { observed.delete(image); unobserved++; }
      disconnect() { observed.clear(); disconnected++; }
    } : undefined,
  });
  for (const [start, end] of [
    ['function node(', 'function safeLink('],
    ['function safeLink(', 'async function getJson('],
    ['const coverObserver =', 'const moreObserver ='],
  ]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), cards);
  const works = Array.from({ length: 6 }, (_, i) => ({
    id: `x:${i}`, title: `Work ${i}`, source: 'x', page_count: 1, cover_url: `/media/x/${i}/0.jpg`,
  }));
  return { cards, grid, works, observed, get intersect() { return intersect; },
    get observerOptions() { return observerOptions; }, get disconnected() { return disconnected; },
    get unobserved() { return unobserved; }, images: () => grid.children.map(card => card.children[0].children[0]) };
}
{
  const f = cardsFixture();
  f.cards.renderCards(f.works, false);
  const images = f.images();
  assert.equal(f.observerOptions.rootMargin, '360px 0px');
  for (const image of images.slice(0, 4)) {
    assert.equal(image.loading, 'eager');
    assert.equal(new URL(image.src).searchParams.get('width'), '640');
  }
  assert.equal(images[0].fetchPriority, 'high');
  assert.equal(images[1].fetchPriority, 'high');
  assert.equal(images[2].fetchPriority, undefined);
  for (const image of images.slice(4)) {
    assert.equal(image.loading, 'lazy');
    assert.equal(image.src, '', 'distant covers must not initiate a request');
    assert.ok(f.observed.has(image));
  }
  const image = images[4], thumbnail = image.dataset.src;
  f.intersect([{ target: image, isIntersecting: false }]);
  assert.equal(image.assignments.length, 0);
  f.intersect([{ target: image, isIntersecting: true }]);
  f.intersect([{ target: image, isIntersecting: true }]);
  assert.deepEqual(image.assignments, [thumbnail], 'intersection loads a bounded thumbnail exactly once');
  assert.equal(image.dataset.src, undefined);
  assert.equal(f.unobserved, 1);
  image.onerror();
  assert.deepEqual(image.assignments, [thumbnail], 'thumbnail failure must not download the original');
  assert.equal(f.grid.children[4].children[0].classList.contains('image-broken'), true);
  f.cards.renderLoadingCards();
  assert.equal(f.disconnected, 1);
  assert.equal(f.observed.size, 0, 'reset must release old card observers');
  const staleImage = images[5];
  staleImage.isConnected = false;
  f.intersect([{ target: staleImage, isIntersecting: true }]);
  assert.equal(staleImage.assignments.length, 0, 'queued callbacks after a filter reset must not request detached covers');
  assert.equal(f.grid.children.length, 8);
  for (const placeholder of f.grid.children) {
    assert.equal(placeholder.className, 'card-skeleton');
    assert.equal(placeholder.attributes['aria-hidden'], 'true');
  }
}
{
  const f = cardsFixture(false);
  f.cards.renderCards(f.works, false);
  assert.equal(f.images()[4].loading, 'lazy');
  for (const image of f.images()) assert.equal(new URL(image.src).searchParams.get('width'), '640');
  f.cards.renderLoadingCards();
  assert.equal(f.grid.children.length, 8, 'loading placeholders also work without IntersectionObserver');
}
console.log('PASS: eager first covers, bounded viewport requests, no original fallback, native lazy fallback and accessible placeholders');

// Auto-pagination must re-arm after success, serialize requests, and stop on error/end.
const elements = Object.fromEntries(['grid', 'status', 'retry', 'empty', 'end-note', 'more'].map(id =>
  [id, { hidden: true, children: [], classList: { add() {}, remove() {} }, setAttribute() {}, replaceChildren(...children) { this.children = children; } }]));
const paging = { works: [], offset: 0, limit: 2, loading: false, done: false };
let intersect, watched = false, completePage;
const requests = [];
const pagination = vm.createContext({
  state: paging, URLSearchParams, AbortController, $: id => elements[id],
  renderLoadingCards() { elements.grid.replaceChildren('skeleton'); },
  renderCards(works, append) {
    if (!append) elements.grid.replaceChildren();
    elements.grid.children.push(...works);
  },
  updateCount() {},
  getJson(url) { requests.push(url); return new Promise((resolve, reject) => { completePage = { resolve, reject }; }); },
  IntersectionObserver: class {
    constructor(callback) { intersect = callback; }
    observe() { watched = true; }
    unobserve() { watched = false; }
  },
});
vm.runInContext(source.slice(source.indexOf('const moreObserver ='), source.indexOf('function showImage(')), pagination);
let loading = pagination.loadWorks(true);
completePage.resolve({ works: [{ id: 1 }, { id: 2 }] });
await loading;
assert.equal(watched, true);
await intersect([{ isIntersecting: false }]);
assert.equal(requests.length, 1);
loading = intersect([{ isIntersecting: true }]);
await intersect([{ isIntersecting: true }]);
assert.equal(requests.length, 2);
assert.match(requests[1], /offset=2/);
assert.equal(watched, false);
completePage.reject(new Error('offline'));
await loading;
await intersect([{ isIntersecting: true }]);
assert.equal(requests.length, 2);
assert.equal(elements.retry.hidden, false);
assert.equal(watched, false);
loading = pagination.loadWorks();
assert.match(requests[2], /offset=2/);
completePage.resolve({ works: [{ id: 3 }, { id: 4 }] });
await loading;
assert.equal(watched, true);
loading = intersect([{ isIntersecting: true }]);
assert.match(requests[3], /offset=4/);
completePage.resolve({ works: [{ id: 5 }] });
await loading;
assert.equal(paging.works.length, 5);
assert.equal(paging.done, true);
assert.equal(elements.more.hidden, true);
assert.equal(watched, false);
await intersect([{ isIntersecting: true }]);
assert.equal(requests.length, 4);
console.log('PASS: automatic paging, concurrent trigger guard, error pause/retry, re-arm and end-of-results');
loading = pagination.loadWorks(true);
assert.deepEqual(elements.grid.children, ['skeleton']);
completePage.reject(new Error('offline'));
await loading;
assert.deepEqual(elements.grid.children, [], 'failed initial loading must remove skeleton cards');
assert.equal(elements.retry.hidden, false);
assert.equal(paging.loading, false);
console.log('PASS: initial load failure clears placeholders and allows retry');


const adminSource = readFileSync(new URL('../public/admin.js', import.meta.url), 'utf8');
const admin = vm.createContext({});
vm.runInContext(
  adminSource.slice(adminSource.indexOf('function parseTags('), adminSource.indexOf('function node(')),
  admin,
);
assert.equal(admin.parseTags('  #foo, bar，baz  ').join(','), 'foo,bar,baz');
assert.equal(admin.parseTags('foo, foo').join(','), 'foo');
assert.equal(admin.workApiPath('pixiv:123'), '/api/admin/works/pixiv%3A123');
assert.equal(admin.workApiPath('pixiv:123', '/hide'), '/api/admin/works/pixiv%3A123/hide');
console.log('PASS: admin tag parse and work id encoding');

// Admin SPA routing keeps the shared navigation and work filters in sync.
const elementMap = {
  'nav-works': { classList: { toggle(c, f) { if (f) this[c] = true; else delete this[c]; } }, setAttribute(k, v) { this[k] = v; } },
  'nav-similar': { classList: { toggle(c, f) { if (f) this[c] = true; else delete this[c]; } }, setAttribute(k, v) { this[k] = v; } },
  'view-works-section': { hidden: false },
  'similar': { hidden: true },
  'sidebar-sources-section': { hidden: false },
  'search-form': { style: {} },
};
const routingContext = vm.createContext({
  window: { location: { pathname: '/admin/similar' }, addEventListener() {} },
  document: { title: '' },
  $: (id) => elementMap[id] || null,
  state: { view: 'works', works: [], similar: [], loading: false },
  loadSimilar() {},
  loadWorks() {},
});
vm.runInContext(
  adminSource.slice(adminSource.indexOf('function setView('), adminSource.indexOf('/* =====================================================================\n   相似图审批相关渲染与操作')),
  routingContext,
);

routingContext.syncRoute();
assert.equal(routingContext.state.view, 'similar');
assert.equal(routingContext.document.title, 'Vitrine · 相似图审批');
assert.equal(elementMap['view-works-section'].hidden, true);
assert.equal(elementMap['similar'].hidden, false);
assert.equal(elementMap['nav-similar'].classList.active, true);
assert.equal(elementMap['sidebar-sources-section'].hidden, true);
assert.equal(elementMap['search-form'].hidden, true);

routingContext.window.location.pathname = '/admin';
routingContext.syncRoute();
assert.equal(routingContext.state.view, 'works');
assert.equal(routingContext.document.title, 'Vitrine · 作品管理');
assert.equal(elementMap['view-works-section'].hidden, false);
assert.equal(elementMap['similar'].hidden, true);
assert.equal(elementMap['nav-works'].classList.active, true);
assert.equal(elementMap['sidebar-sources-section'].hidden, false);
assert.equal(elementMap['search-form'].hidden, false);
console.log('PASS: admin SPA routing /admin and /admin/similar');

// Shared links preserve filters and image position, and never concatenate raw metadata.
const urls = vm.createContext({ URLSearchParams, state: { source: 'x', tag: 'a & b', q: '<title>' } });
vm.runInContext(source.slice(source.indexOf('function galleryUrl('), source.indexOf('function syncFilters(')), urls);
const shared = new URL(urls.galleryUrl('x:123?evil#', 2), 'https://gallery.example');
assert.equal(shared.searchParams.get('work'), 'x:123?evil#');
assert.equal(shared.searchParams.get('page'), '3');
assert.equal(shared.searchParams.get('tag'), 'a & b');
assert.equal(new URL(urls.galleryUrl(), shared).searchParams.has('work'), false);
assert.equal(context.thumbUrl('/media/a.png', 9000), '');
assert.equal(context.safeLink('https://user:password@example.com'), '');
console.log('PASS: share URL round-trip and bounded thumbnail presets');

// Filtering during an in-flight page must not append the stale response.
const pendingPages = [];
pagination.getJson = url => new Promise(resolve => pendingPages.push({ url, resolve }));
const oldPage = pagination.loadWorks(true);
const newPage = pagination.loadWorks(true);
pendingPages[1].resolve({ works: [{ id: 'new' }] });
await newPage;
pendingPages[0].resolve({ works: [{ id: 'old' }] });
await oldPage;
assert.equal(paging.works.map(work => work.id).join(','), 'new');
assert.equal(paging.loading, false);
console.log('PASS: stale pages cannot overwrite a newer filter');

// dialog.close() queues its close event. A detail response may resolve first,
// so filtering must cancel it synchronously and never rewrite the new URL.
const viewerElements = new Map();
function viewerElement(id) {
  if (!viewerElements.has(id)) viewerElements.set(id, {
    open: false, hidden: false, textContent: '', style: {},
    classList: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
    getBoundingClientRect() { return {x:0,y:0,width:1000,height:700}; },
    setAttribute() {}, removeAttribute() {}, replaceChildren() {},
    querySelectorAll() { return []; },
    showModal() { this.open = true; },
    close() { this.open = false; }, // The queued close listener has not run yet.
  });
  return viewerElements.get(id);
}
const viewerState = { source: '', tag: '', q: '', viewerRequest: null };
const viewerRoutes = [];
const viewerHistory = {
  state: null,
  pushState(state, _title, url) { this.state = state; viewerRoutes.push(url); },
  replaceState(state, _title, url) { this.state = state; viewerRoutes.push(url); },
};
let completeDetail;
const viewer = vm.createContext({
  URL, URLSearchParams, AbortController,
  location: { origin: 'https://gallery.example' }, history: viewerHistory,
  state: viewerState, $: viewerElement, window: { scrollTo() {} },
  animateLightbox() {}, dismissLightbox() { viewerElement('lightbox').close(); },
  getJson() { return new Promise(resolve => { completeDetail = resolve; }); },
  showWorkInfo() {}, syncFilters() {}, loadWorks() {},
});
for (const [start, end] of [
  ['function mediaUrl(', 'async function getJson('],
  ['function galleryUrl(', 'function syncFilters('],
  ['function applyFilters(', 'async function loadFilters('],
  ['function showImage(', 'function showWorkInfo('],
  ['async function openLightbox(', 'function syncRoute('],
]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), viewer);
const pendingDetail = viewer.openLightbox({ id: 'pixiv:123' }, null);
const detailController = viewerState.viewerRequest;
viewer.applyFilters({ tag: 'new-tag' });
completeDetail({ work: { id: 'pixiv:123' }, images: [{ r2_key: 'pixiv/123/0.png' }] });
await pendingDetail;
assert.equal(viewerElement('lightbox').open, false);
assert.equal(detailController.signal.aborted, true);
assert.equal(viewerState.viewerRequest, null);
assert.deepEqual(viewerRoutes, ['/?work=pixiv%3A123', '/?tag=new-tag']);
assert.equal(viewerRoutes.at(-1).includes('work='), false);
console.log('PASS: closing during a detail request cannot rewrite the new filter URL');

// The server cursor, not a moving row offset, carries the next browse request.
const cursorPages = [];
pagination.getJson = url => new Promise(resolve => cursorPages.push({url, resolve}));
const cursorFirst = pagination.loadWorks(true);
cursorPages[0].resolve({works:[{id:'cursor-a'},{id:'cursor-b'}],next_cursor:'["date","cursor-b"]'});
await cursorFirst;
const cursorNext = pagination.loadWorks();
assert.equal(new URL(cursorPages[1].url,'https://gallery.example').searchParams.get('cursor'),'["date","cursor-b"]');
assert.equal(new URL(cursorPages[1].url,'https://gallery.example').searchParams.has('offset'),false);
cursorPages[1].resolve({works:[{id:'cursor-c'}],next_cursor:null});
await cursorNext;
assert.equal(paging.done,true);
console.log('PASS: server cursor drives pagination and terminal state');

// Shared theme boots before either page script, without their $ helper.
const themeSource = readFileSync(new URL('../public/theme.js', import.meta.url), 'utf8');
for (const saved of [null, 'dark', 'light', 'invalid', 'blocked']) for (const withViewport of [false, true]) {
  const root = { style: {} }, meta = {}, toggle = { label: '', click: null };
  const input = { addEventListener() {} };
  const toggleElement = {
    setAttribute(_name, value) { toggle.label = value; },
    addEventListener(_name, callback) { toggle.click = callback; },
  };
  const boot = vm.createContext({
    window: { scrollY: 0, addEventListener() {}, visualViewport: withViewport ? { addEventListener() {} } : undefined },
    document: { documentElement: root, querySelector: () => meta,
      getElementById: id => id === 'theme-toggle' ? toggleElement : ['q', 'back-to-top'].includes(id) ? input : null },
    localStorage: { getItem() { if (saved === 'blocked') throw new Error('blocked'); return saved; }, setItem() {} },
  });
  vm.runInContext(themeSource, boot);
  assert.equal(root.style.colorScheme, saved === 'dark' ? 'dark' : 'light');
  toggle.click();
  assert.equal(root.style.colorScheme, saved === 'dark' ? 'light' : 'dark');
}
console.log('PASS: shared theme boots independently, honors saved choices, toggles and tolerates blocked storage');

// Floating controls keep draft text while the software keyboard pans the viewport.
const scrollEvents = new Map(), viewportEvents = new Map(), scrollCalls = [];
let focusedOptions, viewportOffset;
const scrollElements = {
  q: { value: '未提交的搜索', addEventListener(type, cb) { scrollEvents.set(`q:${type}`, cb); } },
  'back-to-top': { hidden: true, addEventListener(type, cb) { scrollEvents.set(`top:${type}`, cb); } },
  collection: { focus(options) { focusedOptions = options; } },
};
const scrollWindow = {
  scrollY: 0,
  scrollTo(options) { scrollCalls.push(options); },
  addEventListener(type, cb) { scrollEvents.set(type, cb); },
  visualViewport: { scale: 1, offsetTop: 140, addEventListener(type, cb) { viewportEvents.set(type, cb); } },
};
const scrollDocument = {
  activeElement: scrollElements.q,
  getElementById: id => scrollElements[id],
  querySelector(selector) { return selector === "main" ? scrollElements.collection : { style: { setProperty(_name, value) { viewportOffset = value; } } }; },
};
const controls = vm.createContext({ window: scrollWindow, document: scrollDocument, $: id => scrollElements[id] });
vm.runInContext(themeSource.slice(themeSource.indexOf('function syncSearchViewport(')), controls);
assert.equal(scrollElements['back-to-top'].hidden, true);
scrollWindow.scrollY = 1200;
scrollEvents.get('scroll')();
assert.equal(scrollElements['back-to-top'].hidden, false);
viewportEvents.get('scroll')();
assert.equal(viewportOffset, '140px');
assert.equal(scrollElements.q.value, '未提交的搜索');
scrollWindow.visualViewport.offsetTop = -10;
scrollEvents.get('q:focus')();
assert.equal(viewportOffset, '0px', 'viewport offset cannot move the header above its anchor');
scrollWindow.visualViewport.offsetTop = 140;
scrollWindow.visualViewport.scale = 2;
viewportEvents.get('resize')();
assert.equal(viewportOffset, '0px', 'pinch zoom must retain its native pan');
scrollWindow.visualViewport.scale = 1;
viewportEvents.get('resize')();
assert.equal(viewportOffset, '140px', 'keyboard pan resumes after pinch zoom ends');
scrollDocument.activeElement = null;
scrollEvents.get('q:blur')();
assert.equal(viewportOffset, '0px');
scrollEvents.get('top:click')();
assert.equal(focusedOptions.preventScroll, true);
assert.equal(scrollCalls.at(-1).top, 0);
assert.equal(scrollCalls.at(-1).behavior, 'auto', 'CSS preserves reduced-motion preference');
scrollWindow.scrollY = 0;
scrollEvents.get('scroll')();
assert.equal(scrollElements['back-to-top'].hidden, true);
scrollWindow.visualViewport = undefined;
controls.syncSearchViewport();
assert.equal(viewportOffset, '0px');
console.log('PASS: back-to-top visibility/focus, keyboard viewport offset, draft preservation and zoom fallback');

// A platform error page must produce a readable message, while auth still expires normally.
const requestFailure = vm.createContext({ fetch: async () => ({
  ok: false, status: 500, headers: new Headers(), json: async () => { throw new Error('HTML response'); },
}) });
vm.runInContext(adminSource.slice(adminSource.indexOf('async function send('), adminSource.indexOf('const state =')), requestFailure);
await assert.rejects(requestFailure.send('/api/admin/similar'), /暂时无法加载/);
requestFailure.fetch = async () => ({ ok: false, status: 429, headers: new Headers(), json: async () => { throw new Error('HTML response'); } });
await assert.rejects(requestFailure.send('/api/admin/similar'), /请求太频繁/);
requestFailure.fetch = async () => ({ ok: false, status: 401, headers: new Headers(), json: async () => { throw new Error('HTML response'); } });
await assert.rejects(requestFailure.send('/api/admin/similar'), error => error.status === 401 && error.message === 'unauthorized');
requestFailure.fetch = async () => ({ ok: false, status: 503, headers: new Headers({ 'Retry-After': '3600' }),
  json: async () => ({ ok: false, code: 'database_daily_limit', error: '数据库今日访问额度已用完。' }) });
await assert.rejects(requestFailure.send('/api/admin/similar'), error => error.status === 503 &&
  error.code === 'database_daily_limit' && error.retryAfter === 3600 && error.message === '数据库今日访问额度已用完。');
console.log('PASS: non-JSON errors stay readable, auth expires and daily quota metadata survives');
