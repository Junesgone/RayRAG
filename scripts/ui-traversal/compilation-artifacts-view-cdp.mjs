// Dataset Artifacts tab end-to-end (internal verification, not published).
//
// Creates a probe dataset with one parsed chunk, points it at a pipeline id so
// the upstream canGenerateWiki gate opens, then drives the RAGFlow v0.27.2
// compilation UI like a user: Generate -> workspace -> topic -> page detail ->
// search -> graph tab -> mode switch -> clear. The mock model server
// (more scripts/ui-traversal/mock-compilation-model-server.py) answers the map
// and synthesis prompts deterministically.
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const EMAIL = process.env.RAYRAG_LOGIN_EMAIL || "ui-probe-94@example.com";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || "Probe-pass-123";
const SHOT_DIR = process.env.SHOT_DIR || "/tmp";

const checks = [];
function check(name, condition, detail) {
  checks.push({ name, ok: !!condition, detail: detail ?? "" });
  if (!condition) console.error("FAIL", name, detail ?? "");
}

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1500, height: 1000 });
// The editor is localized from the lang cookie; pin English so the assertions
// below can read the upstream labels directly.
await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 300)));

const api = async (path, init) =>
  page.evaluate(
    async ({ path, init }) => {
      const token = localStorage.getItem("rayrag_token") || "";
      const response = await fetch(path, {
        ...(init || {}),
        headers: {
          ...(init && init.body instanceof FormData ? {} : { "Content-Type": "application/json" }),
          Authorization: "Bearer " + token,
          ...((init && init.headers) || {}),
        },
      });
      let body = null;
      try {
        body = await response.json();
      } catch (error) {
        body = null;
      }
      return { status: response.status, body };
    },
    { path, init },
  );

const visible = async (selector) => page.locator(selector).first().isVisible().catch(() => false);
const text = async (selector) => ((await page.locator(selector).first().textContent().catch(() => "")) || "").trim();

