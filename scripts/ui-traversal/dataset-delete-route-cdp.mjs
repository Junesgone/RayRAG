// CDP probe for the dataset delete flow on `/datasets`
// (`pages/datasets/index.tsx` card menu → ConfirmDeleteDialog → upstream
// `web/src/utils/api.ts::rmKb`): the card's More button opens the menu, Delete
// opens the dialog, confirming sends `DELETE /api/v1/datasets` with
// `{ids:[...]}` and the card disappears. The throwaway dataset is created and
// removed by the probe itself.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/dataset-delete-route-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const NAME = "probe-delete-" + Date.now();
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
const deleteRequests = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
page.on("request", (request) => {
  if (request.method() === "DELETE" && request.url().includes("/api/v1/datasets")) {
    deleteRequests.push({ url: request.url(), body: request.postData() });
  }
});
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

  const created = await page.evaluate(async (name) => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    const data = await res.json();
    return data.data ? data.data.id : "";
  }, NAME);
  check(Boolean(created), "throwaway_dataset_created");

  await page.goto(`${ORIGIN}/datasets`, { waitUntil: "domcontentloaded", timeout: 40000 });
  const card = page.locator(".ds-card", { hasText: NAME }).first();
  await card.waitFor({ timeout: 20000 });
  check(true, "card_is_listed");

  await card.locator("[data-testid='dataset-card-more']").click();
  // The card menu is built on demand (`#dsCardMenu.home-card-menu`): Rename then
  // the danger-styled Delete.
  const menuDelete = page.locator("#dsCardMenu button.danger").first();
  await menuDelete.waitFor({ timeout: 10000 });
  check(true, "card_menu_opens");
  await menuDelete.click();
  await page.waitForSelector("[data-testid='dataset-delete-dialog'].show", { timeout: 10000 });
  const dialogName = (await page.locator("#dsDeleteName").innerText()).trim();
  check(dialogName === NAME, "confirm_dialog_names_the_dataset", dialogName);

  await page.locator("[data-testid='dataset-delete-confirm']").click();
  await page.waitForFunction(
    (name) => !document.body.innerText.includes(name),
    NAME,
    { timeout: 20000 },
  );
  check(true, "card_disappears_after_delete");

  const request = deleteRequests[deleteRequests.length - 1];
  check(Boolean(request), "delete_request_observed");
  if (request) {
    check(
      request.url.endsWith("/api/v1/datasets"),
      "delete_uses_the_collection_route",
      request.url,
    );
    check(
      (request.body || "").includes(created),
      "delete_body_carries_the_ids_array",
      request.body || "",
    );
  }
  const gone = await page.evaluate(async (id) => {
    const res = await fetch(`/api/v1/datasets/${id}`);
    const data = await res.json().catch(() => ({}));
    return !(data && data.data && data.data.id);
  }, created);
  check(gone, "dataset_is_really_gone");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/dataset-delete-route.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
