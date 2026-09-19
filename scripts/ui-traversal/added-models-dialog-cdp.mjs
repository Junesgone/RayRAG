// CDP probe for the Added-models panel (`used-model.tsx`) and the shared
// `ConfirmDeleteDialog` shell.
//
// Real clicks in the fnOS Chrome: it logs in, opens `/user-setting/model`,
// expands the first added instance (real click on the "View models" toggle),
// measures the upstream geometry (`LlmIcon` 32px glyph without RayRAG's letter
// tile, `bg-bg-card` list, `.5px` row separators, hover-only pencil) and then
// opens the delete confirmation on that row, checks the upstream
// `ConfirmDeleteDialog` copy/test ids, closes it with the lucide `X` and
// finally cancels — nothing is deleted.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/added-models-dialog-cdp.mjs
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
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1600, height: 1000 });
const results = { errors: [], pageErrors };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));

  await page.goto(`${ORIGIN}/user-setting/model`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='added-model-card']", {
    timeout: 20000,
  });

  // 1. Provider header icon: 32px glyph, no letter tile behind it.
  results.providerIcon = await page.evaluate(() => {
    const card = document.querySelector("[data-testid='added-model-card']");
    const icon = card.querySelector(".added-model-provider-icon");
    const glyph = icon.querySelector(".provider-brand-icon");
    const style = getComputedStyle(icon);
    return {
      provider: card.dataset.provider,
      iconBox: [style.width, style.height],
      tileBackground: style.backgroundColor,
      tileRadius: style.borderRadius,
      glyphTag: glyph ? glyph.tagName : null,
      glyphBox: glyph
        ? [getComputedStyle(glyph).width, getComputedStyle(glyph).height]
        : null,
      nameSize: getComputedStyle(
        card.querySelector(".added-model-provider-name"),
      ).fontSize,
    };
  });
  await page.screenshot({ path: `${shots}/added-models-20260821.png` });

  // 2. Expand the first instance with a real click.
  await page.click(".added-model-instance .added-model-toggle");
  await page.waitForFunction(
    () =>
      document.querySelectorAll(".added-model-row").length > 0 ||
      document.querySelector(".added-model-error") !== null,
    null,
    { timeout: 20000 },
  );
  results.expanded = await page.evaluate(() => {
    const content = document.querySelector(".added-model-content");
    const list = document.querySelector(".added-model-rows");
    const row = document.querySelector(".added-model-row");
    const pencil = row ? row.querySelector(".added-model-edit") : null;
    const toggle = document.querySelector(".added-model-instance .added-model-toggle");
    return {
      hidden: content.hidden,
      toggleLabel: toggle.textContent.trim(),
      toggleExpanded: toggle.getAttribute("aria-expanded"),
      listBackground: list ? getComputedStyle(list).backgroundColor : null,
      listMaxHeight: list ? getComputedStyle(list).maxHeight : null,
      rowPadding: row ? getComputedStyle(row).padding : null,
      rowBorder: row ? getComputedStyle(row).borderBottomWidth : null,
      rowCount: document.querySelectorAll(".added-model-row").length,
      typeChips: Array.from(
        document.querySelectorAll(".added-model-type-summary .added-model-type"),
      ).map((chip) => chip.textContent),
      pencilHidden: pencil ? getComputedStyle(pencil).opacity : null,
      firstModel: row
        ? row.querySelector(".added-model-name").textContent
        : null,
      switchRole: row
        ? [row.querySelector(".added-model-switch").getAttribute("role"),
           row.querySelector(".added-model-switch").getAttribute("aria-checked")]
        : null,
    };
  });
  // The pencil only shows on hover, like upstream `hidden group-hover:flex`.
  await page.hover(".added-model-row");
  await page.waitForTimeout(250);
  results.pencilOnHover = await page.evaluate(
    () => getComputedStyle(document.querySelector(".added-model-edit")).opacity,
  );
  await page.screenshot({ path: `${shots}/added-models-expanded-20260821.png` });

  // 3. The delete affordance opens the upstream ConfirmDeleteDialog shell.
  await page.click(".added-model-instance .added-model-delete");
  await page.waitForSelector("#deleteModelInstanceModal.show", {
    timeout: 10000,
  });
  results.dialog = await page.evaluate(() => {
    const dialog = document.querySelector("[data-testid='confirm-delete-dialog']");
    const close = document.querySelector(
      "[data-testid='confirm-delete-dialog-close-btn']",
    );
    const cancel = document.querySelector(
      "[data-testid='confirm-delete-dialog-cancel-btn']",
    );
    const confirm = document.querySelector(
      "[data-testid='confirm-delete-dialog-confirm-btn']",
    );
    return {
      title: document.getElementById("deleteModelInstanceTitle").textContent,
      body: document.getElementById("deleteModelInstanceText").textContent,
      role: dialog.getAttribute("role"),
      closeGlyph: close.innerHTML.includes("svg"),
      closeSize: getComputedStyle(close.querySelector("svg")).width,
      cancelLabel: cancel.textContent,
      confirmLabel: confirm.textContent,
      confirmDanger: confirm.className,
    };
  });
  await page.screenshot({ path: `${shots}/added-models-delete-dialog-20260821.png` });

  // 4. The lucide X closes it again (no deletion happened).
  await page.click("[data-testid='confirm-delete-dialog-close-btn']");
  await page.waitForFunction(
    () =>
      !document
        .getElementById("deleteModelInstanceModal")
        .classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  results.closedByX = true;

  // 5. Chinese copy for the same dialog.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='added-model-card']", {
    timeout: 20000,
  });
  await page.click(".added-model-instance .added-model-delete");
  await page.waitForSelector("#deleteModelInstanceModal.show", {
    timeout: 10000,
  });
  results.zh = await page.evaluate(() => ({
    title: document.getElementById("deleteModelInstanceTitle").textContent,
    cancel: document.querySelector(
      "[data-testid='confirm-delete-dialog-cancel-btn']",
    ).textContent,
    confirm: document.querySelector(
      "[data-testid='confirm-delete-dialog-confirm-btn']",
    ).textContent,
    toggle: document.querySelector(".added-model-toggle").textContent.trim(),
  }));
  await page.screenshot({ path: `${shots}/added-models-delete-zh-20260821.png` });
  await page.click("[data-testid='confirm-delete-dialog-cancel-btn']");
  await page.waitForFunction(
    () =>
      !document
        .getElementById("deleteModelInstanceModal")
        .classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  results.cancelled = true;
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/added-models-error-20260821.png` });
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
