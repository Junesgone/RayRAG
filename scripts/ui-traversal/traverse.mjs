// RAGFlow UI traversal: visits every route, records all interactive elements,
// safely clicks each one, and records what appears (modals/dropdowns/tooltips/route change).
import { chromium } from 'playwright-core';
import { writeFileSync, mkdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';

const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const BASE = process.env.RAGFLOW_UI || 'http://127.0.0.1:11526';
const OUT_DIR = path.resolve(process.env.OUT_DIR || '/home/junesgone/.openclaw/workspace/RayRAG/docs/ui-traversal');
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

// Routes to traverse (from web/src/routes.tsx, RAGFlow v0.27.2). Parameterized
// detail routes are covered by filtered second passes with real ids.
const ROUTES = [
  '/',
  '/home',
  '/datasets',
  '/chats',
  '/chats/share',
  '/chats/widget',
  '/searches',
  '/search/share',
  '/agents',
  '/agent-list',
  '/agent',
  '/agent/share',
  '/agent-templates',
  '/memories',
  '/files',
  '/files/skills',
  '/compilation-templates/edit-next',
  '/dataflow-result',
  '/user-setting',
  '/user-setting/profile',
  '/user-setting/locale',
  '/user-setting/model',
  '/user-setting/team',
  '/user-setting/api',
  '/user-setting/mcp',
  '/user-setting/data-source',
  '/user-setting/chat-channel',
  '/admin',
  '/admin/services',
  '/admin/users',
  '/admin/sandbox-settings',
  '/admin/whitelist',
  '/admin/roles',
  '/admin/monitoring',
];

// Clicks with these labels are treated as potentially destructive and are only
// opened (menus/popups) then dismissed, never confirmed.
const DESTRUCTIVE = /delete|remove|discard|confirm|reset|revoke|logout|log out|stop|purge|drop|cancel|删除|移除|清空|确认|撤销|登出|停止|销毁/i;

// Overlay selectors that indicate a popup/modal/drawer appeared after a click.
const OVERLAY_SEL = [
  '.ant-modal:visible',
  '.ant-modal-root:visible .ant-modal',
  '.ant-drawer:visible',
  '.ant-dropdown:visible',
  '.ant-popover:visible',
  '.ant-message:visible',
  '.ant-notification:visible',
  '.ant-tooltip:visible',
  '.ant-select-dropdown:visible',
];

const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0] || (await browser.newContext());
let page = ctx.pages().find(p => p.url().startsWith(BASE)) || (await ctx.newPage());
await page.setViewportSize({ width: 1600, height: 900 }).catch(() => {});

const report = { base: BASE, generated_at: new Date().toISOString(), routes: [] };
const FILTER = (process.env.ROUTE_FILTER || '').split(',').filter(Boolean);
const reportPath = path.join(OUT_DIR, 'ragflow-ui-map.json');
if (FILTER.length && existsSync(reportPath)) {
  try {
    const prev = JSON.parse(readFileSync(reportPath, 'utf8'));
    report.routes = (prev.routes || []).filter(r => !FILTER.includes(r.route));
  } catch (e) { console.log('merge prev map failed:', String(e)); }
}
if (FILTER.length) {
  ROUTES.splice(0, ROUTES.length, ...FILTER);
}

function visibleCount(doc, sels) {
  const out = [];
  for (const s of sels) {
    try {
      const els = doc.querySelectorAll(s);
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) out.push(el);
      }
    } catch {}
  }
  return out;
}

function snapshotPopups(doc) {
  const pops = visibleCount(doc, OVERLAY_SEL);
  return pops.map(el => {
    const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 400);
    return { sel: el.className || el.tagName, text: t };
  }).filter(p => p.text.length > 0);
}

async function restore(page, route) {
  // close popups/modals, then ensure we're back on the route
  for (let i = 0; i < 3; i++) {
    await page.keyboard.press('Escape').catch(() => {});
    await sleep(250);
    const still = await page.evaluate((sels) => {
      for (const s of sels) {
        const els = document.querySelectorAll(s);
        for (const el of els) {
          const r = el.getBoundingClientRect();
          if (r.width > 0) return true;
        }
      }
      return false;
    }, OVERLAY_SEL).catch(() => false);
    if (!still) break;
    // try to click a close button inside a visible modal
    const closed = await page.evaluate(() => {
      const btns = Array.from(document.querySelectorAll('.ant-modal button, .ant-drawer button'))
        .filter(b => /close|cancel|取消|关闭|close/i.test(b.textContent || ''));
      if (btns[0]) { btns[0].click(); return true; }
      return false;
    }).catch(() => false);
    if (!closed) {
      const x = await page.locator('.ant-modal-close, .ant-drawer-close').first().click({ timeout: 300 }).catch(() => null);
      if (!x) break;
    }
  }
  if (!page.url().includes(route)) {
    await page.goto(BASE + route, { waitUntil: 'domcontentloaded', timeout: 60000 }).catch(() => {});
    await sleep(2500);
  }
}

