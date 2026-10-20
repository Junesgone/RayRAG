// CDP probe for the search page's settings drawer
// (`next-search/search-setting.tsx`): open the drawer from the toolbar trigger,
// check the field set and their order, toggle the rerank and AI-summary
// switches, add a manual metadata-filter row, save through the drawer's
// `search-settings-save` button, and confirm the app record took the change.
// The probe creates a one-off search app and deletes it again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... RAYRAG_ADMIN_EMAIL=... \
//        node scripts/ui-traversal/search-settings-drawer-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
// The drawer saves through `PUT /api/v1/searchapps/{id}`; the body it sent is
// the strongest evidence of what the drawer collected.
const saves = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (!url.pathname.startsWith("/api/v1/searchapps")) return;
  if (!["PUT", "POST"].includes(request.method())) return;
  let body = request.postData() || "";
  try {
    const parsed = JSON.parse(body);
    body = { name: parsed.name, method: parsed.meta_data_filter?.method ?? null };
  } catch {
    body = body.slice(0, 120);
  }
  saves.push({ method: request.method(), path: url.pathname, body });
});
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

let token = null;
const api = async (path, method = "GET", body) => {
  const res = await fetch(ORIGIN + path, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: "Bearer " + token } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    json = { raw: text.slice(0, 120) };
  }
  return { status: res.status, body: json };
};

const fieldIds = [
  "ssName",
  "ssDesc",
  "kbChips",
  "ssMetaMethod",
  "ssRefMeta",
  "ssThreshRange",
  "thresh",
  "ssWeightRange",
  "vWeight",
  "rerank",
  "ssTopKRange",
  "topK",
  "ssSummary",
  "ssFreedom",
  "ssTemperature",
  "ssTopP",
  "ssPresencePenalty",
  "ssFrequencyPenalty",
  "relatedSearch",
  "ssMindmap",
];

