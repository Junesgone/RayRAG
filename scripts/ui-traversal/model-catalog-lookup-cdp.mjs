// CDP probe for the public model catalogue (`https://models.agent-one.dev/list`)
// behind the custom provider/model dialogs: a real user opens the provider modal,
// opens Add custom model, types a model name, presses "Look up model info", and the
// dialog fills in the model types, context window and tool-call switch from the
// catalogue. The provider dialog resolves an endpoint to a provider the same way.
// Nothing is saved: both dialogs are closed without Confirm.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/model-catalog-lookup-cdp.mjs
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
// A model the catalogue has carried for years and that every provider mirror
// agrees on: a stable probe target, unlike the newest release of the week.
const PROBE_MODEL = process.env.RAYRAG_CATALOG_PROBE_MODEL || "deepseek-chat";
const PROBE_BASE = process.env.RAYRAG_CATALOG_PROBE_BASE || "https://api.deepseek.com/v1";
const STAMP = Date.now();
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 1000 });
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
const results = { steps: [], pageErrors };
const step = (name, value) => {
  results.steps.push({ name, value });
  console.log(`• ${name}: ${JSON.stringify(value)}`);
};
const fail = (message) => {
  throw new Error(message);
};
let dialogOpened = false;

try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, message: data.message };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = `rayrag_token=${data.data.access_token};path=/`;
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + login.message);
  step("logged in", true);

  // 1. On a fresh deployment nothing is cached yet, and the status route says so
  //    rather than fetching (it must never turn a page view into a 4.5 MB download).
  const cold = await page.evaluate(async () => {
    const res = await fetch("/api/v1/model-catalog/status");
    return res.json();
  });
  if (cold.code !== 0) fail("catalog status failed: " + JSON.stringify(cold));
  step("catalog status before lookup", cold.data);
  if (cold.data.cache_path.split("/").pop() !== "model_catalog.json") {
    fail("the status must name the cached copy: " + cold.data.cache_path);
  }

  // 2. The lookup API answers with capabilities, context and pricing; the first call
  //    fetches the live page, which is why it gets a generous timeout.
  const lookup = await page.evaluate(async ({ model, base }) => {
    const query = new URLSearchParams({ model });
    if (base) query.set("base_url", base);
    const res = await fetch("/api/v1/model-catalog/lookup?" + query.toString());
    return res.json();
  }, { model: PROBE_MODEL, base: PROBE_BASE });
  if (lookup.code !== 0) fail("lookup failed: " + JSON.stringify(lookup));
  results.lookup = {
    matches: lookup.data.matches.length,
    first: lookup.data.matches[0]
      ? {
          name: lookup.data.matches[0].model.name,
          provider: lookup.data.matches[0].provider_name,
          context: lookup.data.matches[0].model.context,
          toolCall: lookup.data.matches[0].model.tool_call,
          inputPrice: lookup.data.matches[0].model.input_price,
          summary: lookup.data.matches[0].summary,
          types: lookup.data.matches[0].suggested_model_types,
        }
      : null,
    provider: lookup.data.provider,
  };
  step("lookup api", results.lookup);
  if (!lookup.data.matches.length) {
    fail(`the catalogue must know ${PROBE_MODEL}`);
  }
  if (!lookup.data.provider) {
    fail(`${PROBE_BASE} must resolve to a provider`);
  }
  if (lookup.data.matches[0].provider_id !== lookup.data.provider.id) {
    fail(
      "the configured endpoint's own models must come first: " +
        JSON.stringify(lookup.data.matches.map((match) => match.provider_id)),
    );
  }

  // The status route now reports the cached copy without another download.
  const warm = await page.evaluate(async () => {
    const res = await fetch("/api/v1/model-catalog/status");
    return res.json();
  });
  step("catalog status after lookup", warm.data);
  if (!warm.data.providers || !warm.data.models) {
    fail("the catalogue must be cached after a lookup: " + JSON.stringify(warm.data));
  }
  if (!warm.data.fetched_at) fail("the cached copy must carry a fetch time");

  // 3. Open the provider modal, then the Add-custom-model dialog, and use the real
  //    button: the fields are filled by the dialog, not by the probe.
  await page.goto(ORIGIN + "/user-setting/model", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='available-model-card']", { timeout: 15000 });
  const opened = await page.evaluate(() => {
    const card = Array.from(
      document.querySelectorAll("[data-testid='available-model-card']"),
    ).find((node) => node.dataset.modelPicker === "true");
    if (!card) return null;
    card.click();
    return { provider: card.dataset.provider };
  });
  if (!opened) throw new Error("no picker provider card available");
  step("provider card", opened);
  await page.waitForSelector("#providerInstanceModal.show", { timeout: 10000 });
  await page.click("#piListModels");
  await page.waitForTimeout(500);
  await page.click("[data-testid='pi-add-custom-model']");
  await page.waitForSelector("#providerCustomModelModal.show", { timeout: 10000 });
  dialogOpened = true;
  await page.fill("#pcmName", PROBE_MODEL);
  const emptyBefore = await page.evaluate(() => ({
    status: document.getElementById("pcmLookupStatus").textContent,
    hidden: document.getElementById("pcmCatalogResult").hidden,
    types: Array.from(document.querySelectorAll("#pcmTypesValues .pi-multiselect-badge-label")).map(
      (node) => node.textContent,
    ),
    maxTokens: document.getElementById("pcmMaxTokens").value,
    tools: document.getElementById("pcmTools").checked,
  }));
  step("dialog before lookup", emptyBefore);
  if (emptyBefore.status) fail("the lookup status must start empty");
  if (!emptyBefore.hidden) fail("the result panel must start hidden");

  await page.click("#pcmLookupButton");
  await page.waitForFunction(
    () => !document.getElementById("pcmCatalogResult").hidden,
    null,
    { timeout: 30000 },
  );
  await page.waitForTimeout(300);
  const after = await page.evaluate(() => {
    // Scoped to the result panel: the lookup button's own row shares the class.
    const rows = Array.from(
      document.querySelectorAll("#pcmCatalogResult .pcm-catalog-row"),
    );
    return {
      rows: rows.length,
      status: document.getElementById("pcmLookupStatus").textContent,
      firstName: rows[0] ? rows[0].querySelector(".pcm-catalog-name").textContent : null,
      chips: rows[0]
        ? Array.from(rows[0].querySelectorAll(".pcm-catalog-chip")).map((node) => node.textContent)
        : [],
      context: rows[0] ? rows[0].querySelector(".pcm-catalog-context").textContent : null,
      price: rows[0] ? rows[0].querySelector(".pcm-catalog-price")?.textContent : null,
      applyLabel: rows[0]
        ? rows[0].querySelector(".pcm-catalog-apply").textContent
        : null,
      types: Array.from(document.querySelectorAll("#pcmTypesValues .pi-multiselect-badge-label")).map(
        (node) => node.textContent,
      ),
      maxTokens: document.getElementById("pcmMaxTokens").value,
      tools: document.getElementById("pcmTools").checked,
      nameValue: document.getElementById("pcmName").value,
    };
  });
  results.afterLookup = after;
  step("dialog after lookup", after);
  if (!after.rows) fail("the lookup must render at least one catalogue row");
  if (!after.context || !after.context.trim()) fail("the row must show the context window");
  if (!after.applyLabel) fail("the row must offer Use these values");
  if (Number(after.maxTokens) <= 0) fail("the lookup must fill the context window");
  if (!after.types.length) fail("the lookup must select at least one model type");
  if (!after.status.includes("providers")) fail("the status line must report the catalogue size");
  await page.screenshot({ path: `${shots}/model-catalog-lookup-${STAMP}.png` });

  // 4. The explicit button overwrites a hand-typed value; nothing else does.
  const overwritten = await page.evaluate(() => {
    document.getElementById("pcmMaxTokens").value = "7";
    document.querySelector("#pcmCatalogResult .pcm-catalog-apply").click();
    return { maxTokens: document.getElementById("pcmMaxTokens").value };
  });
  step("apply overwrites", overwritten);
  if (!Number(overwritten.maxTokens) || Number(overwritten.maxTokens) === 7) {
    fail("Use these values must overwrite the typed max tokens");
  }

  // 5. Closing the dialog clears the answer.
  await page.click("#pcmCancel");
  await page.waitForTimeout(300);
  const closed = await page.evaluate(() => ({
    open: document.getElementById("providerCustomModelModal").classList.contains("show"),
    status: document.getElementById("pcmLookupStatus").textContent,
    hidden: document.getElementById("pcmCatalogResult").hidden,
  }));
  step("dialog closed", closed);
  if (closed.open) fail("Cancel must close the dialog");
  if (closed.status || !closed.hidden) fail("closing must clear the lookup result");
  dialogOpened = false;

  // 6. The provider dialog resolves an endpoint to a provider and its models.
  await page.click("#providerInstanceModal button[onclick='closeProviderInstanceModal()']");
  await page.waitForTimeout(300);
  const triggerLabel = await page.textContent("[data-testid='add-provider-open']");
  await page.click("[data-testid='add-provider-open']");
  await page.waitForSelector("#addProvModal.show", { timeout: 10000 });
  step("add provider trigger", triggerLabel);
  await page.fill("#apBase", PROBE_BASE);
  await page.click("#apLookup");
  await page.waitForFunction(
    () => document.getElementById("apCatalogStatus").textContent.trim().length > 0,
    null,
    { timeout: 30000 },
  );
  await page.waitForTimeout(300);
  const provider = await page.evaluate(() => ({
    status: document.getElementById("apCatalogStatus").textContent,
    id: document.getElementById("apId").value,
    name: document.getElementById("apName").value,
    suggestions: Array.from(document.querySelectorAll("#apModelSuggestions option")).length,
    buttonLabel: document.getElementById("apLookup").textContent,
  }));
  results.providerLookup = provider;
  step("provider lookup", provider);
  if (!provider.status.includes("Provider found")) {
    fail("the provider lookup must report the match: " + provider.status);
  }
  if (!provider.id || !provider.name) fail("the provider id and name must be filled in");
  if (!provider.suggestions) fail("the model suggestions must be filled in");
  await page.screenshot({ path: `${shots}/model-catalog-provider-${STAMP}.png` });

  // Nothing is saved: close the dialog.
  await page.evaluate(() => {
    document.getElementById("addProvModal").classList.remove("show");
  });

  if (pageErrors.length) fail("page errors: " + pageErrors.join("; "));
  results.ok = true;
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  results.error = String(error.message);
  console.error("probe failed:", error.message);
  try {
    await page.screenshot({ path: `${shots}/model-catalog-failure-${STAMP}.png` });
  } catch {}
  process.exitCode = 1;
} finally {
  // Leave the instance as it was found: no open dialog, no half-filled form.
  try {
    if (dialogOpened) await page.click("#pcmCancel");
  } catch {}
  try {
    await page.evaluate(() => {
      document.getElementById("addProvModal")?.classList.remove("show");
      document.getElementById("providerInstanceModal")?.classList.remove("show");
      document.getElementById("providerCustomModelModal")?.classList.remove("show");
    });
  } catch {}
  await page.close();
  await b.close();
}
