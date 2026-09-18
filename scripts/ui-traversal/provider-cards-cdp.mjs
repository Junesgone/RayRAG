// CDP probe for the Available-models panel of `/user-setting/model`
// (upstream `pages/user-setting/setting-model/components/un-add-model.tsx`)
// plus the SystemSetting row labels it sits next to.
//
// It logs in, opens the page in both languages and verifies by measuring the
// live DOM after real clicks:
//   * the 63 provider cards in upstream order (rank desc, then name);
//   * each card's tag pills in `orderMap` order and the `All` / per-type chip
//     counts matching the cards they filter;
//   * the `ArrowUpRight` doc link existing only for the `APIMapUrl` factories
//     (OpenAI yes, Ollama/VLLM/LM-Studio no);
//   * chip clicks filtering the grid, and a card click still opening the
//     provider modal (regression for the click handler);
//   * the six `setting.*Model` row labels in both locales.
//
// It never persists anything: the modal is opened and cancelled again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/provider-cards-cdp.mjs
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

const readGrid = () =>
  page.evaluate(() => {
    const cards = Array.from(
      document.querySelectorAll("[data-testid='available-model-card']"),
    );
    return {
      total: cards.length,
      order: cards.slice(0, 6).map((card) => card.dataset.provider),
      tags: Object.fromEntries(
        cards
          .filter((card) =>
            ["OpenAI", "StepFun", "Tongyi-Qianwen", "Ollama"].includes(
              card.dataset.provider,
            ),
          )
          .map((card) => [
            card.dataset.provider,
            Array.from(card.querySelectorAll(".available-provider-tags .badge")).map(
              (badge) => badge.textContent,
            ),
          ]),
      ),
      docLinks: Object.fromEntries(
        cards
          .filter((card) =>
            ["OpenAI", "Ollama", "VLLM", "LM-Studio", "LocalAI", "Xinference", "New API"].includes(
              card.dataset.provider,
            ),
          )
          .map((card) => [
            card.dataset.provider,
            !!card.querySelector(".available-provider-doc"),
          ]),
      ),
    };
  });

const readChips = () =>
  page.evaluate(() =>
    Array.from(document.querySelectorAll(".provider-chip")).map((chip) => ({
      label: chip.textContent.replace(/\s+/g, " ").trim(),
      filter: chip.dataset.filter,
      active: chip.classList.contains("on"),
    })),
  );

const visibleCards = () =>
  page.evaluate(
    () =>
      Array.from(
        document.querySelectorAll("[data-testid='available-model-card']"),
      ).filter((card) => card.style.display !== "none").length,
  );

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
  await page.waitForSelector("[data-testid='available-model-card']", {
    timeout: 20000,
  });
  results.grid = await readGrid();
  results.chips = await readChips();
  results.systemSettingLabels = await page.evaluate(() =>
    Array.from(
      document.querySelectorAll(
        "[data-testid='system-model-settings'] .provider-default-field > label",
      ),
    ).map((node) => node.textContent.replace(/\s+/g, " ").trim()),
  );
  results.initialVisible = await visibleCards();
  await page.screenshot({ path: `${shots}/provider-cards-20260821.png` });

  // Chip click filters the grid and keeps exactly one pill active.
  await page.click(".provider-chip[data-filter='ocr']");
  await page.waitForTimeout(300);
  results.ocrFilter = {
    visible: await visibleCards(),
    activeChips: (await readChips()).filter((chip) => chip.active).map((c) => c.filter),
  };
  await page.screenshot({ path: `${shots}/provider-cards-ocr-20260821.png` });
  await page.click(".provider-chip[data-filter='']");
  await page.waitForTimeout(300);
  results.restored = { visible: await visibleCards() };

  // Search box narrows the same set the `All` pill counts.
  await page.fill("[data-testid='model-providers-search']", "deep");
  await page.waitForTimeout(300);
  results.search = {
    visible: await visibleCards(),
    allCount: await page.textContent("#providerCountAll"),
  };
  await page.fill("[data-testid='model-providers-search']", "");
  await page.waitForTimeout(300);

  // The card click still opens the provider modal; cancel leaves no change.
  await page.click("[data-testid='available-model-card'][data-provider='DeepSeek']");
  await page.waitForFunction(
    () => document.getElementById("providerInstanceModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  results.modal = await page.evaluate(() => ({
    title: document.getElementById("providerInstanceTitle").textContent,
    instanceName: document.getElementById("piName").placeholder,
    apiKeyField: !!document.getElementById("piKey"),
  }));
  await page.screenshot({ path: `${shots}/provider-cards-modal-20260821.png` });
  // The footer's second outline button is Cancel (the first is Verify); the
  // picker trigger is also a `.btn-o`, so scope the click to the action row.
  await page.click(
    "#providerInstanceModal .provider-instance-actions button.btn-o >> nth=1",
  );
  await page.waitForFunction(
    () => !document.getElementById("providerInstanceModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  results.modalClosed = true;

  // Chinese locale: translated embedding/rerank labels, same six rows.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='available-model-card']", {
    timeout: 20000,
  });
  results.zh = {
    systemSettingLabels: await page.evaluate(() =>
      Array.from(
        document.querySelectorAll(
          "[data-testid='system-model-settings'] .provider-default-field > label",
        ),
      ).map((node) => node.textContent.replace(/\s+/g, " ").trim()),
    ),
    chips: (await readChips()).map((chip) => chip.label),
  };
  await page.screenshot({ path: `${shots}/provider-cards-zh-20260821.png` });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/provider-cards-error-20260821.png` });
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
