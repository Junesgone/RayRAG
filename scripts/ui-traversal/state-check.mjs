import { chromium } from 'playwright-core';
const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const RAGFLOW_UI = 'http://127.0.0.1:11526';
const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0];
const page = ctx.pages().find(p => p.url().startsWith(RAGFLOW_UI)) || (await ctx.newPage());
await page.goto(RAGFLOW_UI, { waitUntil: 'domcontentloaded', timeout: 60000 });
await new Promise(r => setTimeout(r, 5000));
const cookies = await ctx.cookies(RAGFLOW_UI);
console.log('cookies:', cookies.map(c => `${c.name}=${c.value.slice(0,20)}...`).join('\n  '));
const text = await page.evaluate(() => document.body.innerText).catch(e => String(e));
console.log('--- body text ---\n' + text.slice(0, 2500));
await page.screenshot({ path: 'home-state.png' }).catch(() => {});
const nav = await page.evaluate(() => Array.from(document.querySelectorAll('nav a, header a, [class*=menu] a, [class*=sidebar] a')).map(a => a.textContent.trim() + ' -> ' + a.getAttribute('href')).filter(Boolean).slice(0, 60));
console.log('--- nav links ---\n' + nav.join('\n'));
await browser.close();