function describe(doc, el) {
  const text = (el.innerText || el.textContent || el.getAttribute('aria-label') || el.getAttribute('title') || '').trim().replace(/\s+/g, ' ').slice(0, 120);
  const role = el.getAttribute('role') || el.tagName.toLowerCase();
  const cls = (el.className && el.className.baseVal !== undefined) ? '' : (el.className || '');
  return { text: text || '(icon)', role: String(role), cls: String(cls).slice(0, 120) };
}

async function collectElements(page) {
  return page.evaluate(() => {
    const sels = [
      'button',
      'a[href]',
      '[role="button"]',
      '.ant-tabs-tab',
      '.ant-select:not(.ant-select-disabled) .ant-select-selector',
      'summary',
      '[data-tip]',
      '.tip',
      '.ant-dropdown-trigger',
      'input[type="checkbox"]',
      'input[type="radio"]',
      'input[type="text"]:not([readonly])',
      'input[type="search"]',
      'textarea',
      'label[for]',
    ];
    const seen = new Set();
    const out = [];
    const push = (el, kind) => {
      if (seen.has(el)) return;
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) return;
      if (el.closest('.ant-modal-mask')) return;
      const text = (el.innerText || el.textContent || el.getAttribute('aria-label') || el.getAttribute('title') || '').trim().replace(/\s+/g, ' ').slice(0, 120);
      const role = el.getAttribute('role') || el.tagName.toLowerCase();
      if (!text && !/button|a|summary/.test(String(role))) return; // skip bare iconless elements without a label
      seen.add(el);
      out.push({ kind, text: text || '(icon)', role: String(role), elId: out.length });
    };
    for (const s of sels) {
      for (const el of document.querySelectorAll(s)) push(el, s);
    }
    return out;
  }).catch(e => { console.error('collect err', String(e)); return []; });
}

const POPUP_SEL_INPAGE = OVERLAY_SEL;

async function clickProbe(page, route, elIndex, elDesc) {
  // find the element by index within the same DOM (re-collect to keep stable)
  const res = await page.evaluate(async (args) => {
    const { kind, text } = args;
    // Re-select the same set and find matching element by text+kind prefix.
    const sels = { button: 'button', 'a[href]': 'a[href]', 'a': 'a[href]', '[role="button"]': '[role="button"]', '.ant-tabs-tab': '.ant-tabs-tab', summary: 'summary', '.tip': '.tip', '[data-tip]': '[data-tip]' }[kind] || kind;
    let candidates = [];
    try { candidates = Array.from(document.querySelectorAll(sels)); } catch { return { err: 'selector ' + kind }; }
    const el = candidates.find(e => ((e.innerText || e.textContent || e.getAttribute('aria-label') || '').trim().replace(/\s+/g, ' ')).slice(0, 120) === text);
    if (!el) return { err: 'element not found' };
    const before = new Set(Array.from(document.querySelectorAll('[class*="ant"], .modal, [role="dialog"]')).map(n => n));
    el.scrollIntoView({ block: 'center' });
    try { el.click(); } catch (e) { return { err: 'click: ' + e }; }
    await new Promise(r => setTimeout(r, 900));
    // detect overlays that became visible
    const popups = [];
    for (const s of [' .ant-modal', ' .ant-drawer', ' .ant-dropdown', ' .ant-popover', ' .ant-select-dropdown', ' .ant-message', ' .ant-notification']) {
      for (const el of document.querySelectorAll(s)) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) {
          const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 300);
          if (t) popups.push(t);
        }
      }
    }
    return { popups };
  }, elDesc).catch(e => ({ err: String(e) }));
  const urlChanged = page.url() !== null;
  return res;
}