let datasetId = null;
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  let loggedIn = await page.evaluate(
    async ({ email, password }) => {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await response.json();
      if (data.code === 0 && data.data && data.data.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { email: EMAIL, password: PASSWORD },
  );
  if (!loggedIn) {
    loggedIn = await page.evaluate(async () => {
      const response = await fetch("/api/v1/users", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          nickname: "ui-probe-94",
          email: "ui-probe-94@example.com",
          password: "Probe-pass-123",
        }),
      });
      const data = await response.json();
      if (data.code === 0 && data.data && data.data.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    });
  }
  check("login", loggedIn);
  console.error("STEP login");
  if (!loggedIn) throw new Error("login failed");

  const stamp = Date.now();
  const created = await api("/api/v1/datasets", {
    method: "POST",
    body: JSON.stringify({
      name: "CDP compilation " + stamp,
      chunk_method: "naive",
      pipeline_id: "mock-pipeline-" + stamp,
    }),
  });
  check("create dataset", created.body && created.body.code === 0, JSON.stringify(created.body).slice(0, 200));
  datasetId = created.body && created.body.data && created.body.data.id;
  check("dataset pipeline id bound", created.body && created.body.data && created.body.data.pipeline_id === "mock-pipeline-" + stamp);
  console.error("STEP dataset");

  const upload = await page.evaluate(
    async ({ datasetId }) => {
      const token = localStorage.getItem("rayrag_token") || "";
      const form = new FormData();
      form.append(
        "file",
        new File(
          ["Alpha is the first subject of the source text. Alpha owns Beta."],
          "alpha.txt",
          { type: "text/plain" },
        ),
      );
      const response = await fetch("/api/v1/datasets/" + datasetId + "/documents", {
        method: "POST",
        headers: { Authorization: "Bearer " + token },
        body: form,
      });
      const data = await response.json();
      const rows = data && data.data;
      const docs = Array.isArray(rows) ? rows : [];
      return { status: response.status, code: data && data.code, docId: docs[0] && docs[0].id, docs: docs.length };
    },
    { datasetId },
  );
  check("upload document", upload.code === 0 && !!upload.docId, JSON.stringify(upload));
  console.error("STEP upload");

  const chunk = await api("/api/v1/datasets/" + datasetId + "/documents/" + upload.docId + "/chunks", {
    method: "POST",
    body: JSON.stringify({ content: "Alpha is the first subject of the source text." }),
  });
  check("add parsed chunk", chunk.body && chunk.body.code === 0, JSON.stringify(chunk.body).slice(0, 200));
  console.error("STEP chunk");

  const detail = await api("/api/v1/datasets/" + datasetId);
  const kb = (detail.body && detail.body.data) || {};
  check("eligibility gate open", Number(kb.chunk_count) > 0 && String(kb.pipeline_id || "").length > 0, JSON.stringify({ chunks: kb.chunk_count, pipeline: kb.pipeline_id }));

  await page.goto(ORIGIN + "/dataset/compilation/" + datasetId, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1500);
  check("compilation panel", await visible("[data-testid='dataset-compilation-panel']"));
  check("empty state before generation", await visible("[data-testid='dataset-compilation-empty']"));
  check("mode selector", await visible("[data-testid='dataset-compilation-mode']"));
  console.error("STEP panel");
  await page.waitForFunction(
    () => {
      const button = document.getElementById("cwpGenerate");
      return button && !button.disabled;
    },
    null,
    { timeout: 20000 },
  );
  check("generate button enabled", true);
  console.error("STEP gen-enabled");
  await page.screenshot({ path: SHOT_DIR + "/rayrag-artifacts-empty.png", fullPage: true });

  await page.click("[data-testid='dataset-compilation-generate']");
  console.error("STEP gen-clicked");
  await page.waitForFunction(
    () => {
      const node = document.getElementById("cwpMsg");
      return node && /Mapped|映射|pages created|生成页面|failed|失败/i.test(node.textContent || "");
    },
    null,
    { timeout: 180000 },
  );
  const compileMessage = await text("#cwpMsg");
  check("generation reports mapping/creation", /Mapped/.test(compileMessage), compileMessage);
  console.error("STEP compile-message");
  await page.waitForFunction(
    () => {
      const work = document.getElementById("cwpWork");
      return work && !work.hidden;
    },
    null,
    { timeout: 30000 },
  );
  check("workspace after generation", await visible("[data-testid='dataset-compilation-work']"));
  const topicRows = await page.locator("[data-testid='cwp-row']").count();
  check("topic rows rendered", topicRows > 0, String(topicRows));
  await page.screenshot({ path: SHOT_DIR + "/rayrag-artifacts-workspace.png", fullPage: true });
  console.error("STEP workspace");

  // Enter the first topic and open its first page.
  await page.locator("[data-testid='cwp-row']").first().click();
  await page.waitForTimeout(500);
  const pageRows = await page.locator("[data-testid='cwp-row']").count();
  check("topic opens page list", pageRows > 0, String(pageRows));
  await page.locator("[data-testid='cwp-row']").first().click();
  await page.waitForFunction(
    () => {
      const title = document.getElementById("cwpDetailTitle");
      return title && title.textContent && title.textContent.trim().length > 0;
    },
    null,
    { timeout: 20000 },
  );
  await page.waitForTimeout(400);
  const detailTitle = await text("#cwpDetailTitle");
  const detailBody = await text("#cwpDetailBody");
  check("page detail title", detailTitle.length > 0, detailTitle);
  check("page detail markdown rendered", /Mock Page|Alpha|Overview/i.test(detailBody), detailBody.slice(0, 160));
  check("detail headings rendered", (await page.locator("#cwpDetailBody h1, #cwpDetailBody h2").count()) > 0);
  await page.screenshot({ path: SHOT_DIR + "/rayrag-artifacts-detail.png", fullPage: true });

  // Search filters the topic/page list.
  await page.fill("[data-testid='dataset-compilation-search']", "Alpha");
  await page.waitForTimeout(400);
  const searchBody = await text("[data-testid='dataset-compilation-list']");
  check("search returns matches", /Alpha/i.test(searchBody), searchBody.slice(0, 160));
  console.error("STEP search");
  await page.fill("[data-testid='dataset-compilation-search']", "");
  await page.waitForTimeout(300);

  // Graph tab has no graph artifacts for this run.
  await page.click("[data-testid='cwp-tab-graph']");
  await page.waitForTimeout(400);
  const graphBody = await text("[data-testid='dataset-compilation-list']");
  check("graph tab empty notice", /No graph artifacts yet/i.test(graphBody), graphBody.slice(0, 120));
  console.error("STEP graph-tab");
  await page.click("[data-testid='cwp-tab-contents']");
  await page.waitForTimeout(300);

  // Mode switch shows the upstream per-mode empty states.
  await page.selectOption("[data-testid='dataset-compilation-mode']", "graph");
  await page.waitForTimeout(900);
  check("graph mode empty title", (await text("#cwpEmptyTitle")) === "No graph yet", await text("#cwpEmptyTitle"));
  check("structure hint auto-compiled", /Compiled automatically/i.test(await text("#cwpEmptyHint")), await text("#cwpEmptyHint"));
  await page.selectOption("[data-testid='dataset-compilation-mode']", "tree");
  await page.waitForTimeout(900);
  check("tree mode empty title", (await text("#cwpEmptyTitle")) === "No navigation nodes", await text("#cwpEmptyTitle"));
  await page.selectOption("[data-testid='dataset-compilation-mode']", "llm-wiki");
  await page.waitForTimeout(1000);
  check("wiki workspace restored", await visible("[data-testid='dataset-compilation-work']"));
  console.error("STEP modes");

  // Clear the wiki: the upstream confirmation dialog is accepted.
  page.once("dialog", (dialog) => dialog.accept());
  await page.click("[data-testid='dataset-compilation-clear']");
  await page.waitForFunction(
    () => {
      const empty = document.getElementById("cwpEmpty");
      return empty && !empty.hidden;
    },
    null,
    { timeout: 20000 },
  );
  const clearedHead = await page.evaluate(async ({ datasetId }) => {
    const token = localStorage.getItem("rayrag_token") || "";
    const response = await fetch("/api/v1/datasets/" + datasetId + "/artifacts", {
      method: "HEAD",
      headers: { Authorization: "Bearer " + token },
    });
    return response.status;
  }, { datasetId });
  check("clear empties artifacts", clearedHead === 404, String(clearedHead));
  check("empty state after clear", await visible("[data-testid='dataset-compilation-empty']"));
  console.error("STEP cleared");
  check("no page errors", pageErrors.length === 0, pageErrors.join(" | "));
} catch (error) {
  checks.push({ name: "traversal crashed", ok: false, detail: String(error).slice(0, 400) });
  console.error(error);
} finally {
  if (datasetId) {
  console.error("STEP cleanup");
    await api("/api/v1/datasets/" + datasetId, { method: "DELETE" }).catch(() => {});
  }
  await page.close().catch(() => {});
  await browser.close().catch(() => {});
  const failed = checks.filter((entry) => !entry.ok);
  console.log(JSON.stringify({ total: checks.length, failed: failed.length, checks }, null, 2));
  if (failed.length > 0) process.exitCode = 1;
}
