// CDP probe for the SSH sandbox provider (`admin/sandbox-settings.tsx` +
// `src/ssh_exec.rs`, upstream `agent/sandbox/providers/ssh.py`): the console's
// "Test" button must execute a snippet on the remote host, not merely report that
// the port is open. The probe configures the SSH provider with a throwaway key
// against a temporary sshd on 127.0.0.1:2323, clicks through the real UI and
// asserts the returned stdout. It restores the previously active provider and
// clears the stored config afterwards, so the console is left as it was found.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... \
//        RAYRAG_SSH_PROBE_KEY=/tmp/rr-sshd/client \
//        RAYRAG_SSH_PROBE_PORT=2323 RAYRAG_SSH_PROBE_USER=... \
//        node scripts/ui-traversal/admin-sandbox-ssh-test-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync, readFileSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const KEY_PATH = process.env.RAYRAG_SSH_PROBE_KEY || "/tmp/rr-sshd/client";
const SSH_PORT = Number(process.env.RAYRAG_SSH_PROBE_PORT || 2323);
// Inside the container the probe sshd is reached through the docker bridge.
const SSH_HOST = process.env.RAYRAG_SSH_PROBE_HOST || "127.0.0.1";
const SSH_USER = process.env.RAYRAG_SSH_PROBE_USER;
if (!PASSWORD || !SSH_USER) {
  console.error("RAYRAG_LOGIN_PASSWORD and RAYRAG_SSH_PROBE_USER are required");
  process.exit(2);
}
const PRIVATE_KEY = readFileSync(KEY_PATH, "utf8");

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

