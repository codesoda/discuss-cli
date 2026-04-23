(function() {
  const selector = 'pre > code.language-mermaid';
  const blocks = Array.from(document.querySelectorAll(selector));
  if (!blocks.length) return;

  function renderBlocks() {
    const mermaid = window.mermaid && (window.mermaid.mermaidAPI || window.mermaid);
    if (!mermaid || typeof mermaid.render !== 'function') return;
    if (typeof mermaid.initialize === 'function') {
      mermaid.initialize({ startOnLoad: false });
    }

    blocks.forEach((code, index) => {
      const pre = code.parentElement;
      if (!pre) return;
      const source = code.textContent || '';
      mermaid.render(`discuss-mermaid-${index}`, source, svg => {
        pre.innerHTML = svg;
        pre.setAttribute('data-mermaid-rendered', 'true');
      }, pre);
    });
  }

  const script = document.createElement('script');
  script.src = window.__DISCUSS_MERMAID_SRC__ || '/assets/mermaid.min.js';
  script.async = true;
  script.onload = renderBlocks;
  document.head.appendChild(script);
})();