// Simpler & more robust approach: do the click via Playwright locators by text, not raw evaluate.
async function probeClick(page, route, elDesc) {
  const { text, role } = elDesc;
  let locator;
  const t = text === '(icon)' ? null : text;
  if (role === 'button') locator = t ? page.locator(`button:has-text("${t.slice(0, 40)}")`).first() : null;
  else if (role === 'a' || role === 'a[href]') locator = t ? page.locator(`a:has-text("${t.slice(0, 40)}")`).first() : null;
  else if (role === 'ant-tabs-tab') locator = t ? page.locator(`.ant-tabs-tab:has-text("${t.slice(0, 40)}")`).first() : null;
  else if (role === 'summary') locator = t ? page.locator(`summary:has-text("${t.slice(0, 40)}")`).first() : null;
  else if (t && /tip/.test(role)) locator = page.locator(`[data-tip="${t.slice(0, 40)}"], .tip:has-text("${t.slice(0, 40)}")`).first();
  else return { skipped: 'no-locator' };
  if (!locator || (await locator.count().catch(() => 0)) === 0) return { skipped: 'not-found' };
  const beforeUrl = page.url();
  const overlaysBefore = await overlayTexts(page);
  let clickErr = null;
  await locator.click({ timeout: 2500 }).catch(e => clickErr = String(e));
  if (clickErr) return { skipped: clickErr };
  await sleep(900);
  const overlaysAfter = await overlayTexts(page);
  const newOverlays = overlaysAfter.filter(o => !overlaysBefore.includes(o));
  const urlNow = page.url();
  const urlChanged = urlNow !== beforeUrl;
  await restore(page, route).catch(() => {});
  return { popped: newOverlays, urlChanged, navTo: urlChanged ? urlNow : undefined };
}

async function overlayTexts(page) {
  return page.evaluate(() => {
    const out = [];
    for (const s of [' .ant-modal', ' .ant-drawer', ' .ant-dropdown', ' .ant-popover', ' .ant-select-dropdown', ' .ant-message', ' .ant-notification']) {
      for (const el of document.querySelectorAll(s)) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) {
          const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 300);
          if (t) out.push(t);
        }
      }
    }
    return out;
  }).catch(() => []);
}

mkdirSync(OUT_DIR, { recursive: true });

async function ensurePage() {
  if (!page.isClosed()) return page;
  page = await ctx.newPage();
  await page.setViewportSize({ width: 1600, height: 900 }).catch(() => {});
  return page;
}

for (const route of ROUTES) {
  const rec = { route, ok: false, elements: [], note: '' };
  await ensurePage().catch(e => { rec.note = 'page closed: ' + String(e); });
  if (rec.note) { report.routes.push(rec); console.log(`route ${route} skipped: ${rec.note}`); continue; }
  try {
    await page.goto(BASE + route, { waitUntil: 'domcontentloaded', timeout: 60000 });
    await sleep(3000);
    const title = await page.title().catch(() => '');
    rec.ok = true;
    rec.title = title;
    rec.body_text_head = await page.evaluate(() => (document.body.innerText || '').slice(0, 600)).catch(() => '');
    await page.screenshot({ path: path.join(OUT_DIR, `shot-${route.replace(/\//g, '_').replace(/^_/, '')}.png`) }).catch(() => {});

    const els = await collectElements(page);
    // de-dup by text+role
    const seen = new Set();
    const uniq = [];
    for (const e of els) {
      const k = e.text + '|' + e.role;
      if (seen.has(k)) continue;
      seen.add(k);
      uniq.push(e);
    }
    // Cap probes per route to keep runtime bounded.
    const CANDIDATES = uniq.filter(e => !DESTRUCTIVE.test(e.text)).slice(0, 30);
    rec.total_interactive = uniq.length;
    rec.probed = CANDIDATES.length;

    for (const e of CANDIDATES) {
      const entry = { text: e.text, role: e.role, result: null };
      if (e.kind.startsWith('input') || e.kind.startsWith('textarea') || e.kind.startsWith('label')) {
        entry.result = 'form-field (not clicked)';
      } else {
        const r = await probeClick(page, route, e);
        if (r.skipped) entry.result = 'skipped: ' + r.skipped;
        else {
          entry.result = [];
          if (r.popped && r.popped.length) entry.result.push('popup: ' + r.popped.join(' | ').slice(0, 300));
          if (r.urlChanged) entry.result.push('nav -> ' + (r.navTo || '').replace(BASE, ''));
          if (!entry.result.length) entry.result.push('no visible popup or nav');
        }
      }
      els && 0;
      rec.elements.push(entry);
      if (DESTRUCTIVE.test(e.text) && entry.result !== 'form-field (not clicked)') continue;
      // re-ensure we are on the route before the next probe
      if (!page.url().includes(route)) { await restore(page, route).catch(() => {}); }
    }
  } catch (e) {
    rec.note = String(e);
  }
  report.routes.push(rec);
  console.log(`[${report.routes.length}/${ROUTES.length}] ${route} ok=${rec.ok} total=${rec.total_interactive} probed=${rec.probed} note=${rec.note || ''}`);
  // write incrementally
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
}

writeFileSync(reportPath, JSON.stringify(report, null, 2));
console.log('done ->', reportPath);
await browser.close();

