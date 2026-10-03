// Drives the new Version history card the way a user would: open it, create a snapshot from the
// real uncommitted changes, and check the row appears.
import { chromium } from "playwright-core";
const ORIGIN = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(String(error).slice(0, 120)));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const dataset = await page.evaluate(async () => (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + localStorage.getItem("rayrag_token") } })).json()).data[0].id);
await page.goto(ORIGIN + "/kbs/" + dataset, { waitUntil: "networkidle", timeout: 40000 });
await page.waitForTimeout(800);
const panelBefore = await page.evaluate(() => document.getElementById("commitPanel").style.display);
await page.click("text=Show snapshots");
await page.waitForTimeout(900);
const listed = await page.evaluate(() => ({
  display: document.getElementById("commitPanel").style.display,
  rows: document.querySelectorAll("#commitRows tr").length,
  status: document.getElementById("commitStatus").textContent,
}));
page.once("dialog", (dialog) => dialog.accept("ui snapshot"));
await page.click("text=Create snapshot");
await page.waitForTimeout(1500);
const after = await page.evaluate(() => ({
  rows: document.querySelectorAll("#commitRows tr").length,
  status: document.getElementById("commitStatus").textContent,
}));
console.log(JSON.stringify({ panelBefore, listed, after, errors }, null, 1));
await page.close(); await context.close(); await browser.close();
process.exit(errors.length ? 1 : 0);
