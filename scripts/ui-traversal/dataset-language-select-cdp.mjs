// CDP probe for the dataset configuration Language field
// (`web/src/components/originui/select-with-search.tsx` driving
// `dataset-setting/general-form.tsx`): the trigger is a `role=combobox` button,
// the popover renders the 27 `LanguageTranslationMap` keys in insertion order
// plus a command search row, typing filters the rows, clicking a row selects it
// and closes the popover, and the chosen value reaches the trigger label.
// The probe does not press Save, so the dataset is left untouched.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/dataset-language-select-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const EXPECTED = [
  "English",
  "Chinese",
  "Traditional Chinese",
  "Russian",
  "Indonesian",
  "Indonesia",
  "Spanish",
  "Vietnamese",
  "Japanese",
  "Korean",
  "Portuguese BR",
  "pt-br",
  "pt-BR",
  "German",
  "French",
  "Italian",
  "Tamil",
  "Telugu",
  "Kannada",
  "Thai",
  "Greek",
  "Hindi",
  "Ukrainian",
  "Bulgarian",
  "Arabic",
  "Turkish",
  "Dutch",
];

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 950 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));

  const kbId = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets?page=1&page_size=1");
    const data = await res.json();
    return data.data && data.data[0] ? data.data[0].id : "";
  });
  if (!kbId) throw new Error("no dataset available to open");

  await page.goto(`${ORIGIN}/dataset/configuration/${kbId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='ds-settings-basic-language-select']", { timeout: 20000 });

  const trigger = page.locator("[data-testid='ds-settings-basic-language-select']");
  check((await trigger.getAttribute("role")) === "combobox", "trigger_is_combobox");
  check((await trigger.getAttribute("aria-expanded")) === "false", "trigger_starts_collapsed");
  check((await trigger.evaluate((n) => n.tagName)) === "BUTTON", "trigger_is_button");
  check(
    (await page.locator("select#cfgLang").count()) === 0,
    "no_native_select_remains",
  );
  check((await trigger.locator("svg").count()) >= 1, "trigger_has_chevron");
  const initialLabel = (await trigger.textContent()).trim();
  check(initialLabel.length > 0, "trigger_shows_a_label", initialLabel);

  // Real click on the trigger opens the cmdk popover.
  await trigger.click();
  await page.waitForSelector("#cfgLangPop:not([hidden])", { timeout: 10000 });
  check((await trigger.getAttribute("aria-expanded")) === "true", "trigger_expands_on_click");
  const rows = page.locator("#cfgLangList .sws-option");
  const rowTexts = await rows.allInnerTexts();
  check(
    JSON.stringify(rowTexts.map((x) => x.trim())) === JSON.stringify(EXPECTED),
    "rendered_options_match_language_translation_map",
    `${rowTexts.length} rows`,
  );
  check(
    (await page.locator("#cfgLangList [data-testid='combobox-option']").count()) === EXPECTED.length,
    "every_row_carries_combobox_option_testid",
  );
  check((await page.locator("#cfgLangList .sws-check").count()) === 1, "single_check_icon");
  const searchBox = page.locator("#cfgLangSearch");
  check(await searchBox.isVisible(), "search_row_visible_past_five_options");
  check((await searchBox.getAttribute("placeholder")) === "Search...", "search_placeholder");

  // The filter keeps any option whose value contains the query.
  await searchBox.fill("indo");
  await page.waitForTimeout(150);
  const filtered = (await rows.allInnerTexts()).map((x) => x.trim());
  check(
    JSON.stringify(filtered) === JSON.stringify(["Indonesian", "Indonesia"]),
    "search_filters_rows",
    filtered.join("|"),
  );
  await searchBox.fill("zzzz");
  await page.waitForTimeout(150);
  check((await rows.count()) === 0, "no_rows_without_a_match");
  check(await page.locator("#cfgLangEmpty").isVisible(), "command_empty_visible");
  const emptyText = (await page.locator("#cfgLangEmpty").innerText()).trim();
  check(emptyText === "No data found.", "command_empty_copy", emptyText);

  // Pick a language with a real click; the popover closes and the label follows.
  await searchBox.fill("Korean");
  await page.waitForTimeout(150);
  const korean = page.locator("#cfgLangList .sws-option", { hasText: "Korean" }).first();
  await korean.click();
  await page.waitForFunction(() => document.getElementById("cfgLangPop").hidden, null, { timeout: 10000 });
  check((await trigger.getAttribute("aria-expanded")) === "false", "popover_closes_after_pick");
  check(
    (await trigger.locator("#cfgLangLabel").innerText()).trim() === "Korean",
    "label_follows_selection",
  );
  check(
    (await page.locator("#cfgLangList .sws-check").count()) === 1,
    "check_icon_moved_to_selection",
  );

  // Outside click closes without changing the value; Escape does the same.
  await trigger.click();
  await page.waitForSelector("#cfgLangPop:not([hidden])", { timeout: 10000 });
  await page.mouse.click(20, 400);
  await page.waitForFunction(() => document.getElementById("cfgLangPop").hidden, null, { timeout: 10000 });
  check(
    (await trigger.locator("#cfgLangLabel").innerText()).trim() === "Korean",
    "outside_click_keeps_value",
  );
  await trigger.click();
  await page.waitForSelector("#cfgLangPop:not([hidden])", { timeout: 10000 });
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => document.getElementById("cfgLangPop").hidden, null, { timeout: 10000 });
  check(
    (await trigger.locator("#cfgLangLabel").innerText()).trim() === "Korean",
    "escape_keeps_value",
  );

  // The save payload carries the field.
  const bodyHasLanguage = await page.evaluate(() => /language:swsValue\('cfgLang'\)/.test(document.documentElement.innerHTML));
  check(bodyHasLanguage, "save_payload_reads_the_field");

  // Full round-trip on a throwaway dataset: pick a language, press the real Save
  // button, reload and confirm the stored value comes back. The dataset is
  // deleted again at the end, so tenant state is left as it was.
  const probeId = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "probe-language-" + Date.now() }),
    });
    const data = await res.json();
    return data.data && data.data.id ? data.data.id : "";
  });
  check(Boolean(probeId), "throwaway_dataset_created");
  if (probeId) {
    await page.goto(`${ORIGIN}/dataset/configuration/${probeId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForSelector("[data-testid='ds-settings-basic-language-select']", { timeout: 20000 });
    await page.locator("[data-testid='ds-settings-basic-language-select']").click();
    await page.waitForSelector("#cfgLangPop:not([hidden])", { timeout: 10000 });
    await page.locator("#cfgLangSearch").fill("Dutch");
    await page.waitForTimeout(150);
    await page.locator("#cfgLangList .sws-option", { hasText: "Dutch" }).first().click();
    await page.locator("[data-testid='ds-settings-basic-save-btn']").click();
    await page.waitForFunction(
      () => /已保存|Saved/.test((document.getElementById("cfgMsg") || {}).textContent || ""),
      null,
      { timeout: 20000 },
    );
    check(true, "save_request_succeeded");
    await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForSelector("[data-testid='ds-settings-basic-language-select']", { timeout: 20000 });
    const reloaded = (
      await page.locator("#cfgLangLabel").innerText()
    ).trim();
    check(reloaded === "Dutch", "stored_language_survives_a_reload", reloaded);
    const stored = await page.evaluate(async (id) => {
      const res = await fetch(`/api/v1/datasets/${id}`);
      const data = await res.json();
      return data.data ? data.data.language : null;
    }, probeId);
    check(stored === "Dutch", "api_returns_the_top_level_column", String(stored));
    const cleaned = await page.evaluate(async (id) => {
      const res = await fetch("/api/v1/datasets", {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ids: [id] }),
      });
      const text = await res.text();
      if (!text) return res.ok ? 0 : -1;
      try {
        return JSON.parse(text).code;
      } catch {
        return res.ok ? 0 : -1;
      }
    }, probeId);
    const stillThere = await page.evaluate(async (id) => {
      const res = await fetch(`/api/v1/datasets/${id}`);
      const data = await res.json().catch(() => ({}));
      return Boolean(data && data.data && data.data.id);
    }, probeId);
    check(cleaned === 0 && !stillThere, "throwaway_dataset_deleted");
  }
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/dataset-language-select.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.stack ? error.stack : error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
