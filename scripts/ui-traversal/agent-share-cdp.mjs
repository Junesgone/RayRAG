// CDP probe for the shared/embedded agent surface
// (`components/embed-dialog/index.tsx` + `pages/agent/share/index.tsx`):
// it seeds a throwaway conversational canvas, opens the editor's
// `Embed into webpage` dialog with a real click, asserts the upstream token
// gate, really creates a tenant API key, walks the embed switches and the
// generated iframe snippet, then opens the share URL and chats through the
// beta-authenticated `agentbots` endpoints — including the Begin `Parameter`
// dialog and `Reset` — and finally revokes the key and deletes the probe canvas.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/agent-share-cdp.mjs
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
const AGENT_TITLE = `cdp-agent-share-probe-${STAMP}`;
const QUESTION = `share probe question ${STAMP}`;
const ANSWER = "embedded answer";
const PROLOGUE = "Hello from the embed";

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
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
let shareUrl = null;
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
    document.cookie = `rayrag_token=${data.data.access_token};path=/`;
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) fail("login failed: " + JSON.stringify(login.body));
  step("login", "ok");

  // Seed a conversational canvas whose Begin → Message path answers without an
  // LLM, with a prologue and one Begin input so the Parameter dialog shows.
  const created = await page.evaluate(async (title) => {
    const response = await fetch("/api/v1/agents", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        title,
        dsl: {
          graph: { nodes: [{ id: "begin", type: "beginNode" }], edges: [] },
          components: {
            begin: {
              obj: {
                component_name: "Begin",
                params: {},
                mode: "conversational",
                prologue: "Hello from the embed",
                input_form: { query: { type: "line", name: "Query" } },
              },
              upstream: [],
              downstream: ["message"],
            },
            message: {
              obj: {
                component_name: "Message",
                params: { content: ["embedded answer"] },
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

  // 1. Real click from the editor toolbar into the embed dialog.
  await page.goto(`${ORIGIN}/agent/${agentId}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='agent-embed']", { timeout: 10000 });
  await page.click("[data-testid='agent-embed']");
  await page.waitForSelector("[data-testid='agent-embed-modal'].show", {
    timeout: 10000,
  });
  await page.waitForFunction(
    () => typeof embedSync === "function",
    null,
    { timeout: 10000 },
  );
  await page.waitForSelector("[data-testid='embed-create-token']", {
    timeout: 10000,
  });
  results.gate = await page.evaluate(() => ({
    title: document.getElementById("embedModalTitle").textContent,
    tokenError: document.getElementById("embedTokenError").hidden
      ? null
      : document.getElementById("embedTokenError").textContent,
    createDisabled: document.getElementById("embedCreateToken").disabled,
    rows: document.querySelectorAll("[data-testid='embed-token-row']").length,
    embedTypeOptions: Array.from(document.querySelectorAll("#embedType option")).map(
      (node) => ({ text: node.textContent, disabled: node.disabled }),
    ),
    locales: document.querySelectorAll("#embedLocale option").length,
  }));
  if (results.gate.title !== "Embed into webpage") {
    fail("embed dialog title is not upstream's: " + results.gate.title);
  }
  // The tenant may already own an API key (the operator's own, or a leftover
  // from an interrupted probe): reuse it instead of revoking somebody's key.
  const reused = results.gate.rows > 0;
  if (!reused) {
    if (results.gate.tokenError !== "Please create API key first.") {
      fail("token gate missing: " + JSON.stringify(results.gate));
    }
    if (results.gate.createDisabled) fail("Create new key must be enabled");
  } else {
    step("existing api key reused", results.gate.rows);
  }
  step("embed gate", { ...results.gate, reused });
  await page.screenshot({ path: `${shots}/agent-embed-gate-${STAMP}.png` });

  // 2. Real click on `Create new key` → one token row, error gone.
  if (!reused) {
    await page.click("[data-testid='embed-create-token']");
  }
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='embed-token-row']").length === 1,
    null,
    { timeout: 15000 },
  );
  results.token = await page.evaluate(() => {
    const row = document.querySelector("[data-testid='embed-token-row']");
    return {
      token: row.querySelector(".agent-embed-token").textContent,
      created: row.children[1].textContent,
      actions: row.querySelectorAll("[data-testid^='embed-token-']").length,
      errorHidden: document.getElementById("embedTokenError").hidden,
      createDisabled: document.getElementById("embedCreateToken").disabled,
      snippet: document.getElementById("embedSnippet").textContent,
    };
  });
  if (!results.token.token.startsWith("ragflow-")) {
    fail("token is not upstream-shaped: " + results.token.token);
  }
  if (!results.token.errorHidden || !results.token.createDisabled) {
    fail("token gate did not clear after creating a key");
  }
  const authMatch = /auth=([0-9a-f]{32})/.exec(results.token.snippet);
  if (!authMatch) fail("snippet carries no beta secret: " + results.token.snippet);
  if (!results.token.snippet.includes(`shared_id=${agentId}`)) {
    fail("snippet does not address the canvas: " + results.token.snippet);
  }
  if (!results.token.snippet.includes("from=agent")) {
    fail("snippet is missing from=agent: " + results.token.snippet);
  }
  if (!results.token.snippet.includes("min-height: 600px")) {
    fail("snippet is not the upstream fullscreen branch: " + results.token.snippet);
  }
  step("api key", { ...results.token, token: results.token.token.slice(0, 16) + "…" });

  // 3. The switches rewrite the generated URL exactly like `generateIframeSrc`.
  await page.click("[data-testid='embed-published']");
  await page.click("[data-testid='embed-hide-avatar']");
  await page.click("[data-testid='embed-theme-dark']");
  await page.selectOption("[data-testid='embed-locale']", "zh-Hans");
  await page.waitForFunction(
    () =>
      document.getElementById("embedSnippet").textContent.includes("release=true") &&
      document.getElementById("embedSnippet").textContent.includes("visible_avatar=1") &&
      document.getElementById("embedSnippet").textContent.includes("theme=dark") &&
      document.getElementById("embedSnippet").textContent.includes("locale=zh-Hans"),
    null,
    { timeout: 10000 },
  );
  results.switches = await page.evaluate(() =>
    document.getElementById("embedSnippet").textContent,
  );
  step("switches", results.switches.split("\n")[1].trim());
  await page.screenshot({ path: `${shots}/agent-embed-snippet-${STAMP}.png` });

  // 4. Open the generated embed URL: the share surface boots with the beta
  //    credential and the auth parameter disappears from the address bar.
  shareUrl = /src="([^"]+)"/.exec(results.switches)[1];
  await page.goto(shareUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.waitForFunction(
    () => typeof shareSendMessage === "function",
    null,
    { timeout: 15000 },
  );
  await page.waitForFunction(
    () => document.getElementById("shareTitle").textContent.length > 0,
    null,
    { timeout: 15000 },
  );
  results.share = await page.evaluate((beta) => ({
    url: location.href,
    title: document.getElementById("shareTitle").textContent,
    brand: !!document.querySelector(".agent-share-brand"),
    reset: document.querySelector("[data-testid='share-reset']").textContent,
    cookie: document.cookie.includes("rayrag_token=" + beta),
    messages: Array.from(document.querySelectorAll(".agent-share-msg")).map((node) =>
      node.querySelector(".agent-share-msg-body").textContent,
    ),
    noAvatar: document
      .querySelector(".agent-share-msg")
      .classList.contains("no-avatar"),
    parameterVisible: document
      .getElementById("shareParameterDialog")
      .classList.contains("show"),
  }), authMatch[1]);
  if (results.share.title !== AGENT_TITLE) {
    fail("share page did not load the canvas title: " + JSON.stringify(results.share));
  }
  if (results.share.url.includes("auth=")) {
    fail("the auth parameter must be stripped after boot: " + results.share.url);
  }
  if (!results.share.cookie) fail("the beta credential was not stored");
  if (!results.share.messages.includes(PROLOGUE)) {
    fail("prologue missing from the embed: " + JSON.stringify(results.share.messages));
  }
  if (!results.share.noAvatar) {
    fail("visible_avatar=1 must hide the message avatars");
  }
  if (!results.share.parameterVisible) {
    fail("the Begin Parameter dialog did not open");
  }
  step("share boot", { ...results.share, url: results.share.url.slice(0, 90) });

  // 5. The Parameter dialog collects the Begin inputs, then the chat streams.
  await page.fill("[data-share-input='query'] input", "probe input");
  await page.click("[data-testid='share-parameter-submit']");
  await page.waitForFunction(
    () => !document.getElementById("shareParameterDialog").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  await page.fill("[data-testid='share-input']", QUESTION);
  await page.waitForFunction(
    () => !document.querySelector("[data-testid='share-send']").disabled,
    null,
    { timeout: 5000 },
  );
  await page.click("[data-testid='share-send']");
  await page.waitForFunction(
    (answer) =>
      Array.from(document.querySelectorAll(".agent-share-msg-body")).some((node) =>
        node.textContent.includes(answer),
      ),
    ANSWER,
    { timeout: 30000 },
  );
  results.chat = await page.evaluate(() => ({
    messages: Array.from(document.querySelectorAll(".agent-share-msg")).map((node) => ({
      role: node.className.includes("user") ? "user" : "assistant",
      content: node.querySelector(".agent-share-msg-body").textContent,
    })),
    sendHidden: document.querySelector("[data-testid='share-send']").hidden,
    stopHidden: document.querySelector("[data-testid='share-stop']").hidden,
    status: document.querySelector("[data-testid='share-status']").textContent,
    theme: document.body.classList.contains("light") ? "light" : "dark",
  }));
  step("shared chat", results.chat);
  await page.screenshot({ path: `${shots}/agent-share-chat-${STAMP}.png` });

  // 6. `Reset` clears the thread and restores the prologue (upstream
  //    `resetSession()` + `clearEventList()` + parameter dialog).
  await page.click("[data-testid='share-reset']");
  await page.waitForFunction(
    (prologue) => {
      const nodes = Array.from(document.querySelectorAll(".agent-share-msg-body"));
      return nodes.length === 1 && nodes[0].textContent === prologue;
    },
    PROLOGUE,
    { timeout: 10000 },
  );
  results.reset = await page.evaluate(() => ({
    messages: Array.from(document.querySelectorAll(".agent-share-msg-body")).map(
      (node) => node.textContent,
    ),
    parameterVisible: document
      .getElementById("shareParameterDialog")
      .classList.contains("show"),
  }));
  step("reset", results.reset);
  await page.screenshot({ path: `${shots}/agent-share-reset-${STAMP}.png` });

  // 7. The API key table really revokes through the upstream endpoint — run
  //    only for the key this probe created, so an operator's own key survives.
  if (!reused) {
    await page.goto(`${ORIGIN}/agent/${agentId}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    await page.click("[data-testid='agent-embed']");
    await page.waitForFunction(
      () => document.querySelectorAll("[data-testid='embed-token-row']").length === 1,
      null,
      { timeout: 15000 },
    );
    await page.click("[data-testid='embed-token-delete']");
    await page.waitForFunction(
      () => document.querySelectorAll("[data-testid='embed-token-row']").length === 0,
      null,
      { timeout: 15000 },
    );
    results.revoke = await page.evaluate(() => ({
      rows: document.querySelectorAll("[data-testid='embed-token-row']").length,
      errorHidden: document.getElementById("embedTokenError").hidden,
    }));
    if (results.revoke.errorHidden) {
      fail("revoking the only key must restore the upstream token gate");
    }
    step("revoke", results.revoke);
  }

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  results.diagnostics = await page
    .evaluate(() => ({
      url: location.href,
      title: document.title,
      markers: {
        embedModal: !!document.getElementById("embedModal"),
        tokenRows: document.querySelectorAll("[data-testid='embed-token-row']").length,
        shareMessages: document.querySelectorAll(".agent-share-msg").length,
      },
    }))
    .catch((diagnosticError) => ({ diagnosticError: String(diagnosticError.message) }));
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
