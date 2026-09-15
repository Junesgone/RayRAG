// Exercise the provider-modal "List models" picker (upstream ToggleList):
// open the panel, check the All-models sentinel, search + no-match copy, row
// selection, the per-row edit pencil and the pinned Add-custom-model footer.
// The provider model endpoint is stubbed so the UI contract is deterministic.
//
// Usage: UI_DUMP_PASSWORD=... [RAYRAG_PROVIDER=Ollama] node scripts/ui-traversal/provider-model-picker-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";
const PROVIDER = process.env.RAYRAG_PROVIDER || "Ollama";

const MODELS = Array.from({ length: 12 }, (_, index) => ({
  name: `model-${String(index + 1).padStart(2, "0")}`,
  model_types: index === 1 ? ["embedding", "rerank"] : ["chat"],
  max_tokens: 8192,
  features: index === 0 ? ["is_tools"] : [],
}));

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1100 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));

const out = { provider: PROVIDER, steps: [] };
const snap = async (label) => {
  const state = await page.evaluate(() => {
    const panel = document.getElementById("piModelPanel");
    const search = document.getElementById("piModelSearch");
    const rows = Array.from(document.querySelectorAll("#piModelList .provider-model-row"));
    const dialog = document.getElementById("providerCustomModelModal");
    return {
      panelOpen: panel ? !panel.hidden : null,
      triggerExpanded: document.getElementById("piListModels")?.getAttribute("aria-expanded"),
      searchHidden: search ? search.hidden : null,
      searchClearHidden: document.getElementById("piModelSearchClear")?.hidden,
      rowCount: rows.length,
      firstRow: rows[0] ? (rows[0].textContent || "").replace(/\s+/g, " ").trim() : "",
      chips: Array.from(document.querySelectorAll("#piModelList .provider-model-type")).slice(0, 4).map((c) => c.textContent),
      checked: Array.from(document.querySelectorAll("#piModelList input[type=checkbox]:checked")).length,
      allChecked: (() => { const box = document.querySelector("#piModelList .provider-model-all input"); return box ? box.checked : null; })(),
      noMatch: !!document.querySelector("#piModelList .provider-model-nomatch"),
      emptyText: (document.getElementById("piModelLoading")?.textContent || "").trim(),
      dialogOpen: dialog ? dialog.classList.contains("show") : null,
      dialogTitle: (document.getElementById("providerCustomModelTitle")?.textContent || "").trim(),
      dialogMode: dialog?.dataset.mode || "",
      nameValue: document.getElementById("pcmName")?.value,
      nameDisabled: document.getElementById("pcmName")?.disabled,
      checkedTypes: Array.from(document.querySelectorAll("#pcmTypes input:checked")).map((i) => i.value),
      toolsChecked: document.getElementById("pcmTools")?.checked,
      dialogError: (document.getElementById("pcmError")?.textContent || "").trim(),
    };
  });
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-picker-${out.steps.length}-${label}.png` });
  return state;
};

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

  // Deterministic provider catalog for the picker.
  await page.route("**/api/v1/providers/*/models*", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, message: "ok", data: MODELS }),
    }),
  );

  await page.goto(`${ORIGIN}/user-setting/model`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2000);
  const card = page.locator(`[data-testid='available-model-card'][data-provider='${PROVIDER}']`).first();
  if (!(await card.count())) throw new Error(`provider card ${PROVIDER} not found`);
  await card.scrollIntoViewIfNeeded();
  await card.locator(".available-provider-add").first().click({ timeout: 8000 });
  await page.waitForTimeout(700);
  await snap("modal");

  await page.locator("#piListModels").click({ timeout: 8000 });
  await page.waitForTimeout(900);
  await snap("opened");

  // Row selection: click the row body (the pencil is hover-only, exactly like
  // upstream's `hidden group-hover:flex` button).
  await page.locator("#piModelList .provider-model-row").nth(1).locator("strong").click({ timeout: 8000 });
  await page.waitForTimeout(250);
  await snap("row-checked");

  // "All models" sentinel toggles every row.
  await page.locator("#piModelList .provider-model-all strong").click({ timeout: 8000 });
  await page.waitForTimeout(250);
  await snap("all-checked");

  // Search + no-match copy, then clear through the × button.
  await page.locator("#piModelSearch").fill("zzz-no-such-model");
  await page.waitForTimeout(300);
  await snap("no-match");
  await page.locator("#piModelSearchClear").click({ timeout: 8000 });
  await page.waitForTimeout(300);
  await snap("search-cleared");

  // Per-row edit pencil opens the dialog pre-filled and name-locked.
  await page.locator("#piModelList .provider-model-row").nth(1).hover();
  await page.waitForTimeout(250);
  await page.locator("#piModelList .provider-model-row").nth(1).locator(".provider-model-edit").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("edit-dialog");
  await page.locator("#pcmTools").click({ timeout: 8000 });
  await page.locator("#pcmSubmit").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  const afterEdit = await snap("edited");

  // Pinned footer opens the Add dialog; validation guards the empty form first.
  await page.locator(".provider-model-add").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("add-dialog");
  await page.locator("#pcmSubmit").click({ timeout: 8000 });
  await page.waitForTimeout(250);
  await snap("add-validation");
  await page.locator("#pcmName").fill("custom-model-x");
  await page.locator("#pcmTypes input[value='chat']").check({ timeout: 8000 });
  await page.locator("#pcmMaxTokens").fill("2048");
  await page.locator("#pcmSubmit").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  const afterAdd = await snap("added");
  out.editedName = afterEdit.firstRow;
  out.addedRowCount = afterAdd.rowCount;
  out.customPresent = await page.evaluate(
    () => !!Array.from(document.querySelectorAll("#piModelList .provider-model-row")).find((r) => r.dataset.name === "custom-model-x"),
  );
  out.customSelected = await page.evaluate(
    () => !!Array.from(document.querySelectorAll("#piModelList .provider-model-row")).find((r) => r.dataset.name === "custom-model-x" && r.querySelector("input[type=checkbox]").checked),
  );
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
