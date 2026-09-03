import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error(
    "RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)",
  );
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = { steps: [], errors: [] };
try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  // Dedicated probe KB (existing KBs are never touched; deleted at the end).
  const kb = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: "CDP-EMBD-" + Date.now(),
        description: "cdp embedding tree probe",
      }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.id) {
      return { ok: false, body: data };
    }
    return { ok: true, id: data.data.id, name: data.data.name };
  });
  results.kb = kb;
  if (!kb.ok) throw new Error("probe kb create failed");

  await page.goto(ORIGIN + "/kbs/" + kb.id, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1200);
  await page.locator("a#nav1").click({ timeout: 8000 });
  await page.waitForSelector("[data-testid='default-model-embd_id']", {
    timeout: 5000,
  });
  const initial = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-embd_id']");
    return {
      state: t.getAttribute("data-state"),
      value: t.dataset.value || "",
    };
  });
  results.initial = initial;
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/kb-embd-before-zh-20260902.png",
  });

  // Simulated user click: open the default-model tree.
  await page
    .locator("[data-testid='default-model-embd_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(700);
  const opened = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-embd_id']");
    const pop = document.getElementById("embdSelPopover");
    const rows = Array.from(
      document.querySelectorAll("#embdSelTree .default-model-node"),
    );
    return {
      expanded: t.getAttribute("aria-expanded"),
      popoverHidden: pop ? pop.hidden : null,
      rowCount: rows.length,
      firstRows: rows.slice(0, 6).map((n) => n.textContent),
    };
  });
  results.opened = opened;
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/kb-embd-open-zh-20260902.png",
  });

  let leafPicked = false;
  if (opened.rowCount > 0) {
    const leaf = page.locator("#embdSelTree .default-model-leaf").first();
    if (await leaf.count()) {
      results.leaf = await leaf.textContent();
      await leaf.click();
      await page.waitForTimeout(500);
      const selected = await page.evaluate(() => {
        const t = document.querySelector(
          "[data-testid='default-model-embd_id']",
        );
        return {
          value: t.dataset.value || "",
          display: document.getElementById("embdSelValue").textContent,
          state: t.getAttribute("data-state"),
        };
      });
      results.selected = selected;
      leafPicked = selected.value !== "";
      await page.screenshot({
        path: "docs/ui-traversal/rayrag-audit/kb-embd-selected-zh-20260902.png",
      });
      if (leafPicked) {
        await page
          .locator("button[onclick*='saveConfig']")
          .first()
          .click({ timeout: 8000 });
        await page.waitForTimeout(1800);
        const saved = await page.evaluate(() => {
          const m = document.getElementById("cfgMsg");
          return m ? m.textContent : null;
        });
        results.saved = saved;
        await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
        await page.waitForTimeout(1500);
        await page.locator("a#nav1").click({ timeout: 8000 });
        await page.waitForTimeout(700);
        const after = await page.evaluate(() => {
          const t = document.querySelector(
            "[data-testid='default-model-embd_id']",
          );
          return {
            value: t.dataset.value || "",
            saved: t.dataset.savedValue || "",
            display: document.getElementById("embdSelValue").textContent,
          };
        });
        results.after = after;
        await page.screenshot({
          path: "docs/ui-traversal/rayrag-audit/kb-embd-after-zh-20260902.png",
        });
        results.persisted =
          results.after.value === results.selected.value &&
          results.after.saved === results.after.value;
      }
    }
  } else {
    results.catalogEmpty = true;
  }

  const ok =
    opened.expanded === "true" &&
    opened.popoverHidden === false &&
    (results.catalogEmpty || (leafPicked && results.persisted));
  results.result = ok
    ? results.catalogEmpty
      ? "PASS_EMPTY_CATALOG"
      : "PASS"
    : "FAIL";
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  results.result = "FAIL";
} finally {
  try {
    if (results.kb && results.kb.id) {
      await page.evaluate(async (id) => {
        await fetch("/api/v1/datasets/" + id, { method: "DELETE" });
      }, results.kb.id);
      results.cleaned = true;
    }
  } catch (e) {
    results.cleaned = "err:" + String(e && e.message ? e.message : e);
  }
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await b.close();
process.exit(results.result && results.result.startsWith("PASS") ? 0 : 1);
