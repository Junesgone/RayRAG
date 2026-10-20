// CDP probe for the search page's retrieval payload (web/src/pages/next-search/
// hooks.ts): the seeded search app's search_config drives the request until the
// user edits a slider, after which that value rides the next request only.
import { chromium } from "playwright-core";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required");
  process.exit(2);
}
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e.message)));
const out = { steps: [], pageErrors };
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
  try {
    return JSON.parse(text);
  } catch {
    return { raw: text.slice(0, 120) };
  }
};
const calls = [];
try {
  const login = await api("/api/v1/auth/login", "POST", { email: EMAIL, password: PASSWORD });
  token = login?.data?.access_token;
  if (!token) throw new Error("login failed: " + JSON.stringify(login).slice(0, 120));
  const created = await api("/api/v1/searchapps", "POST", {
    name: "Hooks probe " + Date.now().toString(36),
    kb_ids: [],
    top_k: 37,
    similarity_threshold: 0.42,
    vector_similarity_weight: 0.55,
    related_search: false,
  });
  const appId = created?.data?.id;
  if (!appId) throw new Error("create app failed: " + JSON.stringify(created).slice(0, 160));
  out.appId = appId;

  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(
    async ({ email, password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
      localStorage.setItem("rayrag_token", data.data.access_token);
    },
    { email: EMAIL, password: PASSWORD },
  );
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.route("**/api/v1/retrieval", async (route) => {
    calls.push(JSON.parse(route.request().postData() || "{}"));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, data: { chunks: [], total: 0 } }),
    });
  });

  const openSearch = async () => {
    await page.goto(ORIGIN + "/search?sapp=" + appId, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForFunction(() => typeof window.runSearch === "function", { timeout: 20000 });
    await page.waitForTimeout(500);
    await page.fill("#query", "how does rag work");
    await page.click("#searchHome button.btn-p");
    await page.waitForFunction(
      () => document.getElementById("searchView")?.style.display === "block",
      { timeout: 20000 },
    );
    await page.waitForTimeout(500);
  };

  // 1) Untouched: the app config drives the search; no touched overrides.
  await openSearch();
  out.untouched = {
    touched: await page.evaluate(() => window.__SEARCH_TOUCHED || null),
    body: calls[calls.length - 1],
  };

  // 2) After editing the similarity slider the touched value rides the request.
  await page.evaluate(() => {
    const slider = document.getElementById("thresh");
    slider.value = "0.73";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => window.runSearch());
  await page.waitForTimeout(800);
  out.touched = {
    touched: await page.evaluate(() => window.__SEARCH_TOUCHED || null),
    body: calls[calls.length - 1],
  };
  out.requestCount = calls.length;
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 400);
} finally {
  if (out.appId) {
    const removed = await api("/api/v1/searchapps/" + out.appId, "DELETE");
    out.cleanup = JSON.stringify(removed).slice(0, 80);
  }
  out.pageErrors = pageErrors;
  console.log(JSON.stringify(out, null, 1));
  await page.close().catch(() => {});
  await b.close().catch(() => {});
  process.exit(out.error || pageErrors.length ? 1 : 0);
}
