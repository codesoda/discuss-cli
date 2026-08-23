(() => {
  'use strict';

  const parentOrigin = location.origin;
  const host = document.createElement('div');
  host.setAttribute('data-discuss-inspector', '');
  const shadow = host.attachShadow({ mode: 'closed' });
  shadow.innerHTML = `
    <style>
      :host { all: initial; position: fixed; inset: 0; z-index: 2147483647; display: block; pointer-events: none; }
      .outline { position: absolute; display: none; pointer-events: none;
        border: 2px solid #0b5fff; background: rgba(11,95,255,.12); box-sizing: border-box;
        box-shadow: 0 0 0 1px rgba(255,255,255,.85); }
      .outline.pulse { animation: discuss-pulse .8s ease-out; }
      .marker { position: absolute; pointer-events: auto; width: 24px; height: 24px; padding: 0;
        transform: translate(-50%,-50%); border: 2px solid white; border-radius: 50%;
        background: #e91e63; color: white; box-shadow: 0 1px 5px rgba(0,0,0,.35);
        font: 700 12px/20px system-ui,sans-serif; text-align: center; cursor: pointer; }
      .marker:hover { transform: translate(-50%,-50%) scale(1.12); }
      @keyframes discuss-pulse { 0% { box-shadow: 0 0 0 0 rgba(11,95,255,.7); }
        100% { box-shadow: 0 0 0 16px rgba(11,95,255,0); } }
    </style>
    <div class="outline"></div>
    <div class="markers"></div>`;
  (document.body || document.documentElement).appendChild(host);

  const outline = shadow.querySelector('.outline');
  const markersHost = shadow.querySelector('.markers');
  const stableAnchors = new Map();
  const resolvedElements = new Map();
  let inspectOn = false;
  let hovered = null;
  let scheduled = false;
  let previousCursor = '';

  function post(type, detail = {}) {
    window.parent.postMessage({ type, ...detail }, parentOrigin);
  }

  function normalizeText(value) {
    return String(value || '').replace(/\s+/g, ' ').trim();
  }

  function cssEscape(value) {
    if (window.CSS && typeof window.CSS.escape === 'function') return window.CSS.escape(String(value));
    return String(value).replace(/[^a-zA-Z0-9_-]/g, character => `\\${character}`);
  }

  function attributeValue(value) {
    return String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  }

  function stableId(element) {
    const id = element && element.id;
    return id && !/\d{4,}|^:|^radix-/i.test(id) ? id : null;
  }

  function unique(selector) {
    try { return document.querySelectorAll(selector).length === 1; }
    catch (_) { return false; }
  }

  function segment(element, nthKind) {
    const tag = element.tagName.toLowerCase();
    const classes = Array.from(element.classList || [])
      .filter(name => name && !name.startsWith('discuss-'))
      .slice(0, 3)
      .map(name => `.${cssEscape(name)}`)
      .join('');
    const siblings = Array.from(element.parentElement ? element.parentElement.children : []);
    const sameTag = siblings.filter(sibling => sibling.tagName === element.tagName);
    let nth = '';
    if (nthKind === 'child') {
      nth = `:nth-child(${siblings.indexOf(element) + 1})`;
    } else if (sameTag.length > 1) {
      nth = `:nth-of-type(${sameTag.indexOf(element) + 1})`;
    }
    return `${tag}${classes}${nth}`;
  }

  function fullPath(element) {
    const parts = [];
    let current = element;
    while (current && current.nodeType === 1 && current !== document.body) {
      parts.unshift(segment(current, 'child'));
      current = current.parentElement;
    }
    parts.unshift('body');
    return parts.join(' > ');
  }

  function shortestUniquePath(element) {
    const parts = [];
    let current = element;
    while (current && current.nodeType === 1) {
      const id = stableId(current);
      if (id) {
        parts.unshift(`#${cssEscape(id)}`);
        return parts.join(' > ');
      }
      parts.unshift(segment(current, 'type'));
      const candidate = parts.join(' > ');
      if (unique(candidate)) return candidate;
      if (current === document.body) break;
      current = current.parentElement;
    }
    return fullPath(element);
  }

  function dataSelector(element) {
    for (const name of ['data-testid', 'data-test', 'data-id']) {
      if (!element.hasAttribute(name)) continue;
      const own = `[${name}="${attributeValue(element.getAttribute(name))}"]`;
      let ancestor = element.parentElement;
      while (ancestor && !stableId(ancestor)) ancestor = ancestor.parentElement;
      const scoped = ancestor ? `#${cssEscape(stableId(ancestor))} ${own}` : own;
      if (unique(scoped)) return scoped;
    }
    return null;
  }

  function selectorDescriptor(element) {
    const id = stableId(element);
    const byId = id ? `#${cssEscape(id)}` : null;
    const byData = dataSelector(element);
    const shortest = shortestUniquePath(element);
    const full = fullPath(element);
    const candidates = [byId, byData, shortest, full].filter(Boolean);
    const selector = candidates.find(unique) || full;
    const fallbacks = [...new Set(candidates.filter(candidate => candidate !== selector))];
    const text = normalizeText(element.innerText || element.textContent).slice(0, 120);
    return {
      selector,
      fallbacks,
      tag: element.tagName.toLowerCase(),
      ...(text ? { textDigest: text } : {}),
      outerHtml: String(element.outerHTML || '').slice(0, 500),
    };
  }

  function breadcrumb(element) {
    const parts = [];
    let current = element;
    while (current && current.nodeType === 1) {
      let label = current.tagName.toLowerCase();
      if (stableId(current)) label += `#${current.id}`;
      else if (current.classList && current.classList.length) label += `.${current.classList[0]}`;
      if (current.parentElement) {
        const matches = Array.from(current.parentElement.children).filter(child => child.tagName === current.tagName);
        if (matches.length > 1) label += ` (${matches.indexOf(current) + 1})`;
      }
      parts.unshift(label);
      if (current === document.body) break;
      current = current.parentElement;
    }
    const text = normalizeText(element.innerText || element.textContent).slice(0, 40);
    return `${parts.join(' > ')}${text ? ` “${text}${text.length === 40 ? '…' : ''}”` : ''}`;
  }

  function dice(left, right) {
    const a = normalizeText(left).toLowerCase();
    const b = normalizeText(right).toLowerCase();
    if (a === b) return 1;
    if (a.length < 2 || b.length < 2) return 0;
    const pairs = new Map();
    for (let i = 0; i < a.length - 1; i++) {
      const pair = a.slice(i, i + 2);
      pairs.set(pair, (pairs.get(pair) || 0) + 1);
    }
    let matches = 0;
    for (let i = 0; i < b.length - 1; i++) {
      const pair = b.slice(i, i + 2);
      const count = pairs.get(pair) || 0;
      if (count) { matches++; pairs.set(pair, count - 1); }
    }
    return (2 * matches) / (a.length + b.length - 2);
  }

  function resolveAnchor(anchor) {
    if (!anchor || typeof anchor !== 'object') return null;
    const selectors = [anchor.selector, ...(Array.isArray(anchor.fallbacks) ? anchor.fallbacks : [])];
    for (const selector of selectors) {
      if (typeof selector !== 'string' || !selector) continue;
      try {
        const matches = document.querySelectorAll(selector);
        if (matches.length === 1) return matches[0];
      } catch (_) {}
    }
    if (!anchor.tag || !anchor.textDigest) return null;
    const candidates = Array.from(document.getElementsByTagName(anchor.tag));
    let best = null;
    let score = 0;
    candidates.forEach(candidate => {
      const candidateScore = dice(candidate.innerText || candidate.textContent, anchor.textDigest);
      if (candidateScore > score) { best = candidate; score = candidateScore; }
    });
    return score >= 0.8 ? best : null;
  }

  function setOutline(element) {
    hovered = element;
    host.toggleAttribute('data-discuss-hovering', !!element);
    if (!element) { outline.style.display = 'none'; return; }
    const rect = element.getBoundingClientRect();
    outline.style.display = rect.width && rect.height ? 'block' : 'none';
    outline.style.left = `${rect.left}px`;
    outline.style.top = `${rect.top}px`;
    outline.style.width = `${rect.width}px`;
    outline.style.height = `${rect.height}px`;
  }

  function resolveAndRender() {
    scheduled = false;
    markersHost.replaceChildren();
    resolvedElements.clear();
    const resolved = [];
    const detached = [];
    let number = 0;
    stableAnchors.forEach((anchor, threadId) => {
      number++;
      const element = resolveAnchor(anchor);
      if (!element) { detached.push(threadId); return; }
      resolvedElements.set(threadId, element);
      const rect = element.getBoundingClientRect();
      resolved.push({ threadId, rect: rectJson(rect) });
      const marker = document.createElement('button');
      marker.className = 'marker';
      marker.type = 'button';
      marker.textContent = String(number);
      marker.title = `Open thread ${number}`;
      marker.dataset.threadId = threadId;
      marker.style.left = `${rect.right}px`;
      marker.style.top = `${rect.top}px`;
      markersHost.appendChild(marker);
    });
    post('discuss:anchors-resolved', { resolved, detached });
  }

  function scheduleResolve() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(resolveAndRender);
  }

  function rectJson(rect) {
    return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, width: rect.width, height: rect.height };
  }

  window.addEventListener('mousemove', event => {
    if (!inspectOn) return;
    const target = event.target;
    setOutline(target && target.nodeType === 1 && target !== host ? target : null);
  }, true);

  window.addEventListener('click', event => {
    if (!inspectOn) return;
    const target = event.target;
    if (!target || target.nodeType !== 1 || target === host) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const rect = target.getBoundingClientRect();
    post('discuss:element-selected', {
      anchor: selectorDescriptor(target),
      rect: rectJson(rect),
      breadcrumb: breadcrumb(target),
      snippet: normalizeText(target.innerText || target.textContent).slice(0, 500),
    });
  }, true);

  shadow.addEventListener('click', event => {
    const marker = event.target.closest('.marker');
    if (!marker) return;
    post('discuss:marker-clicked', { threadId: marker.dataset.threadId });
  });

  window.addEventListener('message', event => {
    if (event.origin !== location.origin || event.source !== window.parent) return;
    const message = event.data;
    if (!message || typeof message.type !== 'string' || !message.type.startsWith('discuss:')) return;
    const payload = message.payload && typeof message.payload === 'object' ? message.payload : message;
    if (message.type === 'discuss:set-inspect') {
      const nextInspectOn = payload.on === true;
      if (nextInspectOn && !inspectOn) {
        previousCursor = document.documentElement.style.cursor;
      }
      inspectOn = nextInspectOn;
      document.documentElement.style.cursor = inspectOn ? 'crosshair' : previousCursor;
      if (!inspectOn) setOutline(null);
    } else if (message.type === 'discuss:resolve-anchors') {
      stableAnchors.clear();
      const anchors = Array.isArray(payload.anchors) ? payload.anchors : [];
      anchors.forEach(item => {
        if (item && item.threadId != null && item.anchor) stableAnchors.set(String(item.threadId), item.anchor);
      });
      scheduleResolve();
    } else if (message.type === 'discuss:focus-thread') {
      const element = resolvedElements.get(String(payload.threadId));
      if (!element) return;
      element.scrollIntoView({ block: 'center', behavior: 'smooth' });
      setOutline(element);
      outline.classList.remove('pulse');
      void outline.offsetWidth;
      outline.classList.add('pulse');
      setTimeout(() => { if (!inspectOn) setOutline(null); }, 900);
    }
  });

  window.addEventListener('scroll', scheduleResolve, true);
  window.addEventListener('resize', scheduleResolve);
  if ('ResizeObserver' in window) new ResizeObserver(scheduleResolve).observe(document.documentElement);
  new MutationObserver(scheduleResolve).observe(document.documentElement, { childList: true, subtree: true, attributes: true });

  if (document.querySelector('[src^="/"], [href^="/"]')) {
    console.warn('discuss: root-absolute prototype assets are not rewritten; use relative URLs.');
  }
  post('discuss:ready');
})();
