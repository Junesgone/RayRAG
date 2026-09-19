// CDP probe for the ToggleList-backed provider model picker
// (`web/src/components/ui/toggle-list.tsx` as consumed by
// `addable-toggle-list.tsx` inside the provider modal).
//
// Real clicks in the fnOS Chrome: login, open an Available provider card that
// renders the picker, expand the panel and measure upstream's behaviour —
// the lucide `ChevronsDown` trigger glyph rotating 180deg over 200ms, the
// lucide `Search` icon in the search row, the `Loader2` spinner shown while the
// model list loads (which also hides the clear button), and the clear button
// appearing once a query is typed. Nothing is saved: the modal is cancelled.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/provider-toggle-list-cdp.mjs
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

const readTrigger = () =>
  page.evaluate(() => {
    const trigger = document.getElementById("piListModels");
    const chevron = trigger.querySelector(".provider-model-chevron");
    const svg = chevron.querySelector("svg");
    return {
      expanded: trigger.getAttribute("aria-expanded"),
      openClass: trigger.classList.contains("open"),
      glyph: svg ? svg.tagName : null,
      glyphSize: svg ? getComputedStyle(svg).width : null,
      transition: getComputedStyle(chevron).transitionProperty,
      duration: getComputedStyle(chevron).transitionDuration,
      transform: getComputedStyle(chevron).transform,
      panelHidden: document.getElementById("piModelPanel").hidden,
    };
  });

const readSearchRow = () =>
  page.evaluate(() => {
    const icon = document.querySelector(".provider-model-search-icon svg");
    const spinner = document.getElementById("piModelSearchSpinner");
    const clear = document.getElementById("piModelSearchClear");
    return {
      searchIcon: icon ? icon.tagName : null,
      spinnerHidden: spinner.hidden,
      spinnerAnimation: getComputedStyle(spinner).animationName,
      clearHidden: clear.hidden,
      clearGlyph: clear.querySelector("svg") ? "svg" : null,
    };
  });

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
  await page.click(
    "[data-testid='available-model-card'][data-provider='Ollama']",
  );
  await page.waitForFunction(
    () => !document.getElementById("piModelPicker").hidden,
    null,
    { timeout: 15000 },
  );

  results.closed = await readTrigger();
  // Real click on the trigger: the panel opens and the chevron rotates.
  await page.click("#piListModels");
  await page.waitForFunction(
    () => document.getElementById("piListModels").classList.contains("open"),
    null,
    { timeout: 10000 },
  );
  results.searchWhileLoading = await readSearchRow();
  // The rotation is a 200ms transition, so read it after it settles.
  await page.waitForTimeout(350);
  results.opened = await readTrigger();
  await page.screenshot({ path: `${shots}/provider-toggle-open-20260821.png` });
  // Wait for the model list to settle (spinner disappears).
  await page.waitForFunction(
    () => document.getElementById("piModelSearchSpinner").hidden,
    null,
    { timeout: 20000 },
  );
  results.searchAfterLoad = await readSearchRow();

  // The search row only exists once the list exceeds 10 options upstream, so
  // seed a catalogue and re-render through the same code path, then type for
  // real and watch the clear button appear.
  await page.evaluate(() => {
    window.piModels = Array.from({ length: 12 }, (_, index) => ({
      name: `probe-model-${index}`,
      model_type: ["chat"],
      max_tokens: 8192,
      custom: true,
    }));
    window.piModelsLoaded = true;
    window.piModelsLoading = false;
    window.renderProviderModelList();
  });
  await page.waitForFunction(
    () => !document.getElementById("piModelSearchRow").hidden,
    null,
    { timeout: 10000 },
  );
  results.searchRowVisible = await page.evaluate(
    () => !document.getElementById("piModelSearchRow").hidden,
  );
  await page.fill("#piModelSearch", "probe-model-1");
  await page.waitForTimeout(250);
  results.searchWithQuery = await readSearchRow();
  // A forced loading state hides the clear button and shows the spinner.
  await page.evaluate(() => {
    window.piModelsLoading = true;
    window.renderProviderModelList();
  });
  await page.waitForTimeout(150);
  results.searchLoadingForced = await readSearchRow();
  await page.fill("#piModelSearch", "");
  await page.evaluate(() => {
    window.piModelsLoading = false;
    window.renderProviderModelList();
  });
  await page.waitForTimeout(150);
  results.searchSettled = await readSearchRow();

  // Clicking the trigger again closes the panel and un-rotates the chevron.
  await page.click("#piListModels");
  await page.waitForTimeout(300);
  results.closedAgain = await readTrigger();

  // Chinese locale keeps the same structure.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='available-model-card']", {
    timeout: 20000,
  });
  await page.click(
    "[data-testid='available-model-card'][data-provider='Ollama']",
  );
  await page.waitForFunction(
    () => !document.getElementById("piModelPicker").hidden,
    null,
    { timeout: 15000 },
  );
  await page.click("#piListModels");
  await page.waitForTimeout(300);
  results.zh = {
    trigger: await readTrigger(),
    search: await readSearchRow(),
    triggerLabel: (await page.textContent("#piListModelsText")).trim(),
  };
  await page.screenshot({ path: `${shots}/provider-toggle-zh-20260821.png` });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/provider-toggle-error-20260821.png` });
} finally {
  await page
    .evaluate(() => {
      const modal = document.getElementById("providerInstanceModal");
      if (modal) modal.classList.remove("show");
    })
    .catch(() => {});
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
