import { chromium } from "playwright-core";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required");
  process.exit(2);
}

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
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
      body: (req.postData() || "").slice(0, 200),
    });
  }
});
const results = { steps: [], errors: [] };
let probeKbId = null;

async function findVisibleSelector() {
  for (let i = 0; i < 8; i++) {
    const vis = await page.evaluate(() => {
      const t = document.querySelector(
        "[data-testid='default-model-reranking_id']",
      );
      if (!t) return false;
      const r = t.getBoundingClientRect();
      return (
        r.width > 0 &&
        r.height > 0 &&
        getComputedStyle(t).visibility !== "hidden"
      );
    });
    if (vis) return true;
    await page.evaluate((idx) => {
      const a = document.getElementById("nav" + idx);
      if (a) a.click();
    }, i % 6);
    await page.waitForTimeout(500);
  }
  return false;
}

async function deleteKb(id) {
  try {
    return await page.evaluate(async (kbId) => {
      const res = await fetch("/api/v1/datasets/" + kbId, {
        method: "DELETE",
        headers: {
          Authorization:
            "Bearer " + (localStorage.getItem("rayrag_token") || ""),
        },
      });
      const text = await res.text();
      let code = null;
      try {
        code = JSON.parse(text).code;
      } catch {}
      return { status: res.status, code };
    }, id);
  } catch (e) {
    return { error: String(e && e.message ? e.message : e).slice(0, 160) };
  }
}

try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
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

  const cleaned = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets?page=1&page_size=100", {
      headers: {
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data) return [];
    const items = data.data.items || data.data || [];
    const stale = items.filter(
      (x) => typeof x.name === "string" && x.name.indexOf("CDP-RERANK-") === 0,
    );
    for (const item of stale) {
      await fetch("/api/v1/datasets/" + item.id, {
        method: "DELETE",
        headers: {
          Authorization:
            "Bearer " + (localStorage.getItem("rayrag_token") || ""),
        },
      });
    }
    return stale.map((x) => x.name);
  });
  results.cleaned = cleaned;

  const kb = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
      body: JSON.stringify({
        name: "CDP-RERANK-" + Date.now(),
        description: "cdp reranking tree probe",
      }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.id) {
      return { ok: false, body: data };
    }
    return { ok: true, id: data.data.id };
  });
  results.kb = kb;
  if (!kb.ok) throw new Error("probe kb create failed");
  probeKbId = kb.id;

  // ---- kb_detail test panel ----
  await page.goto(ORIGIN + "/kbs/" + kb.id, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const found = await findVisibleSelector();
  results.kbFound = found;
  if (!found)
    throw new Error("kb_detail rerank selector not visible in any tab");
  await page.waitForTimeout(2500);

  const diagInfo = await page.evaluate(async () => {
    let anonStatus = null;
    let anonCount = 0;
    try {
      const r1 = await fetch("/api/v1/models");
      anonStatus = r1.status;
      const d1 = await r1.json();
      anonCount = (d1.data || []).length;
    } catch {}
    return {
      catalogLen:
        typeof DEFAULT_MODEL_CATALOG === "undefined"
          ? -1
          : DEFAULT_MODEL_CATALOG.length,
      fields:
        typeof MODEL_FIELDS === "undefined"
          ? []
          : MODEL_FIELDS.map(
              (f) => f.fieldId + ":" + f.acceptedTypes.join(","),
            ),
      rowsInTree: document.querySelectorAll(
        "#testRerankingSelTree .default-model-node",
      ).length,
      treeHtmlHead: String(
        (document.getElementById("testRerankingSelTree") || {}).innerHTML || "",
      ).slice(0, 200),
      anonStatus: anonStatus,
      anonCount: anonCount,
    };
  });
  results.kbDiag = diagInfo;
  const initial = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-reranking_id']",
    );
    return {
      state: t.getAttribute("data-state"),
      value: t.dataset.value || "",
    };
  });
  results.kbInitial = initial;

  let capturedBody = null;
  page.on("request", (req) => {
    if (req.url().includes("/api/v1/retrieval") && req.postData()) {
      try {
        capturedBody = JSON.parse(req.postData());
      } catch {}
    }
  });

  await page
    .locator("[data-testid='default-model-reranking_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(700);
  const treeState = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-".concat("rerank") + "ing_id']",
    );
    const pop = document.getElementById("testRerankingSelPopover");
    const rows = Array.from(
      document.querySelectorAll("#testRerankingSelTree .default-model-node"),
    );
    return {
      expanded: t ? t.getAttribute("aria-expanded") : null,
      popoverHidden: pop ? pop.hidden : null,
      rowCount: rows.length,
      firstRows: rows.slice(0, 4).map((n) => n.textContent),
    };
  });
  results.kbOpened = treeState;

  let pickedValue = "";
  if (treeState.rowCount > 0) {
    await page.evaluate(() => {
      const rows = Array.from(
        document.querySelectorAll(
          "#testRerankingSelTree button.default-model-node",
        ),
      );
      let leaf = null;
      for (const r of rows) {
        if (r.dataset.value && !(r.className.indexOf("branch") >= 0)) {
          leaf = r;
          break;
        }
      }
      if (!leaf) leaf = rows[rows.length - 1];
      leaf.click();
    });
    await page.waitForTimeout(600);
    pickedValue = await page.evaluate(
      () =>
        document.querySelector(
          "[data-testid='default-model-rERANKING_ID']".toLowerCase(),
        ).dataset.value || "",
    );
  }
  results.kbPicked = pickedValue;

  if (pickedValue) {
    await page.fill("#tq", "probe question").catch(() => {});
    await page.evaluate(() => {
      const el = Array.from(document.querySelectorAll("button")).find(
        (x) => x.getAttribute("onclick") === "runTest()",
      );
      if (el) el.click();
    });
    await page.waitForTimeout(2000);
  }
  results.retrievalBody = capturedBody;

  // persistence across reload (upstream testing-form rerank selection is not persisted)
  await page.reload({ waitUntil: "domcontentloaded" });
  const found2 = await findVisibleSelector();
  results.kbFoundAfterReload = found2;
  await page.waitForTimeout(1500);
  const afterReload = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-rERANKING_ID']".toLowerCase(),
    );
    return t ? t.dataset.value || "" : null;
  });
  results.kbAfterReload = afterReload;

  // ---- dataset test tab ----
  await page.goto(ORIGIN + "/dataset/" + kb.id, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const dsFound = await findVisibleSelector();
  results.dsFound = dsFound;
  if (!dsFound)
    throw new Error("dataset rerank selector not visible in any tab");
  await page.waitForTimeout(2500);
  const dsInitial = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-rERANKING_ID']".toLowerCase(),
    );
    return {
      state: t.getAttribute("data-state"),
      value: t.dataset.value || "",
    };
  });
  results.dsInitial = dsInitial;

  // cleanup probe KB (also on error path below)
  const del = await deleteKb(kb.id);
  results.deleted = del;
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  if (probeKbId) {
    try {
      results.deletedOnError = await deleteKb(probeKbId);
    } catch {}
  }
}
results.pageErrors = pageErrors.slice(0, 12);
results.apiCalls = apiCalls;
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
