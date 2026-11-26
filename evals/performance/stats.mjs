export function stats(values) {
  if (!values.length || values.some(v => !Number.isFinite(v))) throw new Error('invalid samples');
  const sorted = [...values].sort((a, b) => a - b);
  const pick = q => sorted[Math.ceil(q * sorted.length) - 1];
  return { count: sorted.length, p50: pick(.5), p95: pick(.95), median: sorted.length % 2 ? sorted[(sorted.length - 1) / 2] : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2 };
}

export function assertCompatible(a, b) {
  for (const key of ['version', 'fixtures', 'options', 'scenarioNames']) {
    if (JSON.stringify(a[key]) !== JSON.stringify(b[key])) throw new Error(`incompatible ${key}`);
  }
  if (a.environment.profile !== b.environment.profile || a.environment.os !== b.environment.os || a.environment.cpu !== b.environment.cpu || a.environment.browser !== b.environment.browser || a.environment.node !== b.environment.node) throw new Error('incompatible machine/browser profile; no cross-host claims');
  for (const name of a.scenarioNames) {
    if (!b.scenarios[name] || !a.scenarios[name]) throw new Error(`missing scenario ${name}`);
    const left = a.scenarios[name].samples;
    const right = b.scenarios[name].samples;
    if (JSON.stringify(Object.keys(left).sort()) !== JSON.stringify(Object.keys(right).sort())) throw new Error(`incompatible metrics ${name}`);
    for (const metric of Object.keys(left)) {
      if (left[metric].length < 20 || right[metric].length !== left[metric].length) throw new Error(`incompatible samples ${name}/${metric}`);
    }
  }
}
