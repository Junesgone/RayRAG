// CDP probe for the data-source connector dialogs (upstream
// `pages/user-setting/data-source/{add-datasource-modal.tsx,constant/*.tsx}`): the
// dynamic form's hover tooltips, the informational panels that follow the segmented
// scope, and the cross-field rules that refuse an unusable configuration with the
// upstream sentence.
//
// Everything is driven by real clicks and typing. The probe creates one SeaFile source
// and deletes it again, so the instance is left exactly as it was found.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/data-source-form-cdp.mjs
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
const STAMP = Date.now();
const SOURCE_NAME = `cdp-seafile-${STAMP}`;
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
let createdId = null;

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) throw new Error("login failed");
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = `rayrag_token=${data.data.access_token};path=/`;
  }, PASSWORD);
  step("logged in", true);

  // 1. Open the Add-data-source dialog for SeaFile: the form is generated from the
  //    schema, and the segmented scope control decides which fields exist.
  await page.goto(`${ORIGIN}/user-setting/data-source`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  // The dialog starts hidden; the card grid is what proves the page is ready.
  await page.waitForSelector("#dsAvailable .cc-available-card", { timeout: 30000 });
  const opened = await page.evaluate(() => {
    // The card grid is built from the schema manifest; click the SeaFile card's own
    // `+ Add` button, exactly as an operator would.
    const card = document.querySelector("#dsAvailable .cc-available-card[data-type='seafile']");
    if (!card) return null;
    const add = card.querySelector(".cc-add-btn");
    if (!add) return null;
    add.click();
    return { type: card.dataset.type, label: card.textContent.trim().slice(0, 20) };
  });
  if (!opened) fail("the data-source page has no SeaFile entry");
  await page.waitForSelector("#dsAddModal.show", { timeout: 15000 });
  step("card clicked", opened);
  const accountScope = await page.evaluate(() => ({
    panels: Array.from(document.querySelectorAll("#dsAddFields .ds-panel:not([hidden])")).map(
      (node) => node.textContent.trim(),
    ),
    // The tooltip trigger and popup live inside the label span (as upstream's `Tooltip`
    // does), so the label is the span's own text node.
    fields: Array.from(document.querySelectorAll("#dsAddFields .provider-instance-field"))
      .filter((node) => !node.hidden)
      .map((node) => node.querySelector("span")?.firstChild?.textContent.trim()),
    tooltips: Array.from(document.querySelectorAll("#dsAddFields .ds-tip-trigger")).length,
  }));
  results.accountScope = accountScope;
  step("account scope", accountScope);
  if (!accountScope.panels.some((text) => text.includes("all libraries visible to the Account"))) {
    fail("the account scope must show its panel: " + JSON.stringify(accountScope.panels));
  }
  if (accountScope.panels.some((text) => text.includes("Provide one of these"))) {
    fail("the token panel belongs to the library/directory scopes");
  }
  if (!accountScope.tooltips) fail("the fields must carry hover tooltips");

  // 2. The tooltip is a hover popup, not a permanent paragraph.
  const tooltip = await page.evaluate(async () => {
    const trigger = document.querySelector("#dsAddFields .ds-tip-trigger");
    const popup = trigger.parentElement.querySelector(".ds-tip-popup");
    const before = popup.classList.contains("show");
    trigger.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    const after = popup.classList.contains("show");
    trigger.dispatchEvent(new MouseEvent("mouseleave", { bubbles: false }));
    return {
      text: popup.textContent.slice(0, 60),
      hiddenBefore: !before,
      shownOnHover: after,
      hiddenAfterLeave: !popup.classList.contains("show"),
      role: popup.getAttribute("role"),
    };
  });
  results.tooltip = tooltip;
  step("hover tooltip", tooltip);
  if (!tooltip.hiddenBefore || !tooltip.shownOnHover || !tooltip.hiddenAfterLeave) {
    fail("the tooltip must appear on hover and disappear on leave");
  }
  if (tooltip.role !== "tooltip") fail("the popup must be announced as a tooltip");
  await page.screenshot({ path: `${shots}/data-source-form-${STAMP}.png` });

  // 3. Switching the scope swaps the panel and the visible credential fields.
  const libraryScope = await page.evaluate(() => {
    const button = Array.from(document.querySelectorAll("#dsAddFields .ds-segment button")).find(
      (node) => node.dataset.value === "library",
    );
    if (!button) return null;
    button.click();
    return true;
  });
  if (!libraryScope) fail("the sync-scope segmented control is missing");
  await page.waitForTimeout(300);
  const library = await page.evaluate(() => ({
    panels: Array.from(document.querySelectorAll("#dsAddFields .ds-panel:not([hidden])")).map(
      (node) => node.textContent.trim(),
    ),
    fields: Array.from(document.querySelectorAll("#dsAddFields .provider-instance-field"))
      .filter((node) => !node.hidden)
      .map((node) =>
        (node.querySelector("span")?.firstChild?.textContent || "").replace(" *", "").trim(),
      ),
  }));
  results.libraryScope = library;
  step("library scope", library);
  if (!library.panels.some((text) => text.includes("Provide one of these authentication methods"))) {
    fail("the library scope must show the token panel: " + JSON.stringify(library.panels));
  }
  if (library.panels.some((text) => text.includes("all libraries visible to the Account"))) {
    fail("the account panel must disappear in the library scope");
  }
  for (const label of ["Library Token", "Library ID"]) {
    if (!library.fields.includes(label)) {
      fail(`the library scope must ask for ${label}: ${JSON.stringify(library.fields)}`);
    }
  }

  // 4. Submitting an unusable configuration is refused with the upstream sentence — and
  //    the request never leaves the page.
  const refused = await page.evaluate(async () => {
    document.getElementById("dsAddName").value = "cdp-seafile-refused";
    // Fill the plain required field first: the rule under test is the cross-field one.
    const urlWrap = Array.from(
      document.querySelectorAll("#dsAddFields .provider-instance-field"),
    ).find((node) => (node.querySelector("span")?.firstChild?.textContent || "").startsWith("SeaFile Server URL"));
    if (urlWrap) {
      const input = urlWrap.querySelector("input");
      input.value = "https://sf.example.com";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    const requests = [];
    const originalFetch = window.fetch;
    window.fetch = function (...args) {
      requests.push(String(args[0]));
      return originalFetch.apply(this, args);
    };
    document.getElementById("dsAddConfirm").click();
    await new Promise((resolve) => setTimeout(resolve, 500));
    window.fetch = originalFetch;
    return {
      message: document.getElementById("dsAddError").textContent,
      field: document.getElementById("dsAddError").dataset.field,
      posted: requests.some((url) => url.includes("/api/v1/data_sources")),
      stillOpen: document.getElementById("dsAddModal").classList.contains("show"),
    };
  });
  results.refused = refused;
  step("refused configuration", refused);
  if (!refused.message.includes("Provide either an Account API Token or a Library Token")) {
    fail("the upstream message must be shown: " + refused.message);
  }
  if (refused.posted) fail("an invalid configuration must not be sent");
  if (!refused.stillOpen) fail("the dialog must stay open so the operator can fix it");

  // 5. The server refuses it too, even when the page is bypassed.
  const serverRefusal = await page.evaluate(async () => {
    const res = await fetch("/api/v1/data_sources", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: "cdp-bypass",
        type: "seafile",
        config: { seafile_url: "https://sf.example.com", sync_scope: "account" },
      }),
    });
    return { status: res.status, body: await res.json() };
  });
  results.serverRefusal = serverRefusal;
  step("server refusal", serverRefusal);
  if (serverRefusal.status === 200 || serverRefusal.body.code === 0) {
    fail("the API must reject the same configuration the dialog rejects");
  }
  // The refusal is readable: RAGFlow's `{code, message}` body, not plain text.
  if (serverRefusal.body.message !== "Account API Token is required for Entire Account scope") {
    fail("the API refusal must carry the message: " + JSON.stringify(serverRefusal.body));
  }

  // 6. A complete configuration is saved, then removed again.
  const saved = await page.evaluate(
    async (name) => {
      const setValue = (labelText, value) => {
        const wrap = Array.from(
          document.querySelectorAll("#dsAddFields .provider-instance-field"),
        ).find((node) =>
          (node.querySelector("span")?.firstChild?.textContent || "").startsWith(labelText),
        );
        if (!wrap) return false;
        const input = wrap.querySelector("input");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
        return true;
      };
      document.getElementById("dsAddName").value = name;
      setValue("SeaFile Server URL", "https://sf.example.com");
      setValue("Library Token", "library-token");
      setValue("Library ID", "7a9e1b3c-4d5f-6a7b-8c9d-0e1f2a3b4c5d");
      document.getElementById("dsAddConfirm").click();
      await new Promise((resolve) => setTimeout(resolve, 1200));
      const listed = await fetch("/api/v1/data_sources")
        .then((response) => response.json())
        .then((data) => (data.data || []).find((row) => row.name === name));
      return { closed: !document.getElementById("dsAddModal").classList.contains("show"), id: listed?.id || null };
    },
    SOURCE_NAME,
  );
  results.saved = saved;
  step("saved configuration", saved);
  if (!saved.id) fail("the completed configuration must be stored");

  // 7. S3: the region selector is a combobox (`allowCustomValue` upstream), so a region
  //    newer than the embedded AWS list can be typed — and it must survive the round trip.
  const customRegion = await page.evaluate(
    async (name) => {
      const card = document.querySelector("#dsAvailable .cc-available-card[data-type='s3']");
      if (!card) return { error: "no S3 card" };
      card.querySelector(".cc-add-btn").click();
      await new Promise((resolve) => setTimeout(resolve, 400));
      const wrapFor = (label) =>
        Array.from(document.querySelectorAll("#dsAddFields .provider-instance-field")).find(
          (node) =>
            (node.querySelector("span")?.firstChild?.textContent || "").startsWith(label),
        );
      document.getElementById("dsAddName").value = name;
      const bucket = wrapFor("Bucket Name");
      if (!bucket) return { error: "no bucket field" };
      bucket.querySelector("input").value = "cdp-bucket";
      bucket.querySelector("input").dispatchEvent(new Event("input", { bubbles: true }));
      const regionWrap = wrapFor("Region");
      const regionInput = regionWrap?.querySelector(".ds-custom-select");
      if (!regionInput) return { error: "the region field is not a combobox" };
      const role = regionInput.getAttribute("role");
      const listId = regionInput.getAttribute("list");
      const suggestions = document.getElementById(listId)?.options.length || 0;
      regionInput.value = "ap-southeast-9";
      regionInput.dispatchEvent(new Event("input", { bubbles: true }));
      // A tooltip that arrived with this round: Prefix.
      const prefixWrap = wrapFor("Prefix");
      const trigger = prefixWrap?.querySelector(".ds-tip-trigger");
      let tooltip = null;
      if (trigger) {
        trigger.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
        tooltip = prefixWrap.querySelector(".ds-tip-popup")?.classList.contains("show");
      }
      document.getElementById("dsAddConfirm").click();
      await new Promise((resolve) => setTimeout(resolve, 1200));
      const listed = await fetch("/api/v1/data_sources")
        .then((response) => response.json())
        .then((data) => (data.data || []).find((row) => row.name === name));
      let stored = null;
      if (listed) {
        stored = await fetch(`/api/v1/data_sources/${listed.id}`)
          .then((response) => response.json())
          .then((data) => data.data?.config?.credentials?.region ?? null);
      }
      return {
        role,
        suggestions,
        tooltip,
        id: listed?.id || null,
        stored,
        // Why a save failed, if it did.
        error: listed ? null : document.getElementById("dsAddError").textContent,
        closed: !document.getElementById("dsAddModal").classList.contains("show"),
      };
    },
    `cdp-s3-${STAMP}`,
  );
  results.customRegion = customRegion;
  step("custom region", customRegion);
  if (customRegion.error) fail(customRegion.error);
  if (customRegion.role !== "combobox") fail("the region field must be a combobox");
  if (!customRegion.suggestions) fail("the combobox must suggest the AWS regions");
  if (!customRegion.tooltip) fail("the Prefix tooltip must appear on hover");
  if (customRegion.stored !== "ap-southeast-9") {
    fail("the typed region must be stored, not silently replaced: " + customRegion.stored);
  }
  await page.evaluate(
    async (id) => fetch(`/api/v1/data_sources/${id}`, { method: "DELETE" }),
    customRegion.id,
  );
  step("s3 source removed", true);

  const removed = await page.evaluate(
    async (id) =>
      fetch(`/api/v1/data_sources/${id}`, { method: "DELETE" }).then((response) =>
        response.json(),
      ),
    saved.id,
  );
  step("removed again", { code: removed.code });
  if (removed.code !== 0) fail("the probe's data source must be removed: " + JSON.stringify(removed));
  createdId = null;

  if (pageErrors.length) fail("page errors: " + pageErrors.join("; "));
  results.ok = true;
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  results.error = String(error.message);
  console.error("probe failed:", error.message);
  try {
    await page.screenshot({ path: `${shots}/data-source-form-failure-${STAMP}.png` });
  } catch {}
  process.exitCode = 1;
} finally {
  try {
    if (createdId) {
      await page.evaluate(
        async (id) => fetch(`/api/v1/data_sources/${id}`, { method: "DELETE" }),
        createdId,
      );
    }
  } catch {}
  try {
    await page.evaluate(async (name) => {
      const listed = await fetch("/api/v1/data_sources")
        .then((response) => response.json())
        .then((data) => data.data || []);
      for (const row of listed.filter((entry) => entry.name === name)) {
        await fetch(`/api/v1/data_sources/${row.id}`, { method: "DELETE" });
      }
    }, SOURCE_NAME);
  } catch {}
  await page.close();
  await b.close();
}
