// Drives the chat composer's attach control the way a user would.
import { chromium } from "playwright-core";
const ORIGIN = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(String(error).slice(0, 140)));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password }) });
  const body = await response.json();
  if (body.code === 0) { localStorage.setItem("rayrag_token", body.data.access_token); document.cookie = "rayrag_token=" + body.data.access_token + "; path=/"; }
}, process.env.RAYRAG_LOGIN_PASSWORD);
const chat = await page.evaluate(async () => {
  const created = await (await fetch("/api/v1/chats", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "attach-ui-check" }) })).json();
  return created.data.id;
});
await page.goto(ORIGIN + "/chat/" + chat, { waitUntil: "networkidle", timeout: 40000 });
await page.waitForTimeout(700);
const hasControl = await page.evaluate(() => !!document.getElementById("chatAttach"));
await page.setInputFiles("#chatAttach", "/tmp/ui-attach.txt");
await page.waitForTimeout(1500);
const state = await page.evaluate(() => ({
  status: (document.getElementById("chatAttachStatus") || {}).textContent,
  linkDisplay: (document.getElementById("chatAttachLink") || {}).style.display,
  linkHref: (document.getElementById("chatAttachLink") || {}).href,
}));
console.log(JSON.stringify({ hasControl, state, errors }, null, 1));
// The attachment behind that link is downloadable.
const download = await page.evaluate(async (href) => {
  const response = await fetch(href);
  return { status: response.status, type: response.headers.get("content-type"), body: (await response.text()).trim() };
}, state.linkHref);
console.log(JSON.stringify({ download }, null, 1));
await fetch(ORIGIN + "/api/v1/chats/" + chat, { method: "DELETE", headers: { Authorization: "Bearer " + (await page.evaluate(() => localStorage.getItem("rayrag_token"))) } }).catch(() => {});
await page.close(); await context.close(); await browser.close();
process.exit(errors.length ? 1 : 0);
