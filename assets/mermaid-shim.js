(function () {
  const selector = 'pre > code.language-mermaid';
  let renderSeq = 0;
  let mermaidLoaded = false;

  // Mark each block before Prism runs so highlightCodeBlocks() in
  // discuss.html can skip syntax highlighting for mermaid sources.
  function markBlocks() {
    const blocks = Array.from(document.querySelectorAll(selector)).filter(
      function (code) {
        const pre = code.parentElement;
        return pre && !pre.hasAttribute('data-mermaid');
      }
    );
    blocks.forEach(function (code) {
      const pre = code.parentElement;
      pre.classList.add('mermaid-block', 'no-line-numbers');
      pre.setAttribute('data-mermaid', 'pending');
    });
    return blocks;
  }

  // Turn the SVG markup mermaid returns into a real node instead of assigning
  // innerHTML: parsing happens in an inert document, scripts and inline event
  // handlers are stripped, and only the resulting <svg> element is adopted.
  function parseSvg(markup) {
    const parser = new DOMParser();
    let doc = parser.parseFromString(markup, 'image/svg+xml');
    let svg = doc.querySelector('parsererror') ? null : doc.documentElement;
    if (!svg || svg.nodeName.toLowerCase() !== 'svg') {
      doc = parser.parseFromString(markup, 'text/html');
      svg = doc.body ? doc.body.querySelector('svg') : null;
    }
    if (!svg) return null;
    const imported = document.importNode(svg, true);
    sanitize(imported);
    return imported;
  }

  function sanitize(root) {
    root.querySelectorAll('script').forEach(function (script) {
      script.remove();
    });
    const nodes = [root].concat(Array.from(root.querySelectorAll('*')));
    nodes.forEach(function (node) {
      Array.from(node.attributes || []).forEach(function (attribute) {
        const name = attribute.name.toLowerCase();
        const value = attribute.value.trim().toLowerCase();
        if (name.startsWith('on') || value.startsWith('javascript:')) {
          node.removeAttribute(attribute.name);
        }
      });
    });
  }

  function reportError(pre, error) {
    pre.setAttribute('data-mermaid', 'error');
    const note = document.createElement('div');
    note.className = 'mermaid-error';
    note.textContent =
      'mermaid render failed: ' + (error && error.message ? error.message : error);
    if (pre.parentElement) pre.parentElement.insertBefore(note, pre.nextSibling);
  }

  function renderBlocks() {
    const mermaid = window.mermaid;
    if (!mermaid || typeof mermaid.render !== 'function') return;
    if (typeof mermaid.initialize === 'function') {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'loose',
        theme: 'default',
      });
    }

    document.querySelectorAll('pre[data-mermaid="pending"]').forEach(function (pre) {
      const code = pre.querySelector('code.language-mermaid');
      const source = code ? code.textContent || '' : '';
      const id = 'discuss-mermaid-' + renderSeq++;
      try {
        mermaid
          .render(id, source)
          .then(function (output) {
            const svg = parseSvg(output.svg || '');
            if (!svg) {
              reportError(pre, new Error('mermaid returned unparseable SVG'));
              return;
            }
            pre.replaceChildren(svg);
            pre.setAttribute('data-mermaid', 'rendered');
          })
          .catch(function (error) {
            reportError(pre, error);
          });
      } catch (error) {
        reportError(pre, error);
      }
    });
  }

  let loaderStarted = false;
  function ensureLoadedThenRender() {
    if (mermaidLoaded) {
      renderBlocks();
      return;
    }
    if (loaderStarted) return; // onload will pick up pending blocks
    loaderStarted = true;
    const script = document.createElement('script');
    script.src = window.__DISCUSS_MERMAID_SRC__ || '/assets/mermaid.min.js';
    script.async = true;
    script.onload = function () {
      mermaidLoaded = true;
      renderBlocks();
    };
    document.head.appendChild(script);
  }

  // Re-scan and render after a live source update replaces #doc-content.
  window.__discussRenderMermaid = function () {
    if (markBlocks().length || document.querySelector('pre[data-mermaid="pending"]')) {
      ensureLoadedThenRender();
    }
  };

  if (markBlocks().length) ensureLoadedThenRender();
})();
