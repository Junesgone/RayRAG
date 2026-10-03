import { chromium } from "playwright-core";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const context = await browser.newContext({ viewport: { width: 480, height: 900 } });
const page = await context.newPage();
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const dataset = await page.evaluate(async () => (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + localStorage.getItem("rayrag_token") } })).json()).data[0].id);
await page.goto("http://127.0.0.1:9390/dataset/configuration/" + dataset, { waitUntil: "networkidle", timeout: 40000 });
await page.waitForTimeout(1200);
console.log(JSON.stringify(await page.evaluate(() => {
  const viewport = window.innerWidth;
  const wide = [...document.querySelectorAll("table")].find((table) => table.getBoundingClientRect().width > viewport + 2);
  if (!wide) return { none: true };
  const chain = [];
  let node = wide;
  while (node && node !== document.body) {
    const style = getComputedStyle(node);
    chain.push(`${node.tagName.toLowerCase()}${node.id ? "#" + node.id : ""}.${(typeof node.className === "string" ? node.className : "").split(/\s+/)[0] || ""} [overflowX=${style.overflowX} w=${Math.round(node.getBoundingClientRect().width)}]`);
    node = node.parentElement;
  }
  return { outer: wide.outerHTML.slice(0, 150), chain, articles: document.querySelectorAll("article").length, tab: location.hash };
}), null, 1));
await page.close(); await context.close(); await browser.close();
