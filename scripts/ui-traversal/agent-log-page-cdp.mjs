// CDP probe for the agent log page
// (`web/src/pages/agents/agent-log-page.tsx`, `/agent-log-page/:id`): it seeds a
// throwaway agent, runs two completions so real sessions exist, walks into the
// page with a real click from the agent editor, then exercises the breadcrumb,
// the toolbar (keyword search, date range, Search, Reset), the sortable table,
// the page-size changer, the detail modal and the CSV export — in both locales
// — and finally deletes the probe agent.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/agent-log-page-cdp.mjs
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
const AGENT_TITLE = `cdp-agent-log-probe-${STAMP}`;
const QUESTION = `agent-log-probe-question-${STAMP}`;

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1440, height: 1000 });
const results = { steps: [], errors: [], pageErrors };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

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

  // Seed a canvas whose Begin → Message path answers without any LLM.
  const seeded = await page.evaluate(
    async ({ title, question }) => {
      const created = await fetch("/api/v1/agents", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          title,
          dsl: {
            components: {
              begin: {
                obj: { component_name: "Begin", params: {} },
                upstream: [],
                downstream: ["message"],
              },
              message: {
                obj: {
                  component_name: "Message",
                  params: { content: ["agent log probe answer"] },
                },
                upstream: ["begin"],
                downstream: [],
              },
            },
          },
        }),
      }).then((response) => response.json());
      if (created.code !== 0) return { ok: false, step: "create", body: created };
      const agentId = created.data.id;
      let conversationId = null;
      for (const text of [question, question + " second round"]) {
        const body = { question: text };
        if (conversationId) body.conversation_id = conversationId;
        const answer = await fetch(`/api/v1/agents/${agentId}/completions`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }).then((response) => response.json());
        if (answer.code !== 0) return { ok: false, step: "run", body: answer };
        conversationId = answer.data.conversation_id;
      }
      return { ok: true, agentId, conversationId };
    },
    { title: AGENT_TITLE, question: QUESTION },
  );
  results.seeded = seeded;
  if (!seeded.ok) {
    throw new Error("probe agent seeding failed: " + JSON.stringify(seeded.body));
  }
  const agentId = seeded.agentId;

  // 1. Real click from the agent editor's Log entry.
  await page.goto(`${ORIGIN}/agent/${agentId}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='agent-log-link']", { timeout: 10000 });
  await page.click("[data-testid='agent-log-link']");
  await page.waitForSelector("[data-testid='agent-log-table']", { timeout: 15000 });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='agent-log-row']").length > 0,
    null,
    { timeout: 20000 },
  );
  results.routing = { url: page.url() };
  results.layout = await page.evaluate(() => ({
    breadcrumb: Array.from(document.querySelectorAll(".agent-log-breadcrumb *"))
      .map((node) => node.textContent)
      .filter((text) => text && text !== "/"),
    title: document.querySelector(".agent-log-title").textContent,
    headers: Array.from(
      document.querySelectorAll("[data-testid='agent-log-table'] thead th"),
    ).map((node) => node.textContent.replace(/[↑↓]/g, "").trim()),
    columns: Array.from(
      document.querySelectorAll("[data-testid='agent-log-row'] td"),
    ).map((node) => node.textContent),
    dot: document.querySelector("[data-testid='agent-log-state']").className,
    toolbar: {
      export: !!document.querySelector("[data-testid='agent-log-export']"),
      keywords: !!document.querySelector("[data-testid='agent-log-keywords']"),
      from: !!document.querySelector("[data-testid='agent-log-from']"),
      to: !!document.querySelector("[data-testid='agent-log-to']"),
      search: !!document.querySelector("[data-testid='agent-log-search']"),
      reset: !!document.querySelector("[data-testid='agent-log-reset']"),
    },
    pagination: document
      .querySelector("[data-testid='agent-log-pagination']")
      .textContent.trim(),
  }));
  await page.screenshot({ path: `${shots}/agent-log-page-20260821.png` });

  // 2. Keyword search (matching) and the empty state (non-matching), with real
  //    typing and clicks.
  await page.fill("[data-testid='agent-log-keywords']", QUESTION);
  await page.click("[data-testid='agent-log-search']");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='agent-log-row']").length > 0,
    null,
    { timeout: 15000 },
  );
  results.keywordMatch = await page.evaluate(() => ({
    rows: document.querySelectorAll("[data-testid='agent-log-row']").length,
    pagination: document
      .querySelector("[data-testid='agent-log-pagination']")
      .textContent.trim(),
  }));
  await page.fill("[data-testid='agent-log-keywords']", "no-such-agent-log-entry");
  await page.click("[data-testid='agent-log-search']");
  await page.waitForSelector("[data-testid='agent-log-empty']", { timeout: 15000 });
  results.keywordMiss = await page.evaluate(() => ({
    empty: document.querySelector("[data-testid='agent-log-empty']").textContent,
    rows: document.querySelectorAll("[data-testid='agent-log-row']").length,
  }));
  await page.screenshot({ path: `${shots}/agent-log-empty-20260821.png` });

  // 3. Reset restores the default range and clears the filter.
  await page.click("[data-testid='agent-log-reset']");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='agent-log-row']").length > 0,
    null,
    { timeout: 15000 },
  );
  results.reset = await page.evaluate(() => ({
    keywords: document.getElementById("agentLogKeywords").value,
    from: document.getElementById("agentLogFrom").value,
    to: document.getElementById("agentLogTo").value,
    rows: document.querySelectorAll("[data-testid='agent-log-row']").length,
  }));

  // 4. Sorting toggles the arrow for the clicked column only.
  const sortOnce = async () => {
    await page.click("[data-testid='agent-log-sort-create_date']");
    await page.waitForTimeout(600);
    return page.evaluate(() => ({
      create: document.getElementById("agentLogSortCreate").textContent,
      update: document.getElementById("agentLogSortUpdate").textContent,
    }));
  };
  results.sort = { first: await sortOnce() };
  results.sort.second = await sortOnce();
  results.sort.third = await sortOnce();

  // 5. Page-size changer keeps the table populated.
  await page.selectOption("[data-testid='agent-log-page-size']", "20");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='agent-log-row']").length > 0,
    null,
    { timeout: 15000 },
  );
  results.pageSize = await page.evaluate(() => ({
    text: document
      .querySelector("[data-testid='agent-log-pagination']")
      .textContent.trim(),
    rows: document.querySelectorAll("[data-testid='agent-log-row']").length,
  }));

  // 6. The detail modal (the seeded session has round 2, so the row opens).
  await page.click("[data-testid='agent-log-row']");
  await page.waitForSelector("[data-testid='agent-log-modal'].show", {
    timeout: 10000,
  });
  results.modal = await page.evaluate(() => ({
    title: document.getElementById("agentLogModalTitle").textContent,
    messages: Array.from(
      document.querySelectorAll("[data-testid='agent-log-message']"),
    ).map((node) => ({
      role: node.getAttribute("data-role"),
      body: node.querySelector(".agent-log-message-body").textContent.slice(0, 60),
    })),
  }));
  await page.screenshot({ path: `${shots}/agent-log-modal-20260821.png` });
  await page.click("[data-testid='agent-log-modal-close']");
  await page.waitForFunction(
    () => !document.getElementById("agentLogModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  results.modalClosed = true;

  // 7. CSV export downloads a real file with the upstream name.
  const download = await Promise.all([
    page.waitForEvent("download", { timeout: 15000 }),
    page.click("[data-testid='agent-log-export']"),
  ]).then(([event]) => event);
  results.export = { suggested: download.suggestedFilename() };

  // 8. Chinese locale keeps the same structure with the zh labels.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 30000 });
  await page.waitForSelector("[data-testid='agent-log-table']", { timeout: 15000 });
  results.zh = await page.evaluate(() => ({
    title: document.querySelector(".agent-log-title").textContent,
    headers: Array.from(
      document.querySelectorAll("[data-testid='agent-log-table'] thead th"),
    ).map((node) => node.textContent.replace(/[↑↓]/g, "").trim()),
    toolbar: Array.from(document.querySelectorAll(".agent-log-toolbar button")).map(
      (node) => node.textContent,
    ),
    breadcrumb: Array.from(document.querySelectorAll(".agent-log-breadcrumb *"))
      .map((node) => node.textContent)
      .filter((text) => text && text !== "/"),
  }));
  await page.screenshot({ path: `${shots}/agent-log-zh-20260821.png` });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/agent-log-error-20260821.png` });
} finally {
  if (results.seeded && results.seeded.ok) {
    const cleanup = await page
      .evaluate(async (agentId) => {
        // `DELETE /agents/{id}` leaves the session rows behind (exactly like
        // upstream's `UserCanvasService.delete_by_id`), so the probe removes
        // its own sessions first through `delete_agent_session_item`.
        const sessions = await fetch(
          `/api/v1/canvas/${agentId}/sessions?page_size=100`,
        )
          .then((response) => response.json())
          .catch(() => ({ data: [] }));
        const removed = [];
        for (const row of sessions.data || []) {
          const response = await fetch(
            `/api/v1/agents/${agentId}/sessions/${row.id}`,
            { method: "DELETE" },
          );
          const data = await response.json().catch(() => ({}));
          removed.push({ id: row.id, ok: response.ok && data.code === 0 });
        }
        const response = await fetch(`/api/v1/agents/${agentId}`, {
          method: "DELETE",
        });
        const data = await response.json().catch(() => ({}));
        return {
          ok: response.ok && data.code === 0,
          agentRemoved: response.ok && data.code === 0,
          sessionsRemoved: removed,
          message: data.message || null,
        };
      }, results.seeded.agentId)
      .catch((error) => ({ ok: false, message: String(error) }));
    results.cleanup = cleanup;
  }
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
