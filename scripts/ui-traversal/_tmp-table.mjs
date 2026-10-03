import { chromium } from "playwright-core";
const ORIGIN = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const context = await browser.newContext({ viewport: { width: 480, height: 900 } });
const page = await context.newPage();
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const dataset = await page.evaluate(async () => (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + localStorage.getItem("rayrag_token") } })).json()).data[0].id);
await page.goto(ORIGIN + "/dataset/configuration/" + dataset, { waitUntil: "networkidle", timeout: 40000 });
await page.waitForTimeout(1200);
console.log(JSON.stringify(await page.evaluate(() => {
  const viewport = window.innerWidth;
  return [...document.querySelectorAll("table")].map((table) => {
    const rect = table.getBoundingClientRect();
    const parent = table.parentElement;
    const style = parent ? getComputedStyle(parent) : {};
    return {
      width: Math.round(rect.width),
      over: Math.round(rect.width - viewport),
      parentTag: parent ? parent.tagName.toLowerCase() : "-",
      parentClass: parent && typeof parent.className === "string" ? parent.className.slice(0, 40) : "",
      parentOverflowX: style.overflowX || "-",
      firstHeader: (table.querySelector("th") || {}).textContent || "",
      siblings: table.parentElement ? table.parentElement.children.length : 0,
      inScrollable: (() => { let node = table.parentElement; while (node) { const s = getComputedStyle(node); if (s.overflowX === "auto" || s.overflowX === "scroll") return true; node = node.parentElement; } return false; })(),
    };
  });
}), null, 1));
await page.close(); await context.close(); await browser.close();
