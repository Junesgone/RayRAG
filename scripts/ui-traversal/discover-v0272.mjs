// Discover live detail-route ids on the RAGFlow v0.27.2 instance (CDP, logged-in profile).
import { chromium } from 'playwright-core';
const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const BASE = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = ctx.pages().find((p) => p.url().startsWith(BASE)) || (await ctx.newPage());
const out = {};
const targets = [
  ['datasets', '/datasets', '/dataset/'],
  ['agents', '/agents', '/agent/'],
  ['memories', '/memories', '/memory/'],
  ['chats', '/chats', '/chat/'],
  ['searches', '/searches', '/search/'],
];
for (const [name, url, prefix] of targets) {
  try {
    await page.goto(BASE + url, { waitUntil: 'domcontentloaded', timeout: 60000 });
    await new Promise((r) => setTimeout(r, 3500));
    const hrefs = await page.evaluate(() =>
      Array.from(document.querySelectorAll('a[href]'))
        .map((a) => a.getAttribute('href'))
        .filter(Boolean)
    );
    out[name] = [...new Set(hrefs.filter((h) => h.startsWith(prefix)))].slice(0, 40);
  } catch (error) {
    out[name] = ['ERROR: ' + String(error).slice(0, 160)];
  }
}
console.log(JSON.stringify(out, null, 2));
await browser.close();
