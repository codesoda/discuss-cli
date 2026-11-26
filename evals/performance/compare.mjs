import fs from 'node:fs';
import { stats, assertCompatible } from './stats.mjs';
const [beforePath, afterPath] = process.argv.slice(2);
if (!beforePath || !afterPath) throw new Error('usage: node compare.mjs BEFORE.json AFTER.json');
const before = JSON.parse(fs.readFileSync(beforePath));
const after = JSON.parse(fs.readFileSync(afterPath));
assertCompatible(before, after);
for (const name of before.scenarioNames) {
  console.log(name);
  for (const key of Object.keys(before.scenarios[name].samples)) {
    const a = stats(before.scenarios[name].samples[key]).median;
    const b = stats(after.scenarios[name].samples[key]).median;
    console.log(`  ${key}: ${a.toFixed(3)} -> ${b.toFixed(3)} ms; ${((b - a) / a * 100).toFixed(2)}% (${(b - a).toFixed(3)} ms)`);
  }
}
