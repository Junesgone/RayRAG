// Create (or delete) a temporary dataset on the live RAGFlow v0.27.2 instance for
// detail-route traversal. Usage:
//   node fixture-dataset-v0272.mjs create <name>
//   node fixture-dataset-v0272.mjs delete <id>
import { chromium } from 'playwright-core';
const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const BASE = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const [action, value] = process.argv.slice(2);
if (!action || !value) {
  console.error('usage: node fixture-dataset-v0272.mjs create|delete <name|id>');
  process.exit(2);
}
const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = ctx.pages().find((p) => p.url().startsWith(BASE)) || (await ctx.newPage());
await page.goto(BASE + '/datasets', { waitUntil: 'domcontentloaded', timeout: 60000 });
await new Promise((r) => setTimeout(r, 2500));
const result = await page.evaluate(
  async ({ action, value }) => {
    let authorization = '';
    try {
      authorization = localStorage.getItem('Authorization') || '';
    } catch (error) {
      return { error: 'localStorage unavailable: ' + String(error) };
    }
    if (!authorization) return { error: 'no Authorization token in localStorage' };
    const headers = { Authorization: authorization, 'Content-Type': 'application/json' };
    try {
      if (action === 'create') {
        const response = await fetch('/api/v1/datasets', {
          method: 'POST',
          headers,
          body: JSON.stringify({ name: value }),
        });
        const text = await response.text();
        let parsed = null;
        try {
          parsed = JSON.parse(text);
        } catch {
          parsed = null;
        }
        const id = parsed && parsed.data ? parsed.data.id : null;
        return { status: response.status, id, body: text.slice(0, 300) };
      }
      if (action === 'delete') {
        const response = await fetch('/api/v1/datasets', {
          method: 'DELETE',
          headers,
          body: JSON.stringify({ ids: [value] }),
        });
        return { status: response.status, body: (await response.text()).slice(0, 300) };
      }
      return { error: 'unknown action ' + action };
    } catch (error) {
      return { error: String(error) };
    }
  },
  { action, value }
);
console.log(JSON.stringify(result, null, 2));
await browser.close();
