/**
 * Every dataset destination must load without a page error.
 *
 * A JS error on `/dataset/knowledge-graph/{id}` (an unguarded `nav4` lookup) survived several
 * rounds of the sidebar and layout suites: both only checked status codes and geometry, so a
 * page that throws still looked green. This walks the five dataset destinations upstream
 * defines and fails if any of them reports an error or fails to render its panel.
 */
import { chromium } from 'playwright-core';

const CDP = process.env.RAYRAG_CDP_ENDPOINT || 'http://127.0.0.1:16002';
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || 'http://127.0.0.1:9390';
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || 'rayrag-smoke-admin';
const KB = process.env.RAYRAG_DATASET_ID || '572225ab-7028-4ee5-855f-472833170fc0';
const DESTINATIONS = [
  ['files', 'tab0'],
  ['logs', 'tab1'],
  ['retrieval', 'tab2'],
  ['configuration', 'tab3'],
  ['knowledge-graph', 'tab4'],
  ['compilation', 'tab5'],
];

const browser = await chromium.connectOverCDP(CDP);
const page = await browser.contexts()[0].newPage();
const failures = [];
const errors = [];
page.on('pageerror', (e) => errors.push(String(e)));

const login = async () => {
  if (await page.locator('#email').count()) {
    await page.fill('#email', 'admin@rayrag.local');
    await page.fill('#password', PASSWORD);
    await page.click('button[type=submit]');
    await page.waitForTimeout(4000);
  }
};

const results = [];
for (const [segment, panel] of DESTINATIONS) {
  errors.length = 0;
  await page.goto(`${ORIGIN}/dataset/${segment}/${KB}`, { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.waitForTimeout(2500);
  await login();
  if (!page.url().includes(`/dataset/${segment}/${KB}`)) {
    await page.goto(`${ORIGIN}/dataset/${segment}/${KB}`, { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(2500);
  }
  const state = await page.evaluate((id) => {
    const node = document.getElementById(id);
    return { present: !!node, visible: node ? node.style.display !== 'none' : false };
  }, panel);
  results.push({ segment, panel, ...state, errors: [...errors] });
  if (!state.present || !state.visible) failures.push(`${segment}: panel ${panel} not shown`);
  if (errors.length) failures.push(`${segment}: ${errors[0]}`);
}

await page.close();
await browser.close();
for (const row of results) {
  console.log(`${row.segment.padEnd(16)} ${row.panel} visible=${row.visible} errors=${row.errors.length}`);
}
if (failures.length) {
  for (const failure of failures) console.log(`FAIL ${failure}`);
  console.log(`dataset pages: FAILED (${failures.length})`);
  process.exit(1);
}
console.log(`dataset pages: OK (${results.length} destinations, 0 page errors)`);
