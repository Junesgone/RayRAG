// Login test: connect to the running fnOS Chrome via CDP and log into RAGFlow UI.
import { chromium } from 'playwright-core';

const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const RAGFLOW_UI = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const EMAIL = 'admin@ragflow.io';
const PASSWORD = 'admin123';

const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = ctx.pages().find(p => p.url().startsWith(RAGFLOW_UI)) || (await ctx.newPage());

const sleep = (ms) => new Promise(r => setTimeout(r, ms));

await page.goto(RAGFLOW_UI + '/login-next', { waitUntil: 'domcontentloaded', timeout: 60000 });
await sleep(5000);
console.log('url now:', page.url());

// find any visible inputs
const inputs = await page.locator('input:visible').count().catch(() => 0);
console.log('visible inputs:', inputs);
let content = '';
try { content = (await page.content()).slice(0, 3000); } catch (e) { content = String(e); }
console.log('---content head---\n' + content);

if (inputs > 0) {
  const emailInput = page.locator('input[type="email"], input[placeholder*="mail" i], input[placeholder*="邮箱" i]').first();
  const hasEmail = (await emailInput.count().catch(() => 0)) > 0;
  if (hasEmail) {
    await emailInput.fill(EMAIL);
    const pwd = page.locator('input[type="password"]').first();
    await pwd.fill(PASSWORD);
    const submit = page.locator('button:has-text("Log in"), button:has-text("登录"), button:has-text("Sign in"), button[type="submit"]').first();
    await submit.click().catch(e => console.log('submit err', String(e)));
    await sleep(6000);
    console.log('after-login url:', page.url(), 'title:', await page.title().catch(() => '?'));
    await page.screenshot({ path: 'login-result.png' }).catch(() => {});
  }
}
await browser.close();

