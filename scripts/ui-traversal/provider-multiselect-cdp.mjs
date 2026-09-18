// CDP probe for the shared model-type MultiSelect
// (`web/src/components/ui/multi-select.tsx` as consumed by
// `add-custom-model-dialog.tsx` and `used-model.tsx`).
//
// It drives a real Chrome through the DevTools Protocol: logs in, opens
// `/user-setting/model`, clicks an Available provider card, expands the model
// picker, opens the "Add custom model" dialog and then exercises the Model type
// control the way a user would:
//   * the `(Select all)` row appears above the options and toggles every type
//     (badges render because upstream passes `maxCount={100}`);
//   * typing in the search box filters by cmdk's `command-score` and re-sorts
//     the remaining rows by score;
//   * a query with no match renders upstream's `common.noDataFound` copy;
//   * the required-name message is upstream's composed `"<label> is required"`;
//   * the same dialog in Chinese shows `全选` / `没有找到数据。`.
//
// Nothing is persisted: the provider modal is cancelled at the end, so no
// instance or model is written to the tenant.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/provider-multiselect-cdp.mjs
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

const readOptions = () =>
  page.evaluate(() =>
    Array.from(document.querySelectorAll("#pcmTypesOptions .pi-multiselect-option")).map(
      (row) => ({
        value: row.dataset.value,
        label: row.querySelector(".pi-multiselect-option-label").textContent,
        selected: row.getAttribute("aria-selected"),
        checked: !!row.querySelector(".pi-multiselect-check.on"),
      }),
    ),
  );

const readBadges = () =>
  page.evaluate(() =>
    Array.from(
      document.querySelectorAll("#pcmTypesValues .pi-multiselect-badge-label"),
    ).map((node) => node.textContent),
  );

