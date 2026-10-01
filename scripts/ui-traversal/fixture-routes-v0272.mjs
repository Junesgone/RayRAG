// RAGFlow v0.27.2 fixture helper for UI traversal: search apps, documents, chunks.
// Usage:
//   node fixture-routes-v0272.mjs create-search <name>
//   node fixture-routes-v0272.mjs delete-search <id>
//   node fixture-routes-v0272.mjs upload <dataset_id> <file_name>
//   node fixture-routes-v0272.mjs parse <dataset_id> <document_id>
//   node fixture-routes-v0272.mjs chunks <dataset_id> <document_id>
import { chromium } from 'playwright-core';

const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const BASE = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const [action, ...args] = process.argv.slice(2);
if (!action) {
  console.error('usage: node fixture-routes-v0272.mjs <action> [args]');
  process.exit(2);
}
const browser = await chromium.connectOverCDP(CDP);
const context = browser.contexts()[0] || (await browser.newContext());
const page = context.pages().find((item) => item.url().startsWith(BASE)) || (await context.newPage());
await page.goto(BASE + '/datasets', { waitUntil: 'domcontentloaded', timeout: 60000 });
await new Promise((resolve) => setTimeout(resolve, 2000));

const result = await page.evaluate(
  async ({ action, args }) => {
    const authorization = localStorage.getItem('Authorization') || '';
    if (!authorization) return { error: 'no Authorization token in localStorage' };
    const jsonHeaders = { Authorization: authorization, 'Content-Type': 'application/json' };
    try {
      if (action === 'create-search') {
        const response = await fetch('/v1/searches', {
          method: 'POST',
          headers: jsonHeaders,
          body: JSON.stringify({ name: args[0] }),
        });
        const text = await response.text();
        let id = null;
        try { id = JSON.parse(text).data?.search_id || null; } catch { /* keep raw */ }
        return { status: response.status, id, body: text.slice(0, 240) };
      }
      if (action === 'delete-search') {
        const response = await fetch('/v1/searches/' + encodeURIComponent(args[0]), { method: 'DELETE', headers: jsonHeaders });
        return { status: response.status, body: (await response.text()).slice(0, 240) };
      }
      if (action === 'upload') {
        const form = new FormData();
        const file = new File(['RayRAG UI traversal fixture.\nSecond line for chunking.\n'], args[1] || 'traversal-fixture.txt', { type: 'text/plain' });
        form.append('file', file, file.name);
        const response = await fetch('/v1/datasets/' + encodeURIComponent(args[0]) + '/documents', {
          method: 'POST',
          headers: { Authorization: authorization },
          body: form,
        });
        const text = await response.text();
        let id = null;
        try { id = JSON.parse(text).data?.[0]?.id || null; } catch { /* keep raw */ }
        return { status: response.status, id, body: text.slice(0, 240) };
      }
      if (action === 'parse') {
        const response = await fetch('/v1/datasets/' + encodeURIComponent(args[0]) + '/chunks', {
          method: 'POST',
          headers: jsonHeaders,
          body: JSON.stringify({ document_ids: [args[1]] }),
        });
        return { status: response.status, body: (await response.text()).slice(0, 300) };
      }
      if (action === 'chunks') {
        const response = await fetch(
          '/v1/datasets/' + encodeURIComponent(args[0]) + '/documents/' + encodeURIComponent(args[1]) + '/chunks',
          { headers: { Authorization: authorization } },
        );
        const text = await response.text();
        let chunkId = null;
        let total = 0;
        try {
          const parsed = JSON.parse(text);
          const chunks = parsed.data?.chunks || parsed.data || [];
          total = Array.isArray(chunks) ? chunks.length : 0;
          chunkId = total ? chunks[0].id || chunks[0].chunk_id || null : null;
        } catch { /* keep raw */ }
        return { status: response.status, total, chunkId, body: text.slice(0, 300) };
      }
      return { error: 'unknown action ' + action };
    } catch (error) {
      return { error: String(error) };
    }
  },
  { action, args },
);
console.log(JSON.stringify(result, null, 2));
await browser.close();
