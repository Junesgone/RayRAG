// CDP probe for the token-exchange half of the Box / Gmail token fields
// (web/src/pages/user-setting/data-source/component/{box,gmail}-token-field.tsx):
// start -> popup callback -> result, with the token endpoints pointed at the
// deterministic mock server, then create the connector with the minted tokens.
// It also re-reads the mock's request log to prove both exchanges happened.
import { chromium } from "playwright-core";
import { readFileSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
const OAUTH_LOG = process.env.RAYRAG_MOCK_OAUTH_LOG || "/tmp/mock-oauth.log";
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
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const loggedIn = await page.evaluate(
    async ({ email, password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { email: EMAIL, password: PASSWORD },
  );
  if (!loggedIn) throw new Error("login failed");
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);

  out.box = await page.evaluate(async (redirect) => {
    const start = await fetch("/api/v1/connectors/box/oauth/web/start", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        client_id: "box-client",
        client_secret: "box-secret",
        redirect_uri: redirect,
      }),
    }).then((response) => response.json());
    const flowId = start.data.flow_id;
    const callback = await fetch(
      "/api/v1/connectors/box/oauth/web/callback?code=mock-box-code&state=" +
        encodeURIComponent(flowId),
    ).then((response) => response.text());
    const result = await fetch("/api/v1/connectors/box/oauth/web/result", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ flow_id: flowId }),
    }).then((response) => response.json());
    return {
      started: start.code === 0 && String(start.data.authorization_url).includes("account.box.com"),
      callbackCompleted: callback.includes("Authorization complete"),
      resultCode: result.code,
      credentials: result.data && result.data.credentials,
    };
  }, ORIGIN + "/api/v1/connectors/box/oauth/web/callback");

  out.gmail = await page.evaluate(async (redirect) => {
    const credentials = JSON.stringify({
      web: { client_id: "google-client", client_secret: "google-secret" },
    });
    const start = await fetch(
      "/api/v1/connectors/google/oauth/web/start?type=gmail",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ credentials, redirect_uri: redirect }),
      },
    ).then((response) => response.json());
    const flowId = start.data.flow_id;
    const callback = await fetch(
      "/api/v1/connectors/gmail/oauth/web/callback?code=mock-google-code&state=" +
        encodeURIComponent(flowId),
    ).then((response) => response.text());
    const result = await fetch(
      "/api/v1/connectors/google/oauth/web/result?type=gmail",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ flow_id: flowId }),
      },
    ).then((response) => response.json());
    return {
      started: start.code === 0 && String(start.data.authorization_url).includes("accounts.google.com"),
      callbackCompleted: callback.includes("Authorization complete"),
      resultCode: result.code,
      credentials: result.data && result.data.credentials,
    };
  }, ORIGIN + "/api/v1/connectors/gmail/oauth/web/callback");

  // Create the Gmail connector with the minted tokens and verify the record.
  out.connector = await page.evaluate(async (credentials) => {
    const response = await fetch("/api/v1/data_sources", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "gmail-oauth-probe", type: "gmail", config: credentials }),
    }).then((next) => next.json());
    if (response.code !== 0) return { code: response.code, message: response.message };
    const list = await fetch("/api/v1/data_sources").then((next) => next.json());
    const row = (list.data || []).find((item) => item.id === response.data?.id);
    return { code: response.code, id: response.data?.id, rowType: row?.type };
  }, out.gmail.credentials);

  const log = readFileSync(OAUTH_LOG, "utf8").trim().split("\n");
  out.exchanges = log.filter((line) =>
    line.startsWith("/box/token") || line.startsWith("/google/token"),
  );
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 400);
} finally {
  out.pageErrors = pageErrors;
  console.log(JSON.stringify(out, null, 1));
  await page.close().catch(() => {});
  await b.close().catch(() => {});
  process.exit(out.error || pageErrors.length ? 1 : 0);
}
