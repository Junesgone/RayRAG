import { chromium } from "playwright-core";
const ORIGIN = "http://127.0.0.1:9390";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const errs = [];
page.on("pageerror", (e) => errs.push(String(e.message)));
page.on("framenavigated", (frame) => console.log("navigated ->", frame.url()));
page.on("response", (response) => { if (response.url().includes("error=") || response.url().endsWith("/login")) console.log("response", response.status(), response.url()); });
await page.goto(`${ORIGIN}/?error=invalid_state`, { waitUntil: "commit", timeout: 40000 });
for (const delay of [200, 500, 1500]) {
  await page.waitForTimeout(delay === 200 ? 200 : delay - 200);
  console.log(delay, JSON.stringify(await page.evaluate(() => ({
    url: location.href,
    hasToast: !!document.getElementById("oauthToast"),
    toastText: (document.getElementById("oauthToast") || {}).textContent || null,
    toastHidden: (document.getElementById("oauthToast") || {}).hidden,
    bodyChildren: document.body ? document.body.children.length : -1,
  })).catch((error) => String(error))));
}
console.log("errors:", errs);
await page.close(); await b.close();