try {
  const login = await api("/api/v1/auth/login", "POST", { email: EMAIL, password: PASSWORD });
  token = login.body?.data?.access_token;
  check(Boolean(token), "admin_login", JSON.stringify(login));

  const created = await api("/api/v1/searchapps", "POST", {
    name: `Drawer probe ${Date.now().toString(36)}`,
    kb_ids: [],
    hybrid: false,
    rerank: false,
    related_search: false,
  });
  const appId = created.body?.data?.id;
  check(Boolean(appId), "probe_app_created", JSON.stringify(created));
  results.appId = appId;

  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${ORIGIN}/login`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(
    async ({ email, password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
      localStorage.setItem("rayrag_token", data.data.access_token);
      document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    },
    { email: EMAIL, password: PASSWORD },
  );
  await page.goto(`${ORIGIN}/search`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(() => typeof window.ssToggle === "function", { timeout: 15000 });
  // The drawer's trigger lives in the results toolbar, which appears once a
  // search has been run (`#searchView` starts hidden, like upstream).
  await page.fill("#query", "drawer probe");
  await page.click("#searchHome button");
  await page.waitForFunction(
    () => document.getElementById("searchView")?.style.display === "block",
    { timeout: 15000 },
  );
  await page.waitForTimeout(500);

  // Closed by default, opened from the toolbar trigger.
  check(
    (await page.getAttribute("#ssDrawer", "data-open")) === "false",
    "drawer_starts_closed",
  );
  await page.click("[data-testid='search-settings-trigger']");
  await page.waitForTimeout(400);
  check(
    (await page.getAttribute("#ssDrawer", "data-open")) === "true" &&
      (await page.getAttribute("#ssOpen", "aria-expanded")) === "true" &&
      (await page.locator("#ssDrawer").isVisible()),
    "drawer_opens_from_trigger",
  );

  // Avatar upload goes through the shared crop dialog (avatar-upload.tsx):
  // pick a real PNG, confirm the 64x64 crop, and keep the data URL for save.
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    "base64",
  );
  await page.setInputFiles("#ssAvatarFile", { name: "avatar.png", mimeType: "image/png", buffer: png });
  await page.waitForFunction(
    () => document.getElementById("avatarCropModal")?.classList.contains("show"),
    { timeout: 15000 },
  );
  check(
    await page.locator("[data-testid='avatar-crop-dialog']").isVisible(),
    "avatar_crop_dialog_opens",
  );
  await page.click("#avatarCropOk");
  await page.waitForTimeout(400);
  const avatarPreview = await page.evaluate(() => {
    const image = document.getElementById("ssAvatarImg");
    const clear = document.getElementById("ssAvatarClear");
    return {
      hidden: image ? image.hidden : null,
      src: image ? String(image.getAttribute("src") || "").slice(0, 32) : "",
      clearHidden: clear ? clear.hidden : null,
    };
  });
  results.avatarPreview = avatarPreview;
  check(
    avatarPreview.hidden === false &&
      avatarPreview.src.startsWith("data:image/png") &&
      avatarPreview.clearHidden === false,
    "avatar_crop_writes_preview_and_clear",
    JSON.stringify(avatarPreview),
  );

  const missing = await page.evaluate(
    (ids) => ids.filter((id) => !document.getElementById(id)),
    fieldIds,
  );
  check(missing.length === 0, "drawer_fields_present", JSON.stringify(missing));
  const order = await page.evaluate(() =>
    Array.from(document.querySelectorAll("#ssDrawer .ss-body > *")).map((node) => {
      const inner = node.querySelector("input[id],select[id],div[id]");
      return node.id || inner?.id || node.className;
    }),
  );
  results.fieldOrder = order;
  // The upstream field order of `search-setting.tsx`.
  const expectedOrder = [
    "ssAvatarFile",
    "ssName",
    "ssDesc",
    "kbChips",
    "ssMetaMethod",
    "ssRefMeta",
    "ssMetaFieldsRow",
    "ssThreshRange",
    "ssWeightRange",
    "rerank",
    "searchRerankRow",
    "ssTopKField",
    "ssSummary",
    "ssLlmFields",
    "relatedSearch",
    "ssMindmap",
  ];
  check(
    JSON.stringify(order) === JSON.stringify(expectedOrder),
    "drawer_field_order_matches_upstream",
    JSON.stringify(order),
  );

  // Rerank switch reveals the required model tree and the Top K slider.
  check(await page.locator("#ssTopKField").isHidden(), "topk_hidden_until_rerank");
  await page.click("[data-testid='search-settings-rerank']");
  await page.waitForTimeout(300);
  const rerankState = await page.evaluate(() => {
    const box = document.getElementById("rerank");
    const row = document.getElementById("searchRerankRow");
    const topk = document.getElementById("ssTopKField");
    return {
      checked: box ? box.checked : null,
      rowExists: Boolean(row),
      rowHidden: row ? row.hidden : null,
      rowDisplay: row ? row.style.display : null,
      rowVisible: row ? row.getBoundingClientRect().height > 0 : null,
      topkHidden: topk ? topk.hidden : null,
      topkVisible: topk ? topk.getBoundingClientRect().height > 0 : null,
      selectors: document.querySelectorAll("#searchRerankSel").length,
    };
  });
  results.rerankState = rerankState;
  check(
    rerankState.checked === true &&
      rerankState.rowVisible === true &&
      rerankState.topkVisible === true &&
      rerankState.selectors === 1,
    "rerank_reveals_model_tree_and_topk",
    JSON.stringify(rerankState),
  );

  // AI summary reveals the LLM rows.
  check(await page.locator("#ssLlmFields").isHidden(), "llm_rows_hidden_until_summary");
  await page.click("[data-testid='search-settings-summary']");
  await page.waitForTimeout(200);
  check(await page.locator("#ssLlmFields").isVisible(), "summary_reveals_llm_rows");

  // Manual metadata filter adds a row with the upstream operator list.
  await page.selectOption("#ssMetaMethod", "manual");
  await page.waitForTimeout(300);
  const rowCount = await page.locator("#ssMetaManualRows .ss-meta-row").count();
  const ops = await page.evaluate(() =>
    Array.from(document.querySelectorAll("#ssMetaManualRows .ss-meta-op option")).map(
      (option) => option.value,
    ),
  );
  results.metadataOperators = ops;
  check(
    rowCount === 1 &&
      ops.length === 14 &&
      ops.includes("contains") &&
      ops.includes("≥") &&
      ops.includes("not in"),
    "manual_filter_row_and_operators",
    JSON.stringify({ rowCount, ops }),
  );
  await page.fill("#ssMetaManualRows .ss-meta-key", "owner");
  await page.fill("#ssMetaManualRows .ss-meta-value", "Farm A");

  // The request body the page would send carries the drawer's filter.
  const payload = await page.evaluate(() => ssMetaFilterPayload());
  results.metaPayload = payload;
  check(
    payload &&
      payload.method === "manual" &&
      payload.logic === "and" &&
      payload.manual.length === 1 &&
      payload.manual[0].key === "owner" &&
      payload.manual[0].value === "Farm A",
    "manual_filter_payload",
    JSON.stringify(payload),
  );
  const semi = await page.evaluate(() => {
    document.getElementById("ssMetaMethod").value = "semi_auto";
    ssMetaChange();
    const rows = document.querySelectorAll("#ssMetaSemiRows .ss-meta-row");
    return { rows: rows.length, payload: ssMetaFilterPayload() };
  });
  results.semiPayload = semi;
  check(
    semi.rows === 1 && semi.payload && semi.payload.method === "semi_auto",
    "semi_auto_payload",
    JSON.stringify(semi),
  );

  // Cancel closes without saving.
  await page.click("#ssCancel");
  await page.waitForTimeout(300);
  check(
    (await page.getAttribute("#ssDrawer", "data-open")) === "false",
    "cancel_closes_drawer",
  );

  // Reopen against the saved app, rename and save through the drawer.
  await page.goto(`${ORIGIN}/search?sapp=${appId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(() => typeof window.ssToggle === "function", { timeout: 15000 });
  // `#searchView` (and with it the trigger) only appears after a search.
  await page.fill("#query", "drawer probe");
  await page.click("#searchHome button");
  await page.waitForFunction(
    () => document.getElementById("searchView")?.style.display === "block",
    { timeout: 15000 },
  );
  await page.waitForTimeout(500);
  await page.click("[data-testid='search-settings-trigger']");
  await page.waitForTimeout(300);
  const renamed = `Drawer saved ${Date.now().toString(36)}`;
  // Upload the avatar through the crop dialog in the session that saves, so
  // the data URL rides the PUT payload.
  await page.setInputFiles("#ssAvatarFile", { name: "avatar.png", mimeType: "image/png", buffer: png });
  await page.waitForFunction(
    () => document.getElementById("avatarCropModal")?.classList.contains("show"),
    { timeout: 15000 },
  );
  await page.click("#avatarCropOk");
  await page.waitForTimeout(400);
  await page.fill("#ssName", renamed);
  await page.click("[data-testid='search-settings-save']");
  await page.waitForTimeout(1500);
  const stored = await api(`/api/v1/searchapps/${appId}`);
  results.saves = saves;
  results.storedName = stored.body?.data?.name;
  check(
    stored.body?.data?.name === renamed,
    "save_persists_through_searchapps",
    JSON.stringify(stored.body?.data?.name),
  );
  check(
    (await page.getAttribute("#ssDrawer", "data-open")) === "false",
    "save_closes_drawer",
  );
  check(
    String(stored.body?.data?.avatar || "").startsWith("data:image/png"),
    "save_persists_avatar_data_url",
    JSON.stringify(String(stored.body?.data?.avatar || "").slice(0, 32)),
  );

  // The stored avatar reopens in the drawer; the clear affordance removes it.
  // Refresh the page state from the saved record, then reopen the drawer.
  await page.evaluate(
    (app) => {
      window.__SAPP__ = app;
      if (typeof ssLoad === "function") ssLoad(app);
      if (typeof ssToggle === "function") ssToggle(true);
    },
    stored.body.data,
  );
  await page.waitForTimeout(400);
  check(
    await page.locator("[data-testid='search-settings-avatar-clear']").isVisible(),
    "stored_avatar_restores_clear_button",
  );
  await page.evaluate(() => document.getElementById("ssAvatarClear").click());
  await page.waitForTimeout(200);
  await page.click("[data-testid='search-settings-save']");
  await page.waitForTimeout(1500);
  const cleared = await api("/api/v1/searchapps/" + appId);
  results.clearedAvatar = cleared.body?.data?.avatar;
  check(
    cleared.body?.data?.avatar === "",
    "avatar_clear_persists",
    JSON.stringify(cleared.body?.data?.avatar),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page
    .screenshot({ path: "docs/ui-traversal/rayrag-audit/search-settings-drawer-failure.png" })
    .catch(() => {});
} finally {
  if (results.appId) {
    const removed = await api(`/api/v1/searchapps/${results.appId}`, "DELETE");
    results.cleanup = `${removed.status} ${JSON.stringify(removed.body).slice(0, 120)}`;
    check(removed.status === 200, "probe_app_removed", results.cleanup);
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 1));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
