// Click through every global-navigation, user-setting sidebar and dataset
// sidebar entry like a user, recording render health for each target page.
//
// A page is "healthy" when it returns 200, paints content inside the main
// column, raises no page error and does not overflow horizontally.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/sidebar-routes-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));

const out = { groups: {}, pages: [], pageErrors };
const slug = (path) => path.replace(/[^a-z0-9]+/gi, "-").replace(/^-|-$/g, "") || "root";

async function inspect(path) {
  const errors = [];
  const onError = (e) => errors.push(String(e).slice(0, 180));
  page.on("pageerror", onError);
  let status = 0;
  const response = await page
    .goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 })
    .catch(() => null);
  if (response) status = response.status();
  await page.waitForTimeout(1600);
  const state = await page.evaluate(() => {
    const main = document.querySelector(".ct") || document.body;
    const rect = main.getBoundingClientRect();
    const heading = document.querySelector(".ct h1, .ct h2, h1, h2");
    return {
      mainWidth: Math.round(rect.width),
      mainHeight: Math.round(rect.height),
      textLength: (main.textContent || "").replace(/\s+/g, " ").trim().length,
      heading: heading ? (heading.textContent || "").replace(/\s+/g, " ").trim().slice(0, 60) : "",
      docScrollW: document.documentElement.scrollWidth,
      clientW: document.documentElement.clientWidth,
      datasetSidebar: Array.from(document.querySelectorAll(".ds-nav a")).length,
    };
  });
  page.off("pageerror", onError);
  const entry = { path, status, ...state, errors };
  await page.screenshot({ path: `${SHOT}/rayrag-nav-${slug(path)}.png` });
  out.pages.push(entry);
  return entry;
}

try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const ok = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { password: PASSWORD },
  );
  if (!ok) throw new Error("login failed");
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.clearBrowserCache").catch(() => {});

  out.groups.header = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets");
    const data = await res.json();
    return { datasetId: (data.data || [])[0]?.id || "" };
  });
  const home = await inspect("/");
  out.groups.header.links = await page.evaluate(() =>
    Array.from(document.querySelectorAll("nav.gnav a[href]"))
      .map((a) => a.getAttribute("href"))
      .filter((href) => href && href.startsWith("/")),
  );
  const settings = await inspect("/user-setting");
  out.groups.userSetting = await page.evaluate(() =>
    Array.from(document.querySelectorAll("aside a[href^='/user-setting'], .user-setting-nav-item[href]"))
      .map((a) => a.getAttribute("href"))
      .filter((href, index, all) => href && all.indexOf(href) === index),
  );
  out.groups.dataset = [];
  if (out.groups.header.datasetId) {
    const detail = await inspect(`/dataset/files/${out.groups.header.datasetId}`);
    out.groups.dataset = await page.evaluate(() =>
      Array.from(document.querySelectorAll(".ds-nav a[href]")).map((a) => a.getAttribute("href")),
    );
    out.groups.datasetDetailHeight = detail.mainHeight;
  }
  out.groups.memory = [];
  const memories = await page.evaluate(async () => {
    const res = await fetch("/api/v1/memories");
    const data = await res.json();
    return (data.data || [])[0]?.id || "";
  });
  if (memories) {
    await inspect(`/memories/${memories}`);
    out.groups.memory = await page.evaluate(() =>
      Array.from(document.querySelectorAll("aside a[href], .ds-nav a[href]")).map((a) => a.getAttribute("href")),
    );
  }
  out.groups.homeHeading = home.heading;

  const targets = Array.from(
    new Set([
      ...out.groups.header.links,
      ...out.groups.userSetting,
      ...out.groups.dataset,
      ...out.groups.memory,
      "/files",
    ]),
  );
  for (const path of targets) {
    if (out.pages.some((entry) => entry.path === path)) continue;
    await inspect(path);
  }
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
