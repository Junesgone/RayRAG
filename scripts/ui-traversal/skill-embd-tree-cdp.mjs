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
let original = null;
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
      if (data.code !== 0 || !data.data || !data.data.access_token)
        return { ok: false, body: data };
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  // Capture the original config so the probe restores it at cleanup.
  original = await page.evaluate(async () => {
    const res = await fetch("/api/v1/skills/config?space_id=default");
    const data = await res.json();
    return data.data || {};
  });
  results.originalEmbd = original.embd_id || "";

  await page.goto(ORIGIN + "/skills", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1200);
  await page
    .locator("button[onclick*='openSkillConfig']")
    .first()
    .click({ timeout: 8000 });
  await page.waitForSelector("[data-testid='default-model-embd_id']", {
    timeout: 5000,
  });
  const initial = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-embd_id']");
    return {
      state: t.getAttribute("data-state"),
      value: t.dataset.value || "",
      modalShown: document
        .getElementById("skillConfigModal")
        .classList.contains("show"),
    };
  });
  results.initial = initial;
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/skill-embd-before-zh-20260903.png",
  });

  await page
    .locator("[data-testid='default-model-embd_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(700);
  const opened = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-embd_id']");
    const pop = document.getElementById("skillEmbdPopover");
    const rows = Array.from(
      document.querySelectorAll("#skillEmbdTree .default-model-node"),
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
    path: "docs/ui-traversal/rayrag-audit/skill-embd-open-zh-20260903.png",
  });

  let leafPicked = false;
  if (opened.rowCount > 0) {
    const leaf = page.locator("#skillEmbdTree .default-model-leaf").first();
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
          display: document.getElementById("skillEmbdValue").textContent,
          state: t.getAttribute("data-state"),
        };
      });
      results.selected = selected;
      leafPicked = selected.value !== "";
      await page.screenshot({
        path: "docs/ui-traversal/rayrag-audit/skill-embd-selected-zh-20260903.png",
      });
      if (leafPicked) {
        await page
          .locator("button[onclick*='saveSkillConfig']")
          .first()
          .click({ timeout: 8000 });
        await page.waitForTimeout(1500);
        const savedState = await page.evaluate(() => ({
          modalShown: document
            .getElementById("skillConfigModal")
            .classList.contains("show"),
        }));
        results.savedModalClosed = !savedState.modalShown;
        await page
          .locator("button[onclick*='openSkillConfig']")
          .first()
          .click({ timeout: 8000 });
        await page.waitForTimeout(900);
        const after = await page.evaluate(() => {
          const t = document.querySelector(
            "[data-testid='default-model-embd_id']",
          );
          return {
            value: t.dataset.value || "",
            saved: t.dataset.savedValue || "",
            display: document.getElementById("skillEmbdValue").textContent,
          };
        });
        results.after = after;
        await page.screenshot({
          path: "docs/ui-traversal/rayrag-audit/skill-embd-after-zh-20260903.png",
        });
        results.persisted =
          results.after.value === results.selected.value &&
          results.after.saved === results.after.value;
      }
    }
  }

  const ok =
    opened.expanded === "true" &&
    opened.popoverHidden === false &&
    leafPicked &&
    results.savedModalClosed === true &&
    results.persisted === true;
  results.result = ok ? "PASS" : "FAIL";
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  results.result = "FAIL";
} finally {
  try {
    if (original && typeof original.embd_id !== "undefined") {
      await page.evaluate(async (cfg) => {
        const body = Object.assign({ space_id: "default" }, cfg);
        await fetch("/api/v1/skills/config", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
      }, original);
      results.restored = true;
    }
  } catch (e) {
    results.restored = "err:" + String(e && e.message ? e.message : e);
  }
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await b.close();
process.exit(results.result === "PASS" ? 0 : 1);
