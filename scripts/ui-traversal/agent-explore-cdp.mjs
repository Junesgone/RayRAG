// CDP probe for the agent Launch surface
// (`web/src/pages/agent/explore/index.tsx`, upstream `/agent/:id/explore`):
// it seeds a throwaway conversational canvas, walks in with a real click on the
// agent toolbar's Launch button, then exercises the session column (new
// session, search filter, hover ellipsis menu, confirm-delete), the session
// chat (first-send session creation + SSE answer + reload transcript) and the
// zh labels — and finally deletes the probe canvas.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/agent-explore-cdp.mjs
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
const AGENT_TITLE = `cdp-agent-explore-probe-${STAMP}`;
const QUESTION = `explore probe question ${STAMP}`;
const ANSWER = "explore probe answer";
const PROLOGUE = "Hi! I'm your assistant.";

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
const requests = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
page.on("request", (request) => {
  if (request.method() === "POST" || request.method() === "DELETE") {
    requests.push({ method: request.method(), url: request.url() });
  }
});
await page.setViewportSize({ width: 1440, height: 1000 });
const results = { steps: [], errors: [], pageErrors };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

const step = (name, value) => {
  results.steps.push({ name, value });
  console.log(`• ${name}: ${JSON.stringify(value)}`);
};
const fail = (message) => {
  throw new Error(message);
};

