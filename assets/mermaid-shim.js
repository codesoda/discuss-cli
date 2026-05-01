(function () {
  const selector = 'pre > code.language-mermaid';
  const blocks = Array.from(document.querySelectorAll(selector));

  // Mark each block before Prism runs so highlightCodeBlocks() in
  // discuss.html can skip syntax highlighting for mermaid sources.
  blocks.forEach(function (code) {
    const pre = code.parentElement;
    if (!pre) return;
    pre.classList.add('mermaid-block', 'no-line-numbers');
    pre.setAttribute('data-mermaid', 'pending');
  });

  if (!blocks.length) return;

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

    blocks.forEach(function (code, index) {
      const pre = code.parentElement;
      if (!pre) return;
      const source = code.textContent || '';
      const id = 'discuss-mermaid-' + index;
      try {
        mermaid
          .render(id, source)
          .then(function (output) {
            pre.innerHTML = output.svg;
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

  const script = document.createElement('script');
  script.src = window.__DISCUSS_MERMAID_SRC__ || '/assets/mermaid.min.js';
  script.async = true;
  script.onload = renderBlocks;
  document.head.appendChild(script);
})();
