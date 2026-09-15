import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1400, height: 900 });
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1800);
console.log(JSON.stringify(await page.evaluate(() => ({
  channelsVisible: !document.getElementById("authChannels").hidden,
  buttons: Array.from(document.querySelectorAll(".auth-channel-btn")).map((x) => x.textContent.trim()),
  formVisible: !document.getElementById("loginForm").hidden,
})), null, 1));
await page.screenshot({ path: "/tmp/rayrag-shots/rayrag-login-channels.png" });
await page.close();
process.exit(0);