const openDialog = async () => {
  // Ollama is one of the picker factories upstream renders `AddableToggleList`
  // for, so its modal carries the model list plus the "Add custom model" row.
  await page.click(
    "[data-testid='available-model-card'][data-provider='Ollama']",
  );
  await page.waitForFunction(
    () => document.getElementById("providerInstanceModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  await page.waitForFunction(
    () => !document.getElementById("piModelPicker").hidden,
    null,
    { timeout: 15000 },
  );
  await page.click("#piListModels");
  await page.waitForTimeout(300);
  await page.waitForSelector("[data-testid='pi-add-custom-model']", {
    state: "visible",
    timeout: 10000,
  });
  await page.click("[data-testid='pi-add-custom-model']");
  await page.waitForFunction(
    () =>
      document
        .getElementById("providerCustomModelModal")
        .classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  await page.click("[data-testid='pcm-types-trigger']");
  await page.waitForTimeout(200);
};

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
  await openDialog();

  // 1. Initial state: `(Select all)` row first, then the seven type rows.
  results.initial = {
    options: await readOptions(),
    placeholder: await page.textContent("#pcmTypesPlaceholder"),
  };

  // 2. The select-all row toggles every type.
  await page.click("#pcmTypesOptions .pi-multiselect-all");
  await page.waitForTimeout(200);
  results.afterSelectAll = {
    badges: await readBadges(),
    options: await readOptions(),
  };
  await page.screenshot({ path: `${shots}/provider-multiselect-all-20260821.png` });

  // 3. Clicking it again clears the selection.
  await page.click("#pcmTypesOptions .pi-multiselect-all");
  await page.waitForTimeout(200);
  results.afterClearAll = { badges: await readBadges() };

  // 4. cmdk scoring: "e" ranks the labels containing an early "e".
  await page.fill("#pcmTypesSearch", "em");
  await page.waitForTimeout(200);
  results.scored = (await readOptions())
    .filter((row) => row.value !== "__all__")
    .map((row) => row.label);
  // A fuzzy subsequence ("rnk") still matches Rerank through command-score.
  await page.fill("#pcmTypesSearch", "rnk");
  await page.waitForTimeout(200);
  results.fuzzy = (await readOptions())
    .filter((row) => row.value !== "__all__")
    .map((row) => row.label);
  await page.fill("#pcmTypesSearch", "zzzz");
  await page.waitForTimeout(200);
  results.empty = await page.textContent("#pcmTypesOptions .pi-multiselect-empty");
  await page.screenshot({ path: `${shots}/provider-multiselect-empty-20260821.png` });

  // 5. Pick two types by clicking rows, then check the required-name message.
  await page.fill("#pcmTypesSearch", "");
  await page.waitForTimeout(150);
  await page.click(
    "#pcmTypesOptions .pi-multiselect-option[data-value='ocr']",
  );
  await page.click(
    "#pcmTypesOptions .pi-multiselect-option[data-value='chat']",
  );
  await page.waitForTimeout(150);
  results.picked = await readBadges();
  // Close the popover before submitting: it is anchored above the dialog
  // footer. Clicking the trigger toggles it shut; Escape would dismiss the
  // whole dialog (upstream's Dialog listens for it).
  await page.click("[data-testid='pcm-types-trigger']");
  await page.waitForFunction(
    () => document.getElementById("pcmTypesPopover").hidden,
    null,
    { timeout: 10000 },
  );
  await page.click("#pcmSubmit");
  await page.waitForTimeout(200);
  results.requiredMessage = await page.textContent("#pcmError");
  // Filling the name submits the dialog and appends the model to the picker's
  // local catalog (upstream only persists it when the instance is saved).
  await page.fill("#pcmName", `cdp-probe-model-${Date.now()}`);
  await page.click("#pcmSubmit");
  await page.waitForTimeout(400);
  results.addedRow = await page.evaluate(() => {
    const rows = Array.from(
      document.querySelectorAll("#piModelList .provider-model-row"),
    );
    const row = rows[rows.length - 1];
    return row
      ? {
          label: row.textContent.replace(/\s+/g, " ").trim().slice(0, 80),
          chips: Array.from(row.querySelectorAll(".provider-model-chip")).map(
            (chip) => chip.textContent,
          ),
        }
      : null;
  });
  await page.screenshot({ path: `${shots}/provider-multiselect-required-20260821.png` });

  // 6. Chinese dialog: `全选` row and `没有找到数据。` empty copy.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='available-model-card']", {
    timeout: 20000,
  });
  await openDialog();
  results.zh = {
    allRow: await page.textContent("#pcmTypesOptions .pi-multiselect-all"),
    labels: await page.evaluate(() => {
      const labelOf = (selector) => {
        const node = document.querySelector(selector);
        return node ? node.textContent.trim() : null;
      };
      return {
        title: labelOf("#providerCustomModelTitle"),
        // The name field's caption is a bare text node inside its label.
        name: (document.querySelector("#providerCustomModelModal label") || {})
          .textContent,
        type: labelOf("#pcmTypesLabel"),
        maxTokens: labelOf("#pcmMaxTokensLabel"),
        features: labelOf("#pcmFeaturesLabel"),
        toolCall: labelOf("#pcmToolsLabel"),
        placeholder: labelOf("#pcmTypesPlaceholder"),
      };
    }),
  };
  await page.fill("#pcmTypesSearch", "zzzz");
  await page.waitForTimeout(200);
  results.zh.empty = await page.textContent(
    "#pcmTypesOptions .pi-multiselect-empty",
  );
  await page.screenshot({ path: `${shots}/provider-multiselect-zh-20260821.png` });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/provider-multiselect-error-20260821.png` });
} finally {
  // Cancel both dialogs: nothing is written to the tenant.
  await page
    .evaluate(() => {
      const custom = document.getElementById("providerCustomModelModal");
      if (custom) custom.classList.remove("show");
      const modal = document.getElementById("providerInstanceModal");
      if (modal) modal.classList.remove("show");
    })
    .catch(() => {});
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
