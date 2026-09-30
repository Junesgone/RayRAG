// Probe the live RAGFlow v0.27.2 session for API auth + dataset list (CDP, logged-in profile).
import { chromium } from 'playwright-core';
const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const BASE = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = ctx.pages().find((p) => p.url().startsWith(BASE)) || (await ctx.newPage());
await page.goto(BASE + '/datasets', { waitUntil: 'domcontentloaded', timeout: 60000 });
await new Promise((r) => setTimeout(r, 3000));
const out = await page.evaluate(async () => {
  const keys = Object.keys(localStorage);
  const dump = {};
  for (const key of keys) dump[key] = String(localStorage.getItem(key) || '').slice(0, 50);
  const candidates = [
    localStorage.getItem('ragflow_token'),
    localStorage.getItem('token'),
    localStorage.getItem('Authorization'),
  ].filter(Boolean);
  const result = { keys, dump, candidates: candidates.map((c) => c.slice(0, 30)) };
  const headers = candidates[0] ? { Authorization: candidates[0] } : {};
  try {
    const first = await fetch('/api/v1/datasets?page=1&page_size=10', { headers });
    result.list = { status: first.status, body: (await first.text()).slice(0, 400) };
  } catch (error) {
    result.list = { error: String(error) };
  }
  return result;
});
console.log(JSON.stringify(out, null, 2));
await browser.close();
