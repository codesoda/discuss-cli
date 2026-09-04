const routes = {
  '/': ['Operations overview', 'Monitor retry health before widening the rollout.'],
  '/payments': ['Payment retries', 'Inspect the rollout control and leave a route-scoped element comment.'],
  '/settings/alerts': ['Alert settings', 'This route is rendered client-side without a document reload.'],
};

function renderRoute() {
  const [title, copy] = routes[location.pathname] || ['Demo route', 'A client-side route inside the bundled local app.'];
  document.querySelector('#page-title').textContent = title;
  document.querySelector('#page-copy').textContent = copy;
  document.querySelectorAll('[data-route]').forEach(link => {
    if (link.tagName === 'A') {
      if (link.dataset.route === location.pathname) link.setAttribute('aria-current', 'page');
      else link.removeAttribute('aria-current');
    }
  });
}

document.querySelectorAll('a[data-route]').forEach(link => {
  link.addEventListener('click', event => {
    event.preventDefault();
    history.pushState({}, '', link.dataset.route);
    renderRoute();
  });
});
window.addEventListener('popstate', renderRoute);
renderRoute();

fetch('/api/dashboard').then(response => response.json()).then(data => {
  document.querySelector('#recovery-rate').textContent = data.recoveryRate;
  document.querySelector('#retry-budget').textContent = data.retryBudget;
  document.querySelector('#api-status').textContent = data.status;
  document.body.dataset.demoApiLoaded = 'true';
});

document.querySelector('#advance-rollout').addEventListener('click', () => {
  document.querySelector('#advance-rollout').textContent = 'Demo action complete';
});
