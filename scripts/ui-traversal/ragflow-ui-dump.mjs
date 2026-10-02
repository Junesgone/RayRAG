// UI inventory dump: visits each route in the CDP Chrome profile and records
// every interactive control (buttons, links, inputs, selects, switches, tabs,
// table headers, dropdown triggers) plus a screenshot. Run it once against
// RAGFlow (parity reference) and once against RayRAG, then diff the two
// inventories route by route.
//
// Usage:
//   node scripts/ui-traversal/ragflow-ui-dump.mjs [route ...]
//
// RAGFlow (session comes from ragflow-auth-cdp.mjs in the same Chrome profile):
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 \
//   RAGFLOW_UI_DUMP_DIR=docs/ui-traversal/ragflow-pages
//
// RayRAG (script logs in itself via UI_DUMP_AUTH=rayrag):
//   UI_DUMP_AUTH=rayrag RAGFLOW_UI_ORIGIN=http://127.0.0.1:9390 \
//   UI_DUMP_EMAIL=admin@rayrag.local UI_DUMP_PASSWORD=... \
//   RAGFLOW_UI_DUMP_DIR=docs/ui-traversal/rayrag-pages
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const OUT_DIR =
  process.env.RAGFLOW_UI_DUMP_DIR || "docs/ui-traversal/ragflow-pages";
const AUTH = process.env.UI_DUMP_AUTH || "none";
const EMAIL = process.env.UI_DUMP_EMAIL || "admin@rayrag.local";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD || "";
// RayRAG detects its UI language from the `lang=zh-CN` cookie (English
// otherwise). Pinning both dumps to one language keeps the parity diff
// label-based instead of translation-based.
const LANG = process.env.UI_DUMP_LANG || "";

const DEFAULT_ROUTES = [
  "/",
  "/datasets",
  "/searches",
  "/agents",
  "/memories",
  "/files",
  "/user-setting/data-source",
  "/user-setting/chat-channel",
  "/user-setting/model",
  "/user-setting/mcp",
  "/user-setting/team",
  "/user-setting/profile",
  "/user-setting/api",
];

const routes = process.argv.slice(2).length
  ? process.argv.slice(2)
  : DEFAULT_ROUTES;
