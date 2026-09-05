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
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = {};
try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(`${ORIGIN}/login`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.evaluate(
    async ({ password }) => {
      const r = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const j = await r.json();
      localStorage.setItem("rayrag_token", j.data.access_token);
      document.cookie =
        "rayrag_token=" + j.data.access_token + ";path=/;max-age=86400";
    },
    { password: PASSWORD },
  );
  await page.goto(`${ORIGIN}/user-setting/model`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1200);
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  const cards = page.locator(
    "[data-testid='available-model-card'][data-model-picker='true']",
  );
  const count = await cards.count();
  results.pickerCards = count;
  let openedOk = false;
  let picker = null;
  for (let i = 0; i < Math.min(count, 6); i++) {
    await cards.nth(i).click({ timeout: 8000 });
    await page.waitForTimeout(600);
    if (
      !(await page.evaluate(() =>
        document
          .getElementById("providerInstanceModal")
          .classList.contains("show"),
      ))
    )
      continue;
    const provider = await page.evaluate(() =>
      providerInstanceCard ? providerInstanceCard.dataset.provider : null,
    );
    await page
      .locator("button[onclick='toggleProviderModelPicker()']")
      .click({ timeout: 8000 });
    await page.waitForTimeout(1500);
    picker = await page.evaluate((prov) => {
      const list = document.getElementById("piModelList");
      const rows = Array.from(list.querySelectorAll(".provider-model-row"));
      return {
        provider: prov,
        hidden: document.getElementById("piModelPanel").hidden,
        rows: rows.length,
        listRole: list.getAttribute("role"),
        rowRoles: rows.slice(0, 3).map((r) => r.getAttribute("role")),
        loading: document.getElementById("piModelLoading").textContent,
      };
    }, provider);
    if (picker.rows > 0) {
      openedOk = true;
      await page.screenshot({
        path: "docs/ui-traversal/rayrag-audit/provider-modal-keyboard-20260904.png",
      });
      break;
    }
    await page.evaluate(() => closeProviderInstanceModal());
    await page.waitForTimeout(400);
  }
  results.modalOpened = openedOk;
  results.picker = picker;
  if (
    openedOk &&
    picker.listRole === "list" &&
    picker.rowRoles.every((r) => r === "listitem")
  ) {
    await page
      .locator("#piModelSearch")
      .click({ timeout: 5000 })
      .catch(() => {});
    await page.keyboard.press("Tab");
    await page.waitForTimeout(250);
    results.firstFocus = await page.evaluate(
      () =>
        document.activeElement.getAttribute("aria-label") ||
        document.activeElement.tagName,
    );
    await page.keyboard.press(" ");
    await page.waitForTimeout(300);
    results.afterSpace = await page.evaluate(() => {
      const cb = document.querySelector(
        ".provider-model-row input[type=checkbox]",
      );
      return {
        checked: cb ? cb.checked : null,
        size:
          (typeof piSelectedModels !== "undefined" && piSelectedModels.size) ||
          null,
      };
    });
    await page.locator(".provider-model-add").focus();
    await page.keyboard.press("Enter");
    await page.waitForTimeout(500);
    results.dialogOpened = await page.evaluate(() =>
      document
        .getElementById("providerCustomModelModal")
        .classList.contains("show"),
    );
    await page.screenshot({
      path: "docs/ui-traversal/rayrag-audit/provider-custom-dialog-20260904.png",
    });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(400);
    results.dialogClosedByEscape = await page.evaluate(
      () =>
        !document
          .getElementById("providerCustomModelModal")
          .classList.contains("show"),
    );
    results.result =
      results.modalOpened &&
      results.afterSpace.checked === true &&
      results.dialogOpened &&
      results.dialogClosedByEscape
        ? "PASS"
        : "FAIL";
  } else {
    results.result = "FAIL";
  }
} catch (e) {
  results.error = String(e && e.message ? e.message : e);
  results.result = "FAIL";
} finally {
  try {
    await page.evaluate(() => {
      for (const m of document.querySelectorAll(".modal-overlay.show")) {
        m.classList.remove("show");
      }
    });
  } catch {}
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await browser.close();
process.exit(results.result && results.result.startsWith("PASS") ? 0 : 1);
