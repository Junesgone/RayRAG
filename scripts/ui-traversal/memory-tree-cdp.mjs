import { chromium } from "playwright-core";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required");
  process.exit(2);
}

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://[::1]:16002";
const b = await chromium.connectOverCDP(CDP_EP);
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 300)));
const apiCalls = [];
page.on("request", (req) => {
  const u = req.url();
  if (u.includes("/api/v1/") && !u.includes("/auth/login")) {
    apiCalls.push({
      method: req.method(),
      path: u.split(":9390")[1] || u,
      body: (req.postData() || "").slice(0, 220),
    });
  }
});
const results = { steps: [], errors: [] };
let probeMemId = null;

async function api(method, path, body) {
  return await page.evaluate(async ({ method, path, body }) => {
    const res = await fetch(path, {
      method,
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let data = null;
    try {
      data = JSON.parse(text);
    } catch {}
    return { status: res.status, data };
  }, { method, path, body });
}

async function deleteMemory(id) {
  try {
    return await api("DELETE", "/api/v1/memories/" + id);
  } catch (e) {
    return { error: String(e && e.message ? e.message : e).slice(0, 160) };
  }
}

try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  const listed = await api("GET", "/api/v1/memories?page=1&page_size=100");
  results.staleCleaned = [];
  if (listed.data && listed.data.code === 0) {
    const items = (listed.data.data && listed.data.data.memory_list) || [];
    for (const item of items) {
      if (typeof item.name === "string" && item.name.indexOf("CDP-MEM-") === 0) {
        await deleteMemory(item.id);
        results.staleCleaned.push(item.name);
      }
    }
  }

  const created = await api("POST", "/api/v1/memories", {
    name: "CDP-MEM-" + Date.now(),
    description: "cdp memory selector probe",
    memory_type: ["raw"],
    embd_id: "default",
    llm_id: "default",
  });
  results.created = created;
  const createdBody = created.data || {};
  const memId = (createdBody.data && createdBody.data.id) || createdBody.id;
  if (created.data?.code !== 0 || !memId) throw new Error("memory create failed");
  probeMemId = memId;

  await page.goto(ORIGIN + "/memory/memory-setting/" + encodeURIComponent(memId), {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(2500);
  const shell = await page.evaluate(() => {
    const embd = document.getElementById("cfgEmbd");
    const llm = document.getElementById("cfgLlm");
    return {
      modelFields: typeof MODEL_FIELDS === "undefined" ? [] : MODEL_FIELDS.map((f) => f.fieldId + ":" + f.acceptedTypes.join(",")),
      catalogLen: typeof DEFAULT_MODEL_CATALOG === "undefined" ? -1 : DEFAULT_MODEL_CATALOG.length,
      embdDisabled: embd ? embd.disabled : null,
      llmDisabled: llm ? llm.disabled : null,
      embdTestId: embd ? embd.getAttribute("data-testid") : null,
      llmTestId: llm ? llm.getAttribute("data-testid") : null,
    };
  });
  results.shell = shell;

  const opened = await page.evaluate(() => {
    const t = document.getElementById("cfgLlm");
    if (t) t.click();
    return {
      expanded: t ? t.getAttribute("aria-expanded") : null,
      rows: document.querySelectorAll("#cfgLlmTree .default-model-node").length,
      firstRows: Array.from(document.querySelectorAll("#cfgLlmTree .default-model-node")).slice(0, 4).map((n) => n.textContent),
    };
  });
  await page.waitForTimeout(500);
  results.opened = opened;

  let picked = "";
  if (opened.rows > 0) {
    await page.evaluate(() => {
      const leaf = document.querySelector("#cfgLlmTree button.default-model-leaf");
      if (leaf) leaf.click();
    });
    await page.waitForTimeout(600);
    picked = await page.evaluate(() => {
      const trigger = document.getElementById("cfgLlm");
      return trigger.dataset.value || "";
    });
  }
  results.picked = picked;
  if (!picked) throw new Error("llm leaf pick failed");

  const defaultPatchCalls = apiCalls.filter((c) => c.path.indexOf("/api/v1/models/default") >= 0);
  results.defaultPatchCalls = defaultPatchCalls.length;

  await page.locator("#cfgSave").click({ timeout: 8000 });
  await page.waitForTimeout(2000);
  results.putCalls = apiCalls.filter((c) => c.method === "PUT" && c.path.indexOf("/api/v1/memories/") === 0).map((c) => c.body);

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForTimeout(2500);
  const after = await api("GET", "/api/v1/memories/" + encodeURIComponent(memId) + "/config");
  results.configAfterReload = after.data;
  const triggerAfter = await page.evaluate(() => {
    const t = document.getElementById("cfgLlm");
    return t ? t.dataset.value || "" : null;
  });
  results.triggerAfterReload = triggerAfter;

  const deleted = await deleteMemory(memId);
  results.deleted = deleted;
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  if (probeMemId) {
    try {
      results.deletedOnError = await deleteMemory(probeMemId);
    } catch {}
  }
}
results.pageErrors = pageErrors.slice(0, 12);
results.apiCalls = apiCalls;
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
