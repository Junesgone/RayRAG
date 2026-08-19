import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/login');
await page.evaluate(async () => {
  const r = await fetch('/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password:'rayrag-smoke-admin'})});
  const j = await r.json();
  localStorage.setItem('rayrag_token', j.data.access_token);
  document.cookie = 'rayrag_token='+j.data.access_token+';path=/;max-age=86400';
});

// Per-page checks: [route, [marker, desc]]
const checks = [
  ['/', ['Welcome', 'welcome text'], ['See all', 'see-all link'], ['Chat', 'chat entry']],
  ['/datasets', ['Create dataset', 'create btn'], ['Search', 'search'], ['50', 'pagination'], ['English', 'lang switch']],
  ['/chats', ['Create chat', 'create chat'], ['Search', 'search']],
  ['/searches', ['Create search', 'create search']],
  ['/agents', ['Create agent', 'create agent']],
  ['/memories', ['Create memory', 'create memory']],
  ['/files', ['Add file', 'add file'], ['Select all', 'select all']],
  ['/user-setting/data-source', ['Available', 'available sources'], ['Add', 'add button']],
  ['/user-setting/chat-channel', ['Discord', 'discord channel']],
  ['/user-setting/mcp', ['MCP', 'mcp title'], ['Add', 'add mcp']],
  ['/user-setting/team', ['Invite', 'invite member']],
  ['/user-setting/profile', ['Upload', 'upload avatar'], ['Edit', 'edit']],
  ['/user-setting/api', ['API KEY', 'api key']],
];
const out = [];
for (const [route, marks] of checks) {
  await page.goto('http://127.0.0.1:9390' + route + '?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(1200);
  const body = await page.evaluate(() => document.body.innerText);
  const res = {};
  for (const [m, d] of marks) res[d] = body.includes(m);
  out.push({ route, ...res });
}
console.log(JSON.stringify(out, null, 1));
await page.close();
await browser.close();
