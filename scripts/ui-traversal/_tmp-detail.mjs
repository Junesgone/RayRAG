import { chromium } from "playwright-core";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const browser = await chromium.connectOverCDP(CDP);
const context = await browser.newContext({ viewport: { width: 480, height: 900 } });
const page = await context.newPage();
const seen = [];
page.on("pageerror", (error) => seen.push("PAGEERROR: " + String(error).slice(0, 300)));
page.on("console", (message) => { if (message.type() === "error") seen.push("CONSOLE: " + message.text().slice(0, 200)); });
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const dataset = await page.evaluate(async () => (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + localStorage.getItem("rayrag_token") } })).json()).data[0].id);
for (const path of ["/search", "/kbs/" + dataset]) {
  seen.length = 0;
  await page.goto(ORIGIN + path, { waitUntil: "networkidle", timeout: 40000 });
  await page.waitForTimeout(1200);
  console.log("== " + path + " ==");
  console.log(seen.length ? seen.join("\n") : "(no errors)");
}
for (const path of ["/agent-templates", "/dataset/configuration/" + dataset]) {
  await page.goto(ORIGIN + path, { waitUntil: "networkidle", timeout: 40000 });
  await page.waitForTimeout(1000);
  const wide = await page.evaluate(() => {
    const viewport = window.innerWidth;
    const out = [];
    for (const element of document.querySelectorAll("body *")) {
      const rect = element.getBoundingClientRect();
      if (rect.width <= viewport + 2 || rect.width <= 0) continue;
      const scrolls = (node) => { const s = getComputedStyle(node); return s.overflowX === "auto" || s.overflowX === "scroll"; };
      let parent = element.parentElement, contained = false;
      while (parent) { if (scrolls(parent)) { contained = true; break; } parent = parent.parentElement; }
      if (contained) continue;
      out.push({ tag: element.tagName.toLowerCase(), id: element.id, cls: (typeof element.className === "string" ? element.className : "").slice(0, 50), width: Math.round(rect.width) });
    }
    return out.slice(0, 6);
  });
  console.log("== " + path + " (wide, not in a scroll container) ==");
  console.log(JSON.stringify(wide));
}
await page.close(); await context.close(); await browser.close();