let agentId = null;
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
  if (!login.ok) fail("login failed: " + JSON.stringify(login.body));
  step("login", "ok");

  // Seed a conversational canvas whose Begin → Message path answers without any
  // LLM (same no-model fixture the agent-log probe uses) plus a prologue.
  const created = await page.evaluate(async (title) => {
    const response = await fetch("/api/v1/agents", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        title,
        dsl: {
          components: {
            begin: {
              obj: {
                component_name: "Begin",
                params: {},
                mode: "conversational",
                prologue: "Hi! I'm your assistant.",
              },
              upstream: [],
              downstream: ["message"],
            },
            message: {
              obj: {
                component_name: "Message",
                params: { content: ["explore probe answer"] },
              },
              upstream: ["begin"],
              downstream: [],
            },
          },
        },
      }),
    }).then((res) => res.json());
    return response;
  }, AGENT_TITLE);
  if (created.code !== 0) fail("agent create failed: " + JSON.stringify(created));
  agentId = created.data.id;
  step("seeded agent", agentId);

  // 1. Real click on the canvas toolbar's Launch button.
  await page.goto(`${ORIGIN}/agent/${agentId}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='agent-launch']", { timeout: 10000 });
  await page.click("[data-testid='agent-launch']");
  await page.waitForSelector("[data-testid='explore-session-list']", {
    timeout: 15000,
  });
  // The shell already ships the no-session placeholder and the disabled send
  // button (upstream's first paint); waiting for the boot script's own pass
  // keeps the layout assertions deterministic.
  await page.waitForSelector("[data-testid='explore-no-session']", {
    timeout: 15000,
  });
  await page.waitForFunction(
    () => document.querySelector("[data-testid='explore-send']").disabled === true,
    null,
    { timeout: 15000 },
  );
  results.routing = { url: page.url() };
  if (!page.url().includes(`/agent/${agentId}/explore`)) {
    fail("Launch did not navigate to the explore route: " + page.url());
  }
  step("launch navigation", page.url());

  // 2. Layout parity: breadcrumb, session column, chat column.
  results.layout = await page.evaluate(() => {
    const visible = (selector) => {
      const node = document.querySelector(selector);
      return !!node && !node.hidden;
    };
    const rect = (selector) => {
      const node = document.querySelector(selector);
      return node ? Math.round(node.getBoundingClientRect().width) : null;
    };
    return {
      breadcrumb: Array.from(
        document.querySelectorAll("[data-testid='explore-breadcrumb-agent'], .chunk-breadcrumb-sep, [data-testid='explore-breadcrumb-title']"),
      ).map((node) => node.textContent.trim() || "/"),
      sessionsTitle: document.querySelector(
        ".agent-explore-sessions-title h2",
      ).textContent,
      sessionColumnWidth: rect(".agent-explore-sessions"),
      placeholders: Array.from(
        document.querySelectorAll(".agent-explore-placeholder"),
      ).map((node) => node.textContent),
      sendDisabled: document.querySelector("[data-testid='explore-send']")
        .disabled,
      stopVisible: visible("[data-testid='explore-stop']"),
      searchPlaceholder: document.querySelector(
        "[data-testid='explore-session-search']",
      ).placeholder,
      cssClasses: Array.from(
        document.querySelectorAll(".agent-explore-card, .agent-explore-input-row"),
      ).length,
    };
  });
  if (results.layout.sessionsTitle !== "Sessions") {
    fail("session column heading is not upstream's: " + results.layout.sessionsTitle);
  }
  if (!results.layout.placeholders.includes("Please select a session or create a new one")) {
    fail("no-session placeholder missing: " + JSON.stringify(results.layout.placeholders));
  }
  if (results.layout.sendDisabled !== true) {
    fail("send button must start disabled until the input has text");
  }
  if (results.layout.sessionColumnWidth !== 296) {
    fail("session column is not the upstream 296px: " + results.layout.sessionColumnWidth);
  }
  await page.screenshot({ path: `${shots}/agent-explore-launch-${STAMP}.png` });
  step("layout", results.layout);

  // 3. Real click on the ghost Plus button → temporary session.
  await page.click("[data-testid='explore-new-session']");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='explore-session-card']").length > 0,
    null,
    { timeout: 10000 },
  );
  results.newSession = await page.evaluate(() => ({
    url: location.search,
    cards: Array.from(
      document.querySelectorAll("[data-testid='explore-session-card']"),
    ).map((node) => node.textContent.trim()),
    count: document.querySelector("[data-testid='explore-session-count']").textContent,
    placeholder: document.querySelector(".agent-explore-placeholder")?.textContent,
  }));
  if (!results.newSession.url.includes("isNew=true")) {
    fail("temporary session did not update the URL: " + results.newSession.url);
  }
  if (results.newSession.cards[0] !== "New Session") {
    fail("temporary card label mismatch: " + JSON.stringify(results.newSession.cards));
  }
  step("new session", results.newSession);

  // 4. Type a question and send it: the session is created first, then the SSE
  //    run streams into the assistant bubble.
  await page.fill("[data-testid='explore-input']", QUESTION);
  await page.waitForFunction(
    () => !document.querySelector("[data-testid='explore-send']").disabled,
    null,
    { timeout: 5000 },
  );
  await page.click("[data-testid='explore-send']");
  await page.waitForFunction(
    (answer) => {
      const node = document.querySelector("[data-testid='explore-live-answer']");
      return !!node && node.textContent.includes(answer);
    },
    ANSWER,
    { timeout: 30000 },
  );
  await page.waitForFunction(
    () => document.querySelector("[data-testid='explore-stop']").hidden === true,
    null,
    { timeout: 15000 },
  );
  const sessionCall = requests.find(
    (request) =>
      request.method === "POST" &&
      request.url.includes(`/api/v1/agents/${agentId}/sessions`),
  );
  const chatCall = requests.find(
    (request) =>
      request.method === "POST" &&
      request.url.includes("/api/v1/agents/chat/completions"),
  );
  if (!sessionCall) fail("first send did not create the session");
  if (!chatCall) fail("first send did not post to /api/v1/agents/chat/completions");
  const chatBody = await page.evaluate(async (id) => {
    const listed = await fetch(`/api/v1/agents/${id}/sessions?page=1&page_size=100`).then(
      (res) => res.json(),
    );
    const row = (listed.data || [])[0] || {};
    return { id: row.id, name: row.name, messages: (row.message || []).length };
  }, agentId);
  results.firstSend = {
    sessionCall: sessionCall.url,
    chatCall: chatCall.url,
    url: page.url(),
    session: chatBody,
    transcript: await page.evaluate(() =>
      Array.from(document.querySelectorAll(".agent-explore-msg")).map((node) => ({
        role: node.className.includes("user") ? "user" : "assistant",
        content: node.querySelector(".agent-explore-msg-body").textContent,
      })),
    ),
  };
  if (!page.url().includes("sessionId=")) {
    fail("session id missing from the URL after the first send: " + page.url());
  }
  if (results.firstSend.session.name !== QUESTION) {
    fail("upstream names the session after the question: " + results.firstSend.session.name);
  }
  if (results.firstSend.session.messages !== 3) {
    fail(
      "the created session must carry the prologue plus the question and answer: " +
        results.firstSend.session.messages,
    );
  }
  step("first send", results.firstSend);
  await page.screenshot({ path: `${shots}/agent-explore-chat-${STAMP}.png` });

  // 5. Reload restores the transcript from the stored session.
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.querySelectorAll(".agent-explore-msg").length >= 3,
    null,
    { timeout: 15000 },
  );
  results.reload = await page.evaluate(() =>
    Array.from(document.querySelectorAll(".agent-explore-msg")).map((node) =>
      node.querySelector(".agent-explore-msg-body").textContent,
    ),
  );
  if (!results.reload.join(" ").includes(ANSWER)) {
    fail("reload lost the stored answer: " + JSON.stringify(results.reload));
  }
  step("reload transcript", results.reload);

  // 6. Session search filters the column and shows the upstream empty state.
  await page.fill("[data-testid='explore-session-search']", "no-such-session-xyz");
  await page.waitForSelector("[data-testid='explore-session-empty']", {
    timeout: 10000,
  });
  results.searchMiss = await page.evaluate(() => ({
    empty: document.querySelector("[data-testid='explore-session-empty']").textContent,
    cards: document.querySelectorAll("[data-testid='explore-session-card']").length,
  }));
  if (results.searchMiss.empty !== "No sessions found") {
    fail("search empty state is not upstream's: " + results.searchMiss.empty);
  }
  await page.fill("[data-testid='explore-session-search']", "");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='explore-session-card']").length > 0,
    null,
    { timeout: 10000 },
  );
  step("search filter", results.searchMiss);

  // 7. Hover reveals the ellipsis menu; its delete row opens the upstream
  //    confirm dialog and really deletes the session.
  await page.hover("[data-testid='explore-session-card']");
  const moreOpacity = await page.evaluate(
    () => getComputedStyle(document.querySelector("[data-testid='explore-session-more']")).opacity,
  );
  await page.click("[data-testid='explore-session-more']");
  await page.waitForSelector("[data-testid='explore-session-delete']", {
    timeout: 10000,
  });
  await page.click("[data-testid='explore-session-delete']");
  await page.waitForSelector("[data-testid='confirm-delete-dialog'].show", {
    timeout: 10000,
  });
  results.deleteDialog = await page.evaluate(() => ({
    title: document.getElementById("exploreDeleteTitle").textContent,
    cancel: document.querySelector("[data-testid='confirm-delete-dialog-cancel-btn']")
      .textContent,
    confirm: document.querySelector(
      "[data-testid='confirm-delete-dialog-confirm-btn']",
    ).textContent,
    moreOpacity: null,
  }));
  results.deleteDialog.moreOpacity = moreOpacity;
  if (results.deleteDialog.title !== "Are you sure to delete it ?") {
    fail("delete dialog title is not upstream's: " + results.deleteDialog.title);
  }
  await page.screenshot({ path: `${shots}/agent-explore-delete-${STAMP}.png` });
  await page.click("[data-testid='confirm-delete-dialog-confirm-btn']");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='explore-session-card']").length === 0,
    null,
    { timeout: 15000 },
  );
  const remaining = await page.evaluate(
    async (id) =>
      (await fetch(`/api/v1/agents/${id}/sessions?page=1&page_size=100`).then((res) =>
        res.json(),
      )).data.length,
    agentId,
  );
  results.delete = {
    ...results.deleteDialog,
    deleteCalls: requests.filter((request) => request.method === "DELETE").length,
    remainingSessions: remaining,
  };
  if (remaining !== 0) fail("session survived the confirm delete: " + remaining);
  step("delete session", results.delete);

  // 8. zh labels on the same surface.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(`${ORIGIN}/agent/${agentId}/explore`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='explore-new-session']", { timeout: 10000 });
  results.zh = await page.evaluate(() => ({
    sessions: document.querySelector(".agent-explore-sessions-title h2").textContent,
    count: !!document.querySelector("[data-testid='explore-session-count']"),
    search: document.querySelector("[data-testid='explore-session-search']").placeholder,
    placeholder: document.querySelector(".agent-explore-placeholder")?.textContent,
    breadcrumb: Array.from(
      document.querySelectorAll("[data-testid='explore-breadcrumb-agent'], [data-testid='explore-breadcrumb-title']"),
    ).map((node) => node.textContent.trim()),
    send: document.querySelector("[data-testid='explore-send']").getAttribute("aria-label"),
  }));
  for (const expected of ["会话列表", "搜索会话...", "请选择一个会话或创建新会话", "智能体"]) {
    if (!JSON.stringify(results.zh).includes(expected)) {
      fail(`zh label missing: ${expected} / ${JSON.stringify(results.zh)}`);
    }
  }
  await page.screenshot({ path: `${shots}/agent-explore-zh-${STAMP}.png` });
  step("zh surface", results.zh);

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  console.error("PROBE FAILED:", results.errors[results.errors.length - 1]);
} finally {
  if (agentId) {
    await page
      .evaluate(async (id) => {
        await fetch(`/api/v1/agents/${id}`, { method: "DELETE" });
      }, agentId)
      .catch(() => {});
  }
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
