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
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 240)));
const putBodies = [];
page.on("request", (req) => {
  const u = req.url();
  if (u.includes("/api/v1/searchapps") && req.method() === "PUT" && req.postData()) {
    try {
      putBodies.push(JSON.parse(req.postData()));
    } catch {}
  }
});
const results = { steps: [], errors: [] };

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
    const res = await fetch("/api/v1/searchapps", {
      headers: {
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data) return [];
    const stale = (data.data || []).filter(
      (x) => typeof x.name === "string" && x.name.indexOf("CDP-SAPP-") === 0,
    );
    for (const item of stale) {
      await fetch("/api/v1/searchapps/" + item.id, {
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
        name: "CDP-SAPP-KB-" + Date.now(),
        description: "cdp searchapp rerank probe",
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

  const app = await page.evaluate(
    async ({ kbId }) => {
      const res = await fetch("/api/v1/searchapps", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
        },
        body: JSON.stringify({
          name: "CDP-SAPP-" + Date.now(),
          kb_ids: [kbId],
          hybrid: false,
          rerank: false,
          rerank_id: "",
          top_k: 8,
        }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.id) {
        return { ok: false, body: data };
      }
      return { ok: true, id: data.data.id };
    },
    { kbId: kb.id },
  );
  results.app = app;
  if (!app.ok) throw new Error("probe app create failed");

  await page.goto(ORIGIN + "/searches/" + app.id, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1200);

  const initial = await page.evaluate(() => {
    const row = document.getElementById("saRerankRow");
    return {
      rowPresent: !!row,
      rowHidden: row ? getComputedStyle(row).display === "none" : null,
      rerankChecked: !!(document.getElementById("sa_rerank") || {}).checked,
      triggerPresent: !!document.querySelector(
        "[data-testid='default-model-rerank_id']",
      ),
    };
  });
  results.initial = initial;

  await page.evaluate(() => {
    const c = document.getElementById("sa_rerank");
    if (c) c.click();
  });
  await page.waitForTimeout(400);
  const toggled = await page.evaluate(() => {
    const row = document.getElementById("saRerankRow");
    return {
      rowVisible: row ? getComputedStyle(row).display !== "none" : false,
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
      rowCount: document.querySelectorAll(
        "#saRerankSelTree .default-model-node",
      ).length,
    };
  });
  results.toggled = toggled;

  await page
    .locator("[data-testid='default-model-rerank_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(700);
  const opened = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-rerank_id']",
    );
    const pop = document.getElementById("saRerankSelPopover");
    const rows = Array.from(
      document.querySelectorAll("#saRerankSelTree .default-model-node"),
    );
    return {
      expanded: t ? t.getAttribute("aria-expanded") : null,
      popoverHidden: pop ? pop.hidden : null,
      rowCount: rows.length,
      firstRows: rows.slice(0, 4).map((n) => n.textContent),
    };
  });
  results.opened = opened;

  let picked = "";
  if (opened.rowCount > 0) {
    const pick = await page.evaluate(() => {
      const rows = Array.from(
        document.querySelectorAll("#saRerankSelTree button.default-model-node"),
      );
      const diag = rows.slice(0, 5).map((el) => ({
        cls: String(el.className).slice(0, 80),
        text: String(el.textContent).slice(0, 40),
      }));
      let leaf = null;
      for (const r of rows) {
        if (String(r.className).indexOf("default-model-leaf") >= 0) {
          leaf = r;
          break;
        }
      }
      if (!leaf && rows.length) leaf = rows[rows.length - 1];
      if (leaf) leaf.click();
      return { clicked: leaf ? String(leaf.className).slice(0, 60) : "", diag };
    });
    results.pickDiag = pick.diag;
    results.pickClicked = pick.clicked;
    await page.waitForTimeout(600);
    picked = await page.evaluate(() => {
      const t = document.querySelector(
        "[data-testid='default-model-rerank_id']",
      );
      return (t && t.dataset.value) || "";
    });
  }
  results.picked = picked;

  const afterPick = await page.evaluate(() => {
    const t = document.querySelector(
      "[data-testid='default-model-rerank_id']",
    );
    return { value: (t && t.dataset.value) || "" };
  });
  results.afterPick = afterPick;

  if (picked) {
    await page.evaluate(() => {
      const el = Array.from(document.querySelectorAll("button")).find(
        (x) => x.getAttribute("onclick") === "saveSApp()",
      );
      if (el) el.click();
    });
    await page.waitForTimeout(2500);
  }

  let finalVal = "";
  for (let i = 0; i < 10; i++) {
    const v = await page.evaluate(() => {
      const t = document.querySelector(
        "[data-testid='default-model-rerank_id']",
      );
      return (t && t.dataset.value) || "";
    });
    if (v === picked && picked) {
      finalVal = v;
      break;
    }
    await page.waitForTimeout(500);
  }

  const final = await page.evaluate(async () => {
    const t = document.querySelector(
      "[data-testid='default-model-rerank_id']",
    );
    const c = document.getElementById("sa_rerank");
    const row = document.getElementById("saRerankRow");
    const res = await fetch("/api/v1/searchapps", {
      headers: {
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
    });
    const data = await res.json();
    return {
      triggerValue: (t && t.dataset.value) || "",
      rerankChecked: !!(c || {}).checked,
      rowVisible: row ? getComputedStyle(row).display !== "none" : false,
      apps: (data.data || []).map((x) => ({
        id: x.id,
        name: x.name,
        rerank: x.rerank,
        rerank_id: x.rerank_id,
      })),
    };
  });
  results.final = final;
  results.persistedValue = finalVal;
  results.putBodies = putBodies;

  const cleanup = await page.evaluate(
    async ({ appId, kbId }) => {
      const h = {
        "Content-Type": "application/json",
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      };
      const a = await fetch("/api/v1/searchapps/" + appId, {
        method: "DELETE",
        headers: h,
      });
      const k = await fetch("/api/v1/datasets/" + kbId, {
        method: "DELETE",
        headers: h,
      });
      return { app: a.status, kb: k.status };
    },
    { appId: app.id, kbId: kb.id },
  );
  results.cleanup = cleanup;
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 1));
  await page.close();
  process.exit(0);
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e).slice(0, 300));
  console.log(JSON.stringify(results, null, 1));
  await page.close().catch(() => {});
  process.exit(1);
}
