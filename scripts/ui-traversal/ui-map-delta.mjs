// Compare two ragflow-ui-map.json snapshots and emit a markdown delta report.
// Usage: node ui-map-delta.mjs <old.json> <new.json> <out.md>
import { readFileSync, writeFileSync } from 'node:fs';

const [oldPath, newPath, outPath] = process.argv.slice(2);
if (!oldPath || !newPath || !outPath) {
  console.error('usage: node ui-map-delta.mjs <old.json> <new.json> <out.md>');
  process.exit(2);
}
const readMap = (file) => {
  let raw;
  try {
    raw = readFileSync(file, 'utf8');
  } catch (error) {
    console.error('cannot read ' + file + ': ' + String(error));
    process.exit(1);
  }
  try {
    return JSON.parse(raw);
  } catch (error) {
    console.error('invalid JSON in ' + file + ': ' + String(error));
    process.exit(1);
  }
};
const oldMap = readMap(oldPath);
const newMap = readMap(newPath);

const byRoute = (map) => new Map((map.routes || []).map((r) => [r.route, r]));
const oldRoutes = byRoute(oldMap);
const newRoutes = byRoute(newMap);

const elementKey = (e) => e.text + '|' + e.role;
const describeResult = (e) => {
  const r = e.result;
  if (Array.isArray(r)) return r.join(' ; ');
  return String(r ?? '');
};

const lines = [];
lines.push('# RAGFlow UI map delta');
lines.push('');
lines.push('- old: ' + oldMap.base + ' @ ' + (oldMap.generated_at || '?') + ' — ' + oldRoutes.size + ' routes');
lines.push('- new: ' + newMap.base + ' @ ' + (newMap.generated_at || '?') + ' — ' + newRoutes.size + ' routes');
lines.push('');

const addedRoutes = [...newRoutes.keys()].filter((r) => !oldRoutes.has(r));
const removedRoutes = [...oldRoutes.keys()].filter((r) => !newRoutes.has(r));
lines.push('## Routes');
lines.push('');
lines.push('- added: ' + (addedRoutes.join(', ') || 'none'));
lines.push('- removed: ' + (removedRoutes.join(', ') || 'none'));
lines.push('');

lines.push('## Per-route element changes');
lines.push('');
for (const [route, next] of newRoutes) {
  const prev = oldRoutes.get(route);
  if (!prev) {
    lines.push('- `' + route + '`: NEW (' + (next.elements || []).length + ' elements)');
    continue;
  }
  const prevKeys = new Set((prev.elements || []).map(elementKey));
  const nextKeys = new Set((next.elements || []).map(elementKey));
  const added = [...nextKeys].filter((k) => !prevKeys.has(k));
  const removed = [...prevKeys].filter((k) => !nextKeys.has(k));
  const changed = [];
  const prevByKey = new Map((prev.elements || []).map((e) => [elementKey(e), e]));
  for (const e of next.elements || []) {
    const p = prevByKey.get(elementKey(e));
    if (p && describeResult(p) !== describeResult(e)) {
      changed.push(e.text + ' [' + p.role + ']: ' + describeResult(p) + ' -> ' + describeResult(e));
    }
  }
  if (!added.length && !removed.length && !changed.length) continue;
  lines.push('- `' + route + '`: +' + added.length + ' / -' + removed.length + ' / ~' + changed.length);
  for (const a of added.slice(0, 12)) lines.push('  - added: ' + a);
  for (const r of removed.slice(0, 12)) lines.push('  - removed: ' + r);
  for (const c of changed.slice(0, 12)) lines.push('  - changed: ' + c);
}
lines.push('');

writeFileSync(outPath, lines.join('\n'));
console.log('delta written:', outPath, addedRoutes.length + ' added routes,' + removedRoutes.length + ' removed routes');
