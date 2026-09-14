// RAGFlow UI inventory dump: visits each route in the already-authenticated
// CDP Chrome profile and records every interactive control (buttons, links,
// inputs, selects, switches, tabs, table headers, dropdown triggers) plus a
// screenshot. Output is the parity reference for the RayRAG Rust UI.
//
// Usage:
//   node scripts/ui-traversal/ragflow-ui-dump.mjs [route ...]
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 RAYRAG_CDP_ENDPOINT=http://127.0.0.1:16002
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const OUT_DIR =
  process.env.RAGFLOW_UI_DUMP_DIR || "docs/ui-traversal/ragflow-pages";

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
const index = [];
for (const route of routes) {
  const record = { route };
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
  index.push({
    route,
    finalUrl: record.url,
    title: record.title,
    counts: {
      buttons: (record.buttons || []).length,
      links: (record.links || []).length,
      fields: (record.fields || []).length,
    },
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
