// CDP probe for the floating chat widget
// (`components/floating-chat-widget.tsx` + `Routes.ChatWidget`): it seeds a
// throwaway agent, creates an embed key, then walks the widget in a real browser:
// the bubble opens the panel, the title/subtitle/accent colour come from the URL,
// a question streams back an answer, the unread badge and minimize/close behave,
// and the `mode=master` + `mode=window` pair talks over postMessage.
//
// The master/window leg embeds the widget the way a customer site does: the host
// page is served over HTTP from its own origin (a second port = a second origin),
// because a document that only ever lived on `about:blank` has an opaque origin
// where Chrome denies `localStorage` and cookies outright. The sandboxed-frame
// leg then covers exactly that hostile case on purpose: `sandbox="allow-scripts"`
// strips the frame's storage, so the widget must still finish its boot and hand
// the host its `CREATE_CHAT_WINDOW` message.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/chat-widget-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";
import { createServer } from "node:http";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error(
    "RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)",
  );
  process.exit(2);
}
const STAMP = Date.now();
const AGENT_TITLE = `cdp-widget-probe-${STAMP}`;
const QUESTION = `widget probe question ${STAMP}`;
const ANSWER = "widget probe answer";
const ACCENT = "#16a34a";

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
let beta = null;
let token = null;
try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const seeded = await page.evaluate(
    async ({ password, title }) => {
      const login = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      }).then((res) => res.json());
      const token = login.data.access_token;
      localStorage.setItem("rayrag_token", token);
      document.cookie = `rayrag_token=${token};path=/`;
      const created = await fetch("/api/v1/agents", {
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
                  prologue: "Hi from the widget!",
                },
                upstream: [],
                downstream: ["message"],
              },
              message: {
                obj: {
                  component_name: "Message",
                  params: { content: ["widget probe answer"] },
                },
                upstream: ["begin"],
                downstream: [],
              },
            },
          },
        }),
      }).then((res) => res.json());
      const created_token = await fetch("/api/v1/system/tokens", {
        method: "POST",
      }).then((res) => res.json());
      return {
        agentId: created.data.id,
        beta: created_token.data.beta,
        token,
      };
    },
    { password: PASSWORD, title: AGENT_TITLE },
  );
  agentId = seeded.agentId;
  beta = seeded.beta;
  token = seeded.token;
  step("seeded agent", { agentId });

  // 1. Full mode: the bubble opens the panel, settings come from the URL.
  const base = (mode) =>
    `${ORIGIN}/chats/widget?shared_id=${agentId}&from=agent&auth=${beta}&mode=${mode}` +
    `&streaming=true&widget_title=Support&widget_subtitle=Ask us anything&widget_footer=Powered by RayRAG` +
    `&widget_footer_link=${encodeURIComponent("https://example.com")}&widget_accent_color=${encodeURIComponent(ACCENT)}`;
  await page.goto(base("full"), { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.waitForFunction(() => typeof fcwToggle === "function", null, {
    timeout: 15000,
  });
  results.boot = await page.evaluate(() => {
    const root = document.getElementById("fcwRoot");
    return {
      mode: root.dataset.mode,
      accent: getComputedStyle(root).getPropertyValue("--fcw-accent").trim(),
      buttonVisible: !document.getElementById("fcwButtonWrap").hidden,
      windowHidden: document.getElementById("fcwWindow").hidden,
      title: document.getElementById("fcwTitle").textContent,
      subtitle: document.getElementById("fcwSubtitle").textContent,
      footer: document.getElementById("fcwFooter").textContent,
      footerLink: document.querySelector("#fcwFooter a")?.getAttribute("href") || "",
      url: location.search.includes("auth=") ? "auth-not-stripped" : "auth-stripped",
    };
  });
  if (results.boot.mode !== "full") fail("mode was not applied: " + results.boot.mode);
  if (results.boot.accent !== ACCENT) {
    fail("widget accent colour was not applied: " + results.boot.accent);
  }
  if (!results.boot.buttonVisible || !results.boot.windowHidden) {
    fail("full mode should start with the bubble only: " + JSON.stringify(results.boot));
  }
  if (results.boot.title !== "Support" || results.boot.subtitle !== "Ask us anything") {
    fail("widget title/subtitle missing: " + JSON.stringify(results.boot));
  }
  if (results.boot.footer !== "Powered by RayRAG" || !results.boot.footerLink) {
    fail("widget footer missing: " + JSON.stringify(results.boot));
  }
  if (results.boot.url !== "auth-stripped") fail("the auth parameter was not stripped");
  step("full mode boot", results.boot);
  await page.screenshot({ path: `${shots}/chat-widget-bubble-${STAMP}.png` });

  // 2. Real click on the bubble → panel with the prologue, then send a question.
  await page.click("[data-testid='fcw-button']");
  await page.waitForFunction(() => !document.getElementById("fcwWindow").hidden, null, {
    timeout: 10000,
  });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='fcw-message']").length > 0,
    null,
    { timeout: 15000 },
  );
  await page.fill("[data-testid='fcw-input']", QUESTION);
  await page.waitForFunction(() => !document.getElementById("fcwSend").disabled, null, {
    timeout: 5000,
  });
  await page.click("[data-testid='fcw-send']");
  await page.waitForFunction(
    (answer) =>
      Array.from(document.querySelectorAll(".fcw-message-body")).some((node) =>
        node.textContent.includes(answer),
      ),
    ANSWER,
    { timeout: 30000 },
  );
  results.chat = await page.evaluate(() => ({
    messages: Array.from(document.querySelectorAll("[data-testid='fcw-message']")).map(
      (node) => ({
        role: node.className.includes("user") ? "user" : "assistant",
        content: node.querySelector(".fcw-message-body").textContent,
      }),
    ),
    badgeHidden: document.getElementById("fcwBadge").hidden,
  }));
  step("widget chat", results.chat);
  await page.screenshot({ path: `${shots}/chat-widget-chat-${STAMP}.png` });

  // 3. Minimize hides the panel and the bubble comes back with its icon.
  await page.click("[data-testid='fcw-minimize']");
  await page.waitForFunction(() => document.getElementById("fcwWindow").hidden, null, {
    timeout: 10000,
  });
  results.minimize = await page.evaluate(() => ({
    windowHidden: document.getElementById("fcwWindow").hidden,
    iconHasChat: document.getElementById("fcwButtonIcon").innerHTML.includes("M7.9 20A9"),
  }));
  if (!results.minimize.iconHasChat) fail("the bubble icon did not reset");
  step("minimize", results.minimize);

  // 4. master + window modes talk over postMessage: a host page served from its
  //    own origin embeds the master iframe, creates the window iframe on
  //    CREATE_CHAT_WINDOW and toggles it.
  const windowUrl = base("window");
  const hostHtml = `<!doctype html><html><body style="margin:0">
       <iframe id="master" src="${base("master")}" style="width:200px;height:200px;border:0"></iframe>
       <iframe id="sandboxed" sandbox="allow-scripts" src="${base("master")}" style="width:200px;height:200px;border:0"></iframe>
       <script>
         window.__created = null;
         window.__sandboxCreated = null;
         window.__reused = 0;
         window.__messages = [];
         window.addEventListener('message', (event) => {
           window.__messages.push(event.data && event.data.type);
           if (event.data.type === 'CREATE_CHAT_WINDOW') {
             try { window.__hostStorage = String(!!window.localStorage); } catch (error) { window.__hostStorage = 'throw: ' + error.message; }
             if (event.source === document.getElementById('sandboxed').contentWindow) {
               window.__sandboxCreated = event.data.src;
               return;
             }
             if (document.getElementById('chat-win')) { window.__reused += 1; return; }
             const frame = document.createElement('iframe');
             frame.id = 'chat-win';
             frame.src = event.data.src;
             frame.style.cssText = 'position:fixed;bottom:104px;right:24px;width:380px;height:500px;border:0;display:none';
             document.body.appendChild(frame);
             window.__created = event.data.src;
           } else if (event.data.type === 'TOGGLE_CHAT') {
             const frame = document.getElementById('chat-win');
             if (frame) frame.style.display = event.data.isOpen ? 'block' : 'none';
           }
         });
       </script></body></html>`;
  const hostServer = createServer((request, response) => {
    response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    response.end(hostHtml);
  });
  await new Promise((resolve) => hostServer.listen(0, "127.0.0.1", resolve));
  const hostOrigin = `http://127.0.0.1:${hostServer.address().port}`;
  const host = await ctx.newPage();
  const hostErrors = [];
  host.on("pageerror", (error) => hostErrors.push(String(error.message)));
  try {
    await host.goto(hostOrigin + "/", { waitUntil: "domcontentloaded", timeout: 30000 });
    results.hostOrigin = { origin: hostOrigin, widgetOrigin: ORIGIN, crossOrigin: true };
    if (hostOrigin === ORIGIN) fail("the host page must be served from another origin");
    await host.waitForFunction(() => !!window.__created, null, { timeout: 20000 });
    // The sandboxed frame boots cold (no storage, a CORS-blocked info fetch), so
    // give its handshake its own wait instead of racing the plain frame's.
    await host.waitForFunction(() => !!window.__sandboxCreated, null, { timeout: 20000 });
    results.master = await host.evaluate(() => ({
      created: window.__created,
      hasWindowIframe: !!document.getElementById("chat-win"),
      display: document.getElementById("chat-win")?.style.display || "",
      hostStorage: window.__hostStorage,
      sandboxedCreated: window.__sandboxCreated,
      messages: window.__messages,
    }));
    if (!results.master.created.includes("mode=window")) {
      fail("CREATE_CHAT_WINDOW carried the wrong URL: " + JSON.stringify(results.master));
    }
    // A sandboxed frame has no storage at all; the boot must survive it.
    if (!results.master.sandboxedCreated) {
      fail("the sandboxed (storage-less) frame never booted: " + JSON.stringify(results.master));
    }
    step("cross-origin host handshake", results.master);
    // Real click on the master bubble → the host page shows the window iframe.
    const master = host.frames().find((frame) => frame.url().includes("mode=master"));
    await master.waitForFunction(() => typeof fcwToggle === "function", null, { timeout: 15000 });
    await master.click("[data-testid='fcw-button']");
    await host.waitForFunction(
      () => document.getElementById("chat-win")?.style.display === "block",
      null,
      { timeout: 15000 },
    );
    results.masterToggle = await host.evaluate(() => ({
      display: document.getElementById("chat-win").style.display,
      windowUrl: window.location.origin + "/chats/widget",
    }));
    const windowFrame = host.frames().find((frame) => frame.url().includes("mode=window"));
    await windowFrame.waitForFunction(
      () => !document.getElementById("fcwWindow").hidden,
      null,
      { timeout: 15000 },
    );
    results.windowMode = await windowFrame.evaluate(() => ({
      mode: document.getElementById("fcwRoot").dataset.mode,
      windowHidden: document.getElementById("fcwWindow").hidden,
      bubblePresent: !!document.getElementById("fcwButtonWrap"),
    }));
    if (results.windowMode.windowHidden) fail("window mode hid its panel");
    if (results.windowMode.mode !== "window") fail("window mode flag missing");
    await host.screenshot({ path: `${shots}/chat-widget-master-${STAMP}.png` });
    step("master/window", { ...results.master, ...results.masterToggle, ...results.windowMode });
    if (hostErrors.length) fail("host page errors: " + JSON.stringify(hostErrors));
  } finally {
    await host.close().catch(() => {});
    await new Promise((resolve) => hostServer.close(resolve));
  }

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  results.diagnostics = await page
    .evaluate(() => ({
      url: location.href,
      hasRoot: !!document.getElementById("fcwRoot"),
      messages: document.querySelectorAll("[data-testid='fcw-message']").length,
      title: document.getElementById("fcwTitle")?.textContent,
    }))
    .catch((diagnosticError) => ({ diagnosticError: String(diagnosticError.message) }));
  console.error("PROBE FAILED:", results.errors[results.errors.length - 1]);
} finally {
  if (agentId && token) {
    await page
      .evaluate(
        async ({ id, value }) => {
          await fetch(`/api/v1/agents/${id}`, { method: "DELETE" });
          const listed = await fetch("/api/v1/system/tokens").then((res) => res.json());
          for (const row of listed.data || []) {
            if (row.beta === value) {
              await fetch(`/api/v1/system/tokens/${row.token}`, { method: "DELETE" });
            }
          }
        },
        { id: agentId, value: beta },
      )
      .catch(() => {});
  }
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
