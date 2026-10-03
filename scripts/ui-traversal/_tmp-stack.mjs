import { chromium } from "playwright-core";
const ORIGIN = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
page.on("pageerror", (error) => console.log("PAGEERROR:", error.stack ? error.stack.split("\n").slice(0, 4).join(" | ") : error.message));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const dataset = await page.evaluate(async () => (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + localStorage.getItem("rayrag_token") } })).json()).data[0].id);
for (const path of ["/kbs/" + dataset, "/search"]) {
  console.log("== " + path + " ==");
  await page.goto(ORIGIN + path, { waitUntil: "networkidle", timeout: 40000 });
  await page.waitForTimeout(1200);
  // Which script blocks parsed? Then find where each suspicious function is defined vs called.
  const info = await page.evaluate(() => {
    const scripts = [...document.querySelectorAll("script:not([src])")].map((s, index) => ({ index, length: s.textContent.length, definesLoad: s.textContent.includes("function dsTagLoad"), callsLoad: s.textContent.includes("dsTagLoad("), fn: (s.textContent.match(/function ([A-Za-z_$][\w$]*)\(/g) || []).length }));
    return { scripts, hasDsTagLoad: typeof window.dsTagLoad };
  });
  console.log(JSON.stringify(info));
}
await page.close(); await context.close(); await browser.close();
