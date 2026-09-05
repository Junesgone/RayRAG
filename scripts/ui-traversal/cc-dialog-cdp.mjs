import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || 'http://127.0.0.1:9390';
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error('RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)');
  process.exit(2);
}
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = {};
const created = [];
const listIds = async (channel) => await page.evaluate(async (ch) => {
  const r = await fetch('/api/v1/chat-channels');
  const j = await r.json();
  return (j.data || []).filter((x) => x.channel === ch).map((x) => x.id);
}, channel);
try {
  await ctx.addCookies([{ name: 'lang', value: 'zh-CN', domain: '127.0.0.1', path: '/' }]);
  await page.goto(`${ORIGIN}/login`, { waitUntil: 'domcontentloaded', timeout: 40000 });
  await page.evaluate(async ({ password }) => {
    const r = await fetch('/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password }) });
    const j = await r.json();
    localStorage.setItem('rayrag_token', j.data.access_token);
    document.cookie = 'rayrag_token=' + j.data.access_token + ';path=/;max-age=86400';
  }, { password: PASSWORD });
  await page.goto(`${ORIGIN}/user-setting/chat-channel`, { waitUntil: 'domcontentloaded', timeout: 40000 });
  await page.waitForTimeout(1200);
  mkdirSync('docs/ui-traversal/rayrag-audit', { recursive: true });

  // ── Nostr: Tag chips + placeholder parity ──
  const beforeNostr = await listIds('nostr');
  await page.locator("[data-testid='available-channel-card'][data-channel='nostr']").click({ timeout: 8000 });
  await page.waitForTimeout(700);
  results.nostrDialog = await page.evaluate(() => document.getElementById('chatChannelDialog').classList.contains('show'));
  const nostr = await page.evaluate(() => {
    const box = document.getElementById('cc-relays');
    const draft = box ? box.querySelector('.cc-tag-draft') : null;
    const key = document.getElementById('cc-private_key');
    return {
      tagBox: !!box,
      draftPlaceholder: draft ? draft.placeholder : null,
      keyPlaceholder: key ? key.placeholder : null,
    };
  });
  results.nostr = nostr;
  await page.screenshot({ path: 'docs/ui-traversal/rayrag-audit/cc-nostr-dialog-20260904.png' });
  const draft = page.locator('#cc-relays .cc-tag-draft');
  await draft.fill('wss://relay.damus.io');
  await draft.press('Enter');
  await draft.fill('wss://relay.nostr.info');
  await draft.press('Enter');
  await draft.fill('wss://relay.damus.io');
  await draft.press('Enter');
  await page.waitForTimeout(300);
  const chips = await page.evaluate(() => {
    const box = document.getElementById('cc-relays');
    return { chips: box.querySelectorAll('.cc-tag-chip').length, values: box.dataset.values };
  });
  results.chips = chips;
  await page.locator('#cc-private_key').fill('nsec1test123');
  await page.locator('#ccName').fill('CC-Probe-Nostr');
  await page.screenshot({ path: 'docs/ui-traversal/rayrag-audit/cc-nostr-chips-20260904.png' });
  await page.locator('#ccSave').click({ timeout: 8000 });
  await page.waitForTimeout(1500);
  const afterNostr = await listIds('nostr');
  const newNostr = afterNostr.filter((id) => !beforeNostr.includes(id));
  results.nostrCreated = newNostr.length;
  if (newNostr.length) created.push(newNostr[0]);
  const rec = await page.evaluate(async (id) => {
    const r = await fetch('/api/v1/chat-channels/' + id);
    const j = await r.json();
    return j.data || null;
  }, newNostr[0]);
  results.nostrRecord = rec ? { relays: rec.config && rec.config.credential && rec.config.credential.relays, key: rec.config && rec.config.credential && rec.config.credential.private_key } : null;
  await page.evaluate(() => closeChatChannelDialog());
  await page.waitForTimeout(400);

  // ── IRC: nested nickserv via dot-path keys ──
  const beforeIrc = await listIds('irc');
  await page.locator("[data-testid='available-channel-card'][data-channel='irc']").click({ timeout: 8000 });
  await page.waitForTimeout(700);
  const irc = await page.evaluate(() => ({
    account: !!document.getElementById('cc-nickserv.account'),
    password: !!document.getElementById('cc-nickserv.password'),
    accountEl: !!document.querySelector("[id='cc-nickserv.account']"),
    hostPlaceholder: (document.getElementById('cc-host') || {}).placeholder || null,
  }));
  results.ircFields = irc;
  await page.locator('#cc-host').fill('irc.libera.chat');
  await page.locator('#cc-port').fill('6697');
  await page.locator('#cc-nick').fill('probe-bot');
  await page.locator("[id='cc-nickserv.account']").fill('probe-user');
  await page.locator("[id='cc-nickserv.password']").fill('probe-pass');
  await page.locator('#ccName').fill('CC-Probe-Irc');
  await page.locator('#ccSave').click({ timeout: 8000 });
  await page.waitForTimeout(1500);
  const afterIrc = await listIds('irc');
  const newIrc = afterIrc.filter((id) => !beforeIrc.includes(id));
  results.ircCreated = newIrc.length;
  if (newIrc.length) created.push(newIrc[0]);
  const ircRec = await page.evaluate(async (id) => {
    const r = await fetch('/api/v1/chat-channels/' + id);
    const j = await r.json();
    return j.data || null;
  }, newIrc[0]);
  results.ircRecord = ircRec ? { nickserv: ircRec.config && ircRec.config.credential && ircRec.config.credential.nickserv, host: ircRec.config && ircRec.config.credential && ircRec.config.credential.host } : null;

  results.result = results.nostrDialog && results.nostr.tagBox && results.nostr.draftPlaceholder === 'wss://relay.damus.io' && results.nostr.keyPlaceholder === 'nsec1xxxx or hex' && chips.chips === 2 && chips.values === 'wss://relay.damus.io,wss://relay.nostr.info' && results.nostrCreated === 1 && Array.isArray(results.nostrRecord && results.nostrRecord.relays) && results.nostrRecord.relays.length === 2 && results.ircFields.account && results.ircFields.password && results.ircCreated === 1 && results.ircRecord && results.ircRecord.nickserv && results.ircRecord.nickserv.account === 'probe-user' && results.ircRecord.nickserv.password === 'probe-pass' ? 'PASS' : 'FAIL';
} catch (e) {
  results.error = String(e && e.message ? e.message : e);
  results.result = 'FAIL';
} finally {
  try {
    for (const id of created) {
      await page.evaluate(async (rid) => { await fetch('/api/v1/chat-channels/' + rid, { method: 'DELETE' }); }, id);
    }
    results.cleaned = created.length;
  } catch (e) { results.cleaned = 'err:' + String(e); }
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await browser.close();
process.exit(results.result && results.result.startsWith('PASS') ? 0 : 1);