const slug = (route) =>
  route
    .replace(/[?#].*$/, "")
    .replace(/^\/+|\/+$/g, "")
    .replace(/[^a-zA-Z0-9]+/g, "-") || "root";

// Collect every visible interactive element in one DOM pass. Kept intentionally
// generic: the parity audit diffs these lists against the RayRAG page.
const COLLECT = () => {
  const visible = (el) => {
    if (!el) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) return false;
    const style = getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  const label = (el) =>
    (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.getAttribute("placeholder") ||
      (el.textContent || "").trim() ||
      ""
    )
      .replace(/\s+/g, " ")
      .slice(0, 80);
  const sel = (el) => {
    const parts = [el.tagName.toLowerCase()];
    for (const attr of ["data-testid", "role", "type", "name", "id"]) {
      const value = el.getAttribute(attr);
      if (value) parts.push(`[${attr}="${value.slice(0, 40)}"]`);
    }
    return parts.join("");
  };
  const nodes = Array.from(
    document.querySelectorAll(
      "button, a, input, select, textarea, [role='button'], [role='switch'], [role='tab'], [role='combobox'], [role='checkbox'], [role='menuitem'], [data-testid]",
    ),
  ).filter(visible);

  const buttons = [];
  const links = [];
  const fields = [];
  const others = [];
  for (const el of nodes) {
    const entry = {
      tag: el.tagName.toLowerCase(),
      testid: el.getAttribute("data-testid") || "",
      role: el.getAttribute("role") || "",
      text: label(el),
      selector: sel(el),
      disabled:
        el.hasAttribute("disabled") ||
        el.getAttribute("aria-disabled") === "true",
      type: el.getAttribute("type") || "",
      href: el.getAttribute("href") || "",
      state:
        el.getAttribute("aria-checked") || el.getAttribute("data-state") || "",
      options:
        el.tagName === "SELECT"
          ? Array.from(el.options)
              .map((o) => o.textContent.trim())
              .slice(0, 20)
          : undefined,
      value: el.tagName === "SELECT" ? el.value : undefined,
    };
    if (
      el.tagName === "BUTTON" ||
      el.getAttribute("role") === "button" ||
      el.getAttribute("role") === "menuitem"
    ) {
      buttons.push(entry);
    } else if (el.tagName === "A") {
      links.push(entry);
    } else if (
      el.tagName === "INPUT" ||
      el.tagName === "SELECT" ||
      el.tagName === "TEXTAREA" ||
      el.getAttribute("role") === "switch" ||
      el.getAttribute("role") === "combobox" ||
      el.getAttribute("role") === "checkbox"
    ) {
      fields.push(entry);
    } else {
      others.push(entry);
    }
  }
  const headings = Array.from(document.querySelectorAll("h1,h2,h3"))
    .filter(visible)
    .map((el) => ({
      tag: el.tagName.toLowerCase(),
      text: (el.textContent || "").trim().slice(0, 100),
    }))
    .slice(0, 40);
  const tableHeaders = Array.from(document.querySelectorAll("th")).map((el) =>
    (el.textContent || "").trim().slice(0, 60),
  );
  const tabs = Array.from(document.querySelectorAll("[role='tab']")).map(
    (el) => ({
      text: (el.textContent || "").trim().slice(0, 60),
      selected: el.getAttribute("aria-selected"),
    }),
  );
  return {
    url: location.href,
    title: document.title,
    bodyText: (document.body.innerText || "")
      .replace(/\n{2,}/g, "\n")
      .slice(0, 1500),
    headings,
    buttons,
    links,
    fields,
    others,
    tableHeaders,
    tabs,
  };
};

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

await fs.mkdir(OUT_DIR, { recursive: true });

// RayRAG issues its bearer token from POST /api/v1/auth/login and keeps it in
// localStorage; the login page itself never redirects into the app.
const authBootstrap = { mode: AUTH };
if (AUTH === "rayrag") {
  if (!PASSWORD) {
    console.error("UI_DUMP_PASSWORD is required when UI_DUMP_AUTH=rayrag");
    process.exit(2);
  }
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  authBootstrap.result = await page.evaluate(
    async ({ email, password }) => {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await response.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { email: EMAIL, password: PASSWORD },
  );
  if (!authBootstrap.result.ok) {
    console.error(
      "RayRAG login failed: " + JSON.stringify(authBootstrap.result),
    );
    process.exit(1);
  }
}

const index = [];
const host = new URL(ORIGIN).hostname;
if (LANG === "en") {
  await ctx.addCookies([
    { name: "lang", value: "en-US", domain: host, path: "/" },
  ]);
} else if (LANG) {
  await ctx.addCookies([
    { name: "lang", value: LANG, domain: host, path: "/" },
  ]);
}
for (const route of routes) {
  const record = { route };
  const routeStart = pageErrors.length;
  try {
    await page.goto(ORIGIN + route, {
      waitUntil: "domcontentloaded",
      timeout: 45000,
    });
    await page.waitForTimeout(3500);
    Object.assign(record, await page.evaluate(COLLECT));
    const file = path.join(OUT_DIR, slug(route) + ".png");
    await page.screenshot({ path: file, fullPage: false });
    record.screenshot = file;
  } catch (error) {
    record.error = String(error && error.message ? error.message : error).slice(
      0,
      300,
    );
  }
  record.pageErrors = pageErrors.slice(routeStart);
  index.push({
    route,
    finalUrl: record.url,
    title: record.title,
    counts: {
      buttons: (record.buttons || []).length,
      links: (record.links || []).length,
      fields: (record.fields || []).length,
    },
    pageErrors: record.pageErrors,
    error: record.error,
  });
  await fs.writeFile(
    path.join(OUT_DIR, slug(route) + ".json"),
    JSON.stringify(record, null, 1),
  );
}

await fs.writeFile(
  path.join(OUT_DIR, "index.json"),
  JSON.stringify(
    {
      origin: ORIGIN,
      generatedAt: new Date().toISOString(),
      auth: authBootstrap,
      pages: index,
      pageErrors,
    },
    null,
    1,
  ),
);
console.log(JSON.stringify({ pages: index, pageErrors }, null, 1));
await page.close().catch(() => {});
process.exit(0);
