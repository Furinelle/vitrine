import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../public/glass.js', import.meta.url), 'utf8');
const context = vm.createContext({});
vm.runInContext(source.slice(0, source.indexOf('(() => {')), context);
const frame = { x: 20, y: -15, width: 200, height: 250 };
const fit = (w, h, objectFit) => JSON.parse(JSON.stringify(context.glassImageRect(frame, w, h, objectFit)));
assert.deepEqual(fit(400, 200), { x: 20, y: 60, width: 200, height: 100 });
assert.deepEqual(fit(100, 250), { x: 70, y: -15, width: 100, height: 250 });
assert.deepEqual(fit(200, 250), { ...frame });
assert.deepEqual(fit(400, 200, 'cover'), { x: -130, y: -15, width: 500, height: 250 });
assert.deepEqual(fit(100, 250, 'cover'), { x: 20, y: -140, width: 200, height: 500 });
assert.equal(fit(0, 0), null);

// Exercise the actual renderer across scroll frames, including the browser's buffer discard contract.
const frames = [], draws = [], clears = [], events = new Map();
const transparency = { matches: true, addEventListener: (type, callback) => events.set(`transparency:${type}`, callback) };
let intersection, transform = 'identity', contextOptions, drawingBuffer = false, allocations = 0;
const gl = Object.fromEntries([
  'shaderSource', 'compileShader', 'attachShader', 'deleteShader', 'linkProgram', 'useProgram',
  'bindBuffer', 'bufferData', 'enableVertexAttribArray', 'vertexAttribPointer', 'bindTexture',
  'texParameteri', 'viewport', 'uniform2f', 'uniform1f', 'texImage2D',
].map(name => [name, () => {}]));
for (const name of ['createProgram', 'createShader', 'createBuffer', 'createTexture']) gl[name] = () => ({});
gl.getProgramParameter = () => true;
gl.getAttribLocation = gl.getUniformLocation = () => 0;
gl.drawArrays = () => { drawingBuffer = true; };
const paint = {
  clearRect: (...args) => clears.push({ args, transform }),
  setTransform: () => { transform = 'viewport'; },
  resetTransform: () => { transform = 'identity'; },
  save() {}, restore() {}, beginPath() {}, roundRect() {}, clip() {}, fillRect() {},
  drawImage: (...args) => draws.push(args.slice(1)),
};
const lens = {
  getContext: (kind, options) => { contextOptions = options; return gl; },
  setAttribute() {}, addEventListener() {},
};
for (const name of ['width', 'height']) Object.defineProperty(lens, name, {
  get() { return this[`_${name}`]; },
  set(value) { this[`_${name}`] = value; allocations++; drawingBuffer = false; },
});
const sample = { getContext: () => paint };
let canvasCount = 0;
const imageFrame = {
  getBoundingClientRect: () => ({ ...frame, left: frame.x, right: frame.x + frame.width, top: frame.y, bottom: frame.y + frame.height }),
};
const img = { parentElement: imageFrame, isConnected: true, complete: true, naturalWidth: 400, naturalHeight: 200 };
let objectFit = 'cover';
const surface = {
  getBoundingClientRect: () => ({ left: 0, top: 0, right: 300, bottom: 100, width: 300, height: 100 }),
  prepend() {},
};
vm.runInNewContext(source, {
  matchMedia: () => transparency,
  document: {
    hidden: false, documentElement: {}, querySelectorAll: () => [surface, {
      getBoundingClientRect: () => ({ left: 0, top: 700, right: 300, bottom: 800, width: 300, height: 100 }),
    }],
    createElement: () => canvasCount++ === 0 ? lens : sample,
    getElementById: id => id === 'grid' ? { querySelectorAll: () => [img] } : null,
    addEventListener: (type, callback) => events.set(`document:${type}`, callback),
  },
  getComputedStyle: target => target === img ? { objectFit } : { borderRadius: '12px', backgroundColor: '#000' },
  requestAnimationFrame: callback => frames.push(callback),
  IntersectionObserver: class { constructor(callback) { intersection = callback; } observe() {} },
  ResizeObserver: class { observe() {} },
  MutationObserver: class { observe() {} },
  window: { addEventListener: (type, callback) => events.set(type, callback) },
  visualViewport: { addEventListener() {} }, innerHeight: 800, devicePixelRatio: 1.5,
});
assert.equal(canvasCount, 0, 'startup must not create GPU contexts before the app can request works');
intersection([{ target: img, isIntersecting: true }]);
frames.shift()();
assert.equal(canvasCount, 0, 'initial reduced transparency must keep native CSS without GPU work');
transparency.matches = false;
img.complete = false;
intersection([{ target: img, isIntersecting: true }]);
frames.shift()();
assert.equal(canvasCount, 0, 'visible but unloaded images need no glass renderer');
img.complete = true;
frame.y = 200;
events.get('document:load')();
frames.shift()();
assert.equal(canvasCount, 0, 'loaded images outside the glass sample area need no GPU context');
frame.y = -15;
events.get('scroll')();
frames.shift()();
assert.equal(canvasCount, 2, 'first backdrop creates just one reusable lens and sample canvas');
assert.deepEqual(draws.at(-1), [-130, -15, 500, 250]);
assert.equal(drawingBuffer, true);
// A non-preserved WebGL drawing buffer is cleared after presentation, even with no new RAF.
if (!contextOptions.preserveDrawingBuffer) drawingBuffer = false;
assert.equal(drawingBuffer, true, 'the last glass frame must survive presentation between scroll events');
objectFit = 'contain';
frame.y -= 20;
events.get('scroll')();
events.get('scroll')();
assert.equal(frames.length, 1, 'continuous scroll events share one pending frame');
frames.shift()();
assert.deepEqual(draws.at(-1), [20, 40, 200, 100]);
assert.equal(allocations, 2, 'scrolling must not reset the lens canvas dimensions');
assert.ok(clears.every(clear => clear.transform === 'identity'));
assert.equal(lens.hidden, false);
transparency.matches = true;
events.get('transparency:change')();
frames.shift()();
assert.equal(lens.hidden, true, 'reduced transparency hides an existing refraction canvas');
transparency.matches = false;
events.get('transparency:change')();
frames.shift()();
assert.equal(lens.hidden, false);
assert.equal(canvasCount, 2, 'scrolls and preference changes must reuse the GPU renderer');
intersection([{ target: img, isIntersecting: false }]);
frames.shift()();
assert.equal(lens.hidden, true, 'native CSS handles surfaces with no gallery backdrop');
console.log('PASS: lazy GPU initialization, cover/contain sampling, retained frames, coalesced redraws and transparency fallback');
