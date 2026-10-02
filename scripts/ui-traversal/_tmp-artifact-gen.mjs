import { chromium } from 'playwright-core';
const CDP='http://127.0.0.1:16002', ORIGIN='http://127.0.0.1:9390', PW='rayrag-smoke-admin';
const KB=process.env.KB||'572225ab-7028-4ee5-855f-472833170fc0';
const browser=await chromium.connectOverCDP(CDP); const ctx=browser.contexts()[0]; const page=await ctx.newPage();
const errors=[]; page.on('pageerror',e=>errors.push(String(e)));
await page.goto(`${ORIGIN}/dataset/compilation/${KB}`,{waitUntil:'domcontentloaded',timeout:60000});
await page.waitForTimeout(2500);
if(await page.locator('#email').count()){
  await page.fill('#email','admin@rayrag.local'); await page.fill('#password',PW);
  await page.click('button[type=submit]'); await page.waitForTimeout(4000);
  await page.goto(`${ORIGIN}/dataset/compilation/${KB}`,{waitUntil:'domcontentloaded'}); await page.waitForTimeout(2500);
}
const hasBtn=await page.locator('#cwpGenerate').count();
let before='', after='';
if(hasBtn){
  before=await page.locator('#cwpMsg').innerText().catch(()=> '');
  const disabled = await page.locator('#cwpGenerate').isDisabled();
  if (!disabled) { await page.click('#cwpGenerate'); await page.waitForTimeout(1500); }
  after = await page.locator('#cwpMsg').innerText().catch(()=> '');
  var state = { disabled, after };
}
const kbState = await page.evaluate(async () => {
  const response = await fetch('/api/v1/datasets/' + encodeURIComponent(window.kbId || '') );
  const data = await response.json();
  const kb = (data && data.data) || {};
  return { chunk_count: kb.chunk_count, pipeline_id: kb.pipeline_id || '' };
}).catch(() => ({}));
console.log(JSON.stringify({url:page.url(),hasGenerate:!!hasBtn,before,after,kbState,errors},null,1));
await page.close(); await browser.close();
