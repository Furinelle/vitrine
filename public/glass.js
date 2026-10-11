function glassImageRect(frame, imageWidth, imageHeight, objectFit = 'contain') {
  if (!imageWidth || !imageHeight) return null;
  const scale = (objectFit === 'cover' ? Math.max : Math.min)(frame.width / imageWidth, frame.height / imageHeight);
  const width = imageWidth * scale, height = imageHeight * scale;
  return { x: frame.x + (frame.width - width) / 2, y: frame.y + (frame.height - height) / 2, width, height };
}

(() => {
  const reduced = matchMedia('(prefers-reduced-transparency: reduce), (prefers-contrast: more), (forced-colors: active)');
  const vertex = 'attribute vec2 position; varying vec2 uv; void main(){ uv=(position+1.0)*0.5; gl_Position=vec4(position,0.0,1.0); }';
  const fragment = `
    precision mediump float;
    uniform sampler2D backdrop;
    uniform vec2 size;
    uniform vec2 textureSize;
    uniform float radius;
    varying vec2 uv;
    void main() {
      vec2 p=vec2(uv.x,1.0-uv.y)*size;
      vec2 center=p-size*0.5;
      vec2 q=abs(center)-(size*0.5-vec2(radius));
      vec2 corner=max(q,0.0);
      float distance=length(corner)+min(max(q.x,q.y),0.0)-radius;
      vec2 normal=length(corner)>0.001 ? normalize(corner)*sign(center)
        : (q.x>q.y ? vec2(sign(center.x),0.0) : vec2(0.0,sign(center.y)));
      float rim=1.0-smoothstep(0.0,min(18.0,radius*0.7),-distance);
      vec2 samplePoint=size*0.5+center*0.96-normal*rim*rim*24.0;
      vec2 coordinate=(samplePoint+32.0)/textureSize;
      vec2 blur=vec2(0.65)/textureSize;
      vec4 color=texture2D(backdrop,coordinate)*0.4;
      color+=texture2D(backdrop,coordinate+vec2(blur.x,blur.y))*0.15;
      color+=texture2D(backdrop,coordinate+vec2(-blur.x,blur.y))*0.15;
      color+=texture2D(backdrop,coordinate+vec2(blur.x,-blur.y))*0.15;
      color+=texture2D(backdrop,coordinate-blur)*0.15;
      float coverage=1.0-smoothstep(-0.8,0.0,distance);
      gl_FragColor=vec4(color.rgb,color.a*coverage*0.94);
    }`;
  const visible = new Set();
  let pending = false;
  const lenses = [...document.querySelectorAll('.liquid-glass')].map(surface => ({ surface, initialized: false, renderer: null }));
  function schedule() {
    if (pending || document.hidden) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      if (reduced.matches) {
        for (const lens of lenses) if (lens.renderer) lens.renderer.canvas.hidden = true;
        return;
      }
      const images = [...visible].filter(img => img.isConnected && img.complete && img.naturalWidth).map(img => {
        const frame = img.parentElement;
        const rect = frame.getBoundingClientRect();
        const style = getComputedStyle(frame);
        return { img, rect, radius: parseFloat(style.borderRadius), background: style.backgroundColor, objectFit: getComputedStyle(img).objectFit };
      });
      for (const lens of lenses) {
        const rect = lens.surface.getBoundingClientRect();
        if (!rect.width || rect.bottom < 0 || rect.top > innerHeight) continue;
        const backdrop = images.filter(({ rect: frame }) => frame.right >= rect.left - 32 && frame.left <= rect.right + 32 && frame.bottom >= rect.top - 32 && frame.top <= rect.bottom + 32);
        if (!backdrop.length) {
          if (lens.renderer) lens.renderer.canvas.hidden = true;
          continue;
        }
        // Compile only when a loaded gallery image needs refraction.
        if (!lens.initialized) {
          lens.initialized = true;
          lens.renderer = createLens(lens.surface);
        }
        lens.renderer?.draw(backdrop, rect);
      }
    });
  }
  function createLens(surface) {
    const canvas = document.createElement('canvas');
    canvas.className = 'glass-lens';
    canvas.setAttribute('aria-hidden', 'true');
    // Keep the last frame visible between event-driven draws and compositor scrolls.
    const gl = canvas.getContext('webgl', { alpha: true, premultipliedAlpha: false, preserveDrawingBuffer: true, antialias: false, depth: false, stencil: false });
    if (!gl) return null;
    const program = gl.createProgram();
    for (const [type, code] of [[gl.VERTEX_SHADER, vertex], [gl.FRAGMENT_SHADER, fragment]]) {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, code);
      gl.compileShader(shader);
      gl.attachShader(program, shader);
      gl.deleteShader(shader);
    }
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
    gl.useProgram(program);
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1, 1,-1, -1,1, -1,1, 1,-1, 1,1]), gl.STATIC_DRAW);
    const position = gl.getAttribLocation(program, 'position');
    gl.enableVertexAttribArray(position);
    gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
    gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    const sizeUniform = gl.getUniformLocation(program, 'size');
    const textureUniform = gl.getUniformLocation(program, 'textureSize');
    const radiusUniform = gl.getUniformLocation(program, 'radius');
    const sample = document.createElement('canvas');
    const paint = sample.getContext('2d');
    if (!paint) return null;
    surface.prepend(canvas);
    let lost = false;
    canvas.addEventListener('webglcontextlost', () => { lost = true; canvas.hidden = true; });
    // ponytail: sample only gallery images; arbitrary DOM text keeps the native CSS blur.
    return { canvas, draw(images, rect) {
      if (lost) return;
      const ratio = Math.min(devicePixelRatio || 1, 1.5);
      const width = Math.round(rect.width * ratio), height = Math.round(rect.height * ratio);
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width; canvas.height = height;
        sample.width = Math.round((rect.width + 64) * ratio);
        sample.height = Math.round((rect.height + 64) * ratio);
      }
      paint.clearRect(0, 0, sample.width, sample.height);
      paint.setTransform(ratio, 0, 0, ratio, (32 - rect.left) * ratio, (32 - rect.top) * ratio);
      for (const image of images) {
        const frame = image.rect;
        const fitted = glassImageRect(frame, image.img.naturalWidth, image.img.naturalHeight, image.objectFit);
        paint.save();
        paint.beginPath();
        paint.roundRect(frame.x, frame.y, frame.width, frame.height, image.radius);
        paint.clip();
        paint.fillStyle = image.background;
        paint.fillRect(frame.x, frame.y, frame.width, frame.height);
        paint.drawImage(image.img, fitted.x, fitted.y, fitted.width, fitted.height);
        paint.restore();
      }
      paint.resetTransform();
      canvas.hidden = false;
      gl.viewport(0, 0, width, height);
      gl.uniform2f(sizeUniform, rect.width, rect.height);
      gl.uniform2f(textureUniform, rect.width + 64, rect.height + 64);
      gl.uniform1f(radiusUniform, Math.min(rect.width, rect.height) / 2);
      try { gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, sample); }
      catch { lost = true; canvas.hidden = true; return; }
      gl.drawArrays(gl.TRIANGLES, 0, 6);
    } };
  }
  for (const { surface } of lenses) new ResizeObserver(schedule).observe(surface);
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) entry.isIntersecting ? visible.add(entry.target) : visible.delete(entry.target);
    schedule();
  }, { rootMargin: '120px' });
  const galleries = ['grid', 'similar-groups', 'image-strip'].map(id => document.getElementById(id)).filter(Boolean);
  const watch = node => node.querySelectorAll?.('.art-button img, .admin-card-thumb img, .admin-image-thumb img, .similar-thumb img') || [];
  for (const grid of galleries) for (const img of watch(grid)) observer.observe(img);
  const mutationObserver = new MutationObserver(records => {
    for (const record of records) {
      for (const node of record.addedNodes) for (const img of watch(node)) observer.observe(img);
      for (const node of record.removedNodes) for (const img of watch(node)) { observer.unobserve(img); visible.delete(img); }
    }
    schedule();
  });
  for (const grid of galleries) mutationObserver.observe(grid, { childList: true, subtree: true });
  document.addEventListener('load', schedule, true);
  document.addEventListener('visibilitychange', schedule);
  window.addEventListener('scroll', schedule, { passive: true, capture: true });
  window.addEventListener('resize', schedule, { passive: true });
  visualViewport?.addEventListener('resize', schedule, { passive: true });
  visualViewport?.addEventListener('scroll', schedule, { passive: true });
  new MutationObserver(schedule).observe(document.documentElement, { attributes: true, attributeFilter: ['style'] });
  reduced.addEventListener('change', schedule);
})();