const api = (path, options) =>
  page.evaluate(
    async ([target, request]) => {
      const token = localStorage.getItem("rayrag_token") || "";
      const res = await fetch(target, {
        ...request,
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer " + token,
          ...(request && request.headers ? request.headers : {}),
        },
      });
      return { status: res.status, body: await res.json().catch(() => ({})) };
    },
    [path, options || {}],
  );

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
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
  }, PASSWORD);

  // Remember what the console had selected so the probe can put it back.
  const before = await api("/api/v1/admin/sandbox/config");
  results.previousProvider = before.body?.data?.provider_type ?? null;

  await page.goto(ORIGIN + "/admin/sandbox-settings", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-sandbox-providers']", { timeout: 20000 });

  // Real click on the SSH provider card.
  const sshCard = page.locator("[data-testid='admin-sandbox-provider-ssh']");
  results.sshCard = {
    count: await sshCard.count(),
    label: (await sshCard.first().innerText().catch(() => "")).replace(/\s+/g, " ").trim(),
  };
  check(
    results.sshCard.count === 1 && /SSH/i.test(results.sshCard.label),
    "ssh_provider_card",
    JSON.stringify(results.sshCard),
  );
  await sshCard.first().click();
  await page.waitForSelector("#sbConfigCard:not([hidden])", { timeout: 15000 });
  await page.waitForTimeout(500);

  // The schema fields the SSH provider renders (upstream `get_config_schema`).
  // `sbFieldBlock` labels each control with `for=<field name>` and gives the input
  // that same id.
  results.fields = await page.locator("#sbFields label[for]").evaluateAll((nodes) =>
    nodes.map((node) => node.getAttribute("for")),
  );
  for (const field of ["host", "port", "username", "password", "work_dir", "timeout"]) {
    check(results.fields.includes(field), `field_${field}`, results.fields.join(","));
  }
  // The SSH provider offers Password / Private Key tabs (upstream's two
  // authentication methods); only the selected panel renders.
  await page.click("[data-testid='admin-sandbox-auth-private_key']");
  await page.waitForSelector("#sbFields #private_key", { timeout: 10000 });
  results.authMode = await page.evaluate(() => ({
    selected: document
      .querySelector("[data-testid='admin-sandbox-auth-private_key']")
      .getAttribute("aria-selected"),
    passwordTab: document
      .querySelector("[data-testid='admin-sandbox-auth-password']")
      .getAttribute("aria-selected"),
    keyField: !!document.getElementById("private_key"),
    passwordField: !!document.getElementById("password"),
    passphrase: !!document.getElementById("passphrase"),
  }));
  check(
    results.authMode.selected === "true" &&
      results.authMode.passwordTab === "false" &&
      results.authMode.keyField === true &&
      results.authMode.passwordField === false &&
      results.authMode.passphrase === true,
    "auth_tab_switches",
    JSON.stringify(results.authMode),
  );

  const fill = async (field, value) => {
    await page.locator(`#sbFields #${field}`).first().fill(String(value));
  };
  await fill("host", SSH_HOST);
  await fill("port", SSH_PORT);
  await fill("username", SSH_USER);
  await fill("private_key", PRIVATE_KEY);
  await fill("work_dir", "/tmp");
  await fill("timeout", 20);
  // The key is a secret field; make sure the value really landed.
  results.keyLength = await page
    .locator("#sbFields #private_key")
    .first()
    .inputValue()
    .then((value) => value.length);
  check(results.keyLength === PRIVATE_KEY.length, "private_key_entered", String(results.keyLength));

  // Real click on Test: this must run python on the remote host.
  await page.click("[data-testid='admin-sandbox-test']");
  await page.waitForSelector("[data-testid='admin-sandbox-test-modal']", { timeout: 45000 });
  // The first SSH connection can take a while (key exchange, interpreter start), so
  // the result is polled instead of assumed.
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const text = await page.locator("[data-testid='admin-sandbox-test-modal']").innerText();
    if (!/Testing/i.test(text)) break;
    await page.waitForTimeout(1000);
  }
  results.testModal = await page.locator("[data-testid='admin-sandbox-test-modal']").innerText();
  check(
    /PASSED/i.test(results.testModal),
    "test_reports_passed",
    results.testModal.slice(0, 240).replace(/\n/g, " | "),
  );
  check(
    results.testModal.includes("rayrag-ssh-ok"),
    "remote_stdout_shown",
    results.testModal.slice(0, 240).replace(/\n/g, " | "),
  );
  check(
    !/not implemented in this build/i.test(results.testModal),
    "no_placeholder_message",
  );

  // The same call through the API, so the payload shape is on record too.
  const direct = await api("/api/v1/admin/sandbox/test", {
    method: "POST",
    body: JSON.stringify({
      provider_type: "ssh",
      config: {
        host: SSH_HOST,
        port: SSH_PORT,
        username: SSH_USER,
        private_key: PRIVATE_KEY,
        work_dir: "/tmp",
      },
    }),
  });
  results.apiCall = {
    success: direct.body?.data?.success,
    message: direct.body?.data?.message,
    stdout: direct.body?.data?.details?.stdout,
    exitCode: direct.body?.data?.details?.exit_code,
  };
  check(
    direct.body?.data?.success === true &&
      direct.body?.data?.details?.exit_code === 0 &&
      (direct.body?.data?.details?.stdout || "").includes("rayrag-ssh-ok"),
    "api_runs_code",
    JSON.stringify(results.apiCall),
  );

  // A missing credential must fail loudly rather than silently "passing".
  const missing = await api("/api/v1/admin/sandbox/test", {
    method: "POST",
    body: JSON.stringify({
      provider_type: "ssh",
      config: { host: SSH_HOST, port: SSH_PORT, username: SSH_USER },
    }),
  });
  // Schema errors come back in the envelope itself (`{code, message}`), while a
  // provider failure carries the payload in `data`.
  results.missingCredential = missing.body?.message ?? missing.body?.data?.message ?? missing.body?.data?.details?.stderr;
  check(
    typeof results.missingCredential === "string" &&
      /password or private_key/.test(results.missingCredential),
    "missing_credential_rejected",
    String(results.missingCredential),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-sandbox-ssh-failure.png" }).catch(() => {});
} finally {
  // Put the console back: clear the SSH config and re-select what was active.
  try {
    const restore = await page.evaluate(
      async ([provider, keys]) => {
        const token = localStorage.getItem("rayrag_token") || "";
        const headers = { "Content-Type": "application/json", Authorization: "Bearer " + token };
        const cleared = {};
        for (const key of keys) cleared[key] = "";
        await fetch("/api/v1/admin/sandbox/config", {
          method: "POST",
          headers,
          body: JSON.stringify({ provider_type: "ssh", config: cleared }),
        });
        if (provider && provider !== "ssh") {
          await fetch("/api/v1/admin/sandbox/config", {
            method: "POST",
            headers,
            body: JSON.stringify({ provider_type: provider }),
          });
        }
        return true;
      },
      [
        results.previousProvider,
        ["host", "port", "username", "password", "private_key", "passphrase", "known_hosts", "work_dir"],
      ],
    );
    results.restored = restore ? "ok" : "FAIL";
  } catch {
    results.restored = "FAIL";
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
