// Visits every page reachable from the sidebar at a desktop and a phone width, and records what a
// user would notice: page errors, horizontal overflow, elements wider than the viewport, and whether
// the page uses card-like containers at all.
import { chromium } from "playwright-core";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const WIDTHS = [
  ["desktop", 1280, 900],
  ["phone", 480, 900],
];

const browser = await chromium.connectOverCDP(CDP);
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();

const errors = [];
page.on("pageerror", (error) => errors.push(String(error).slice(0, 120)));

await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 30000 });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email: "admin@rayrag.local", password }),
  });
  const body = await response.json();
  if (body.code === 0) {
    localStorage.setItem("rayrag_token", body.data.access_token);
    document.cookie = "rayrag_token=" + body.data.access_token + "; path=/";
  }
}, PASSWORD);

// The sidebar links what it links; the page routes are known, so both are used. Deep links are
// discovered from the API so they point at real records instead of 404s.
const STATIC = [
  "/", "/datasets", "/chats", "/searches", "/agents", "/agent-templates", "/memories", "/files",
  "/skills", "/datasources", "/providers", "/search", "/settings", "/status",
  "/user-setting", "/user-setting/model", "/user-setting/mcp", "/user-setting/team",
  "/user-setting/profile", "/user-setting/api", "/user-setting/chat-channel",
  "/admin", "/admin/services", "/admin/users", "/admin/whitelist", "/admin/roles",
  "/admin/monitoring", "/admin/sandbox-settings",
];
const discovered = await page.evaluate(async () => {
  const token = localStorage.getItem("rayrag_token") || "";
  const get = async (path) => {
    try {
      const response = await fetch(path, { headers: { Authorization: "Bearer " + token } });
      const body = await response.json();
      return body.data;
    } catch {
      return null;
    }
  };
  const list = (data, key) => (Array.isArray(data) ? data : (data && data[key]) || []);
  const paths = [];
  const datasets = list(await get("/api/v1/datasets"), "datasets");
  if (datasets[0]) {
    paths.push(`/dataset/${datasets[0].id}`, `/dataset/files/${datasets[0].id}`,
               `/dataset/configuration/${datasets[0].id}`, `/kbs/${datasets[0].id}`);
  }
  const chats = list(await get("/api/v1/chats"), "chats");
  if (chats[0]) paths.push(`/chat/${chats[0].id}`, `/chats/${chats[0].id}`);
  const agents = list(await get("/api/v1/agents"), "canvas");
  if (agents[0]) paths.push(`/agent/${agents[0].id}`);
  const searches = list(await get("/api/v1/searches"), "search_apps");
  if (searches[0]) paths.push(`/searches/${searches[0].id}`);
  return paths;
});
await page.goto(ORIGIN + "/", { waitUntil: "networkidle", timeout: 40000 });
const linked = await page.evaluate(() =>
  [...document.querySelectorAll("a[href^='/']")]
    .map((a) => new URL(a.href).pathname)
    .filter((path) => !path.startsWith("/login") && !path.startsWith("/api"))
);
const paths = [...new Set([...STATIC, ...linked, ...discovered])];

const rows = [];
for (const path of paths) {
  for (const [label, width, height] of WIDTHS) {
    await page.setViewportSize({ width, height });
    const before = errors.length;
    let status = 0;
    try {
      const response = await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
      status = response ? response.status() : 0;
      await page.waitForTimeout(700);
    } catch (error) {
      rows.push({ path, viewport: label, status: 0, note: String(error).slice(0, 60) });
      continue;
    }
    const data = await page.evaluate(() => {
      // A table wider than the screen is fine when its own container scrolls; what a user notices is
      // the page itself moving sideways, so only uncontained elements count.
      const insideHorizontalScroller = (element) => {
        let node = element.parentElement;
        while (node) {
          const overflow = getComputedStyle(node).overflowX;
          if (overflow === "auto" || overflow === "scroll") return true;
          node = node.parentElement;
        }
        return false;
      };
      const viewport = window.innerWidth;
      const oversized = [];
      let cards = 0;
      for (const element of document.querySelectorAll("body *")) {
        const rect = element.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) continue;
        if (rect.width > viewport + 2 && !insideHorizontalScroller(element)) {
          const name =
            element.tagName.toLowerCase() +
            (element.id ? "#" + element.id : "") +
            (typeof element.className === "string" && element.className
              ? "." + element.className.split(/\s+/)[0]
              : "");
          if (oversized.length < 3 && !oversized.includes(name)) oversized.push(name);
        }
        const style = getComputedStyle(element);
        const radius = parseFloat(style.borderTopLeftRadius) || 0;
        const shadow = style.boxShadow && style.boxShadow !== "none";
        if (radius >= 6 && (shadow || parseFloat(style.borderTopWidth) > 0) && rect.height > 40) cards += 1;
      }
      return {
        overflowX: document.documentElement.scrollWidth - viewport,
        oversized,
        cards,
        text: (document.body.innerText || "").trim().length,
      };
    });
    rows.push({
      path,
      viewport: label,
      status,
      errors: errors.length - before,
      overflowX: data.overflowX,
      oversized: data.oversized,
      cards: data.cards,
      text: data.text,
    });
  }
}

const bad = rows.filter((row) => row.status >= 400 || row.errors || row.overflowX > 2 || (row.oversized || []).length);
console.log(JSON.stringify({ pages: paths.length, checked: rows.length, problems: bad.length }, null, 1));
for (const row of bad) console.log(JSON.stringify(row));
if (!bad.length) console.log("no page errors, no horizontal overflow, nothing wider than the viewport");
process.exit(bad.length ? 1 : 0);
