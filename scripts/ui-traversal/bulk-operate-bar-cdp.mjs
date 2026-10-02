// CDP probe for the dataset files page's bulk-operate bar (upstream
// `components/bulk-operate-bar.tsx` + `pages/dataset/dataset/*`): the operator ticks
// rows, the bar appears with the upstream item set, and every item is exercised with a
// real click — enable/disable (server-side status), parse through the reparse dialog,
// rename through the bulk rename dialog, and delete through the confirm dialog.
//
// The probe works on a dataset it creates itself and deletes it at the end, so the
// user's own datasets are untouched.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/bulk-operate-bar-cdp.mjs
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
const KB_NAME = `cdp-bulk-${STAMP}`;
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 1000 });
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
const results = { steps: [], pageErrors };
const step = (name, value) => {
  results.steps.push({ name, value });
  console.log(`• ${name}: ${JSON.stringify(value)}`);
};
const fail = (message) => {
  throw new Error(message);
};
let kbId = null;

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const token = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) throw new Error("login failed");
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = `rayrag_token=${data.data.access_token};path=/`;
    return data.data.access_token;
  }, PASSWORD);
  step("logged in", { token: `${token.slice(0, 8)}…` });

  // 1. A throwaway dataset with two tiny text files, created through the API the page
  //    itself uses (the bar is what is under test, not the upload dialog).
  const seeded = await page.evaluate(
    async ({ name }) => {
      const headers = { "Content-Type": "application/json" };
      const created = await fetch("/api/v1/datasets", {
        method: "POST",
        headers,
        body: JSON.stringify({ name, description: "bulk-operate probe" }),
      }).then((res) => res.json());
      if (created.code !== 0) throw new Error(JSON.stringify(created));
      const id = created.data.id;
      for (const fileName of ["probe-one.txt", "probe-two.txt"]) {
        const form = new FormData();
        form.append(
          "file",
          new Blob([`${fileName} says the probe wrote this line.`], { type: "text/plain" }),
          fileName,
        );
        const uploaded = await fetch(`/api/v1/datasets/${id}/documents`, {
          method: "POST",
          body: form,
        }).then((res) => res.json());
        if (uploaded.code !== 0) throw new Error(JSON.stringify(uploaded));
      }
      return { id };
    },
    { name: KB_NAME },
  );
  kbId = seeded.id;
  step("seeded dataset", { id: kbId, name: KB_NAME });

  const documents = () =>
    page.evaluate(
      async (id) =>
        fetch(`/api/v1/datasets/${id}/documents`)
          .then((res) => res.json())
          .then((data) => data.data || []),
      kbId,
    );

  // 2. The page renders two selectable rows and no bar until something is ticked.
  await page.goto(`${ORIGIN}/dataset/files/${kbId}`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='doc-select-row']", { timeout: 30000 });
  const before = await page.evaluate(() => ({
    rows: document.querySelectorAll("[data-testid='doc-select-row']").length,
    barHidden: document.getElementById("batchBar").hidden,
    selectAllLabel: document.querySelector("[data-testid='doc-select-all']").getAttribute("aria-label"),
    rowLabel: document.querySelector("[data-testid='doc-select-row']").getAttribute("aria-label"),
  }));
  results.before = before;
  step("before selection", before);
  if (before.rows !== 2) fail("the probe dataset must show two documents");
  if (!before.barHidden) fail("the bar must stay hidden until rows are selected");
  if (before.selectAllLabel !== "Select all") fail("the header checkbox is mislabelled");
  if (before.rowLabel !== "Select row") fail("the row checkbox is mislabelled");

  // 3. Ticking the header checkbox selects both rows and opens the bar.
  await page.click("[data-testid='doc-select-all']");
  const after = await page.evaluate(() => ({
    barHidden: document.getElementById("batchBar").hidden,
    count: document.getElementById("selCount").textContent,
    role: document.getElementById("batchBar").getAttribute("role"),
    ariaLabel: document.getElementById("batchBar").getAttribute("aria-label"),
    items: Array.from(document.querySelectorAll("#batchBar [role='menuitem']")).map((node) =>
      node.textContent.trim(),
    ),
    allChecked: document.querySelector("[data-testid='doc-select-all']").checked,
    allIndeterminate: document.querySelector("[data-testid='doc-select-all']").indeterminate,
  }));
  results.bar = after;
  step("bulk bar", after);
  if (after.barHidden) fail("selecting all rows must reveal the bar");
  if (after.count !== "Selected: 2 Files") fail("the count must name the selection: " + after.count);
  if (after.role !== "menu" || after.ariaLabel !== "Bulk operate") {
    fail("the bar must be the upstream menu: " + JSON.stringify(after));
  }
  for (const label of ["Enable", "Disable", "Parse", "Cancel", "Metadata", "Rename", "Delete"]) {
    if (!after.items.includes(label)) fail(`the bar is missing the ${label} item: ${after.items}`);
  }
  if (!after.allChecked || after.allIndeterminate) fail("the header checkbox must read as fully selected");
  await page.screenshot({ path: `${shots}/bulk-operate-bar-${STAMP}.png` });

  // 4. Disable then enable: the status reaches the document store, not the browser.
  await page.click("[data-testid='bulk-disabled']");
  await page.waitForFunction(
    () =>
      Array.from(document.querySelectorAll("[data-testid='doc-select-row']")).length === 2 &&
      document.getElementById("batchBar").hidden,
    null,
    { timeout: 20000 },
  );
  const disabled = await documents();
  results.disabled = disabled.map((doc) => ({ id: doc.id, status: doc.status }));
  step("disabled documents", results.disabled);
  if (!disabled.length || disabled.some((doc) => doc.status !== "0")) {
    fail("both documents must be stored as disabled: " + JSON.stringify(results.disabled));
  }
  const greyed = await page.evaluate(
    () => document.querySelectorAll("[data-run][data-status='0']").length,
  );
  if (greyed !== 2) fail("the page must render the disabled rows from the server status");

  await page.click("[data-testid='doc-select-all']");
  await page.waitForFunction(() => !document.getElementById("batchBar").hidden, null, {
    timeout: 10000,
  });
  await page.click("[data-testid='bulk-enabled']");
  await page.waitForFunction(
    async () => {
      const rows = document.querySelectorAll("[data-status='1']");
      return rows.length === 2;
    },
    null,
    { timeout: 20000 },
  );
  const enabled = await documents();
  step("enabled documents", enabled.map((doc) => doc.status));
  if (enabled.some((doc) => doc.status !== "1")) fail("re-enabling must reach the server too");

  // 5. Parse through the reparse dialog: the redo line reflects the selection's chunks.
  await page.click("[data-testid='doc-select-all']");
  await page.click("[data-testid='bulk-run']");
  await page.waitForSelector("#bulkParseModal.show", { timeout: 10000 });
  const dialog = await page.evaluate(() => ({
    title: document.getElementById("bulkParseTitle").textContent,
    tip: document.querySelector("#bulkParseModal .bulk-operate-tip").textContent,
    redo: document.getElementById("bulkParseRedoText").textContent,
    redoChecked: document.getElementById("bulkParseRedo").checked,
  }));
  results.parseDialog = dialog;
  step("parse dialog", dialog);
  if (dialog.title !== "Parse file") fail("the dialog must carry the upstream title");
  if (!dialog.redo) fail("the redo line must say what will be cleared");
  await page.screenshot({ path: `${shots}/bulk-parse-dialog-${STAMP}.png` });
  await page.click("[data-testid='bulk-parse-ok']");
  try {
    await page.waitForFunction(() => !document.getElementById("bulkParseModal").classList.contains("show"), null, { timeout: 20000 });
  } catch (error) {
    const diag = await page.evaluate(() => ({
      modalShown: document.getElementById("bulkParseModal").classList.contains("show"),
      status: (document.getElementById("bulkParseStatus") || {}).textContent || "",
      okDisabled: (document.getElementById("bulkParseOk") || {}).disabled,
    }));
    results.parseTimeoutDiag = diag;
    step("parse modal did not close", diag);
    throw error;
  }
  await page.waitForTimeout(1500);
  const parsed = await documents();
  results.parsed = parsed.map((doc) => ({
    id: doc.id,
    run: doc.run,
    chunks: doc.chunk_count,
  }));
  step("after parse", results.parsed);
  if (parsed.every((doc) => doc.run === "UNSTARTED")) {
    fail("the documents must have been queued: " + JSON.stringify(results.parsed));
  }

  // 6. Rename the selection with find/replace, then check the names on the server.
  //    Parsing is allowed to finish first: deleting a document mid-parse waits for that
  //    parse to commit, which is realistic but makes the probe's timings noisy.
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const busy = (await documents()).some((doc) => doc.run === "RUNNING");
    if (!busy) break;
    await page.waitForTimeout(1000);
  }
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='doc-select-row']", { timeout: 30000 });
  await page.click("[data-testid='doc-select-all']");
  await page.click("[data-testid='bulk-rename']");
  await page.waitForSelector("#bulkRenameModal.show", { timeout: 10000 });
  await page.fill("[data-testid='bulk-rename-find']", "probe-");
  await page.fill("[data-testid='bulk-rename-replace']", "renamed-");
  await page.fill("[data-testid='bulk-rename-prefix']", "bulk-");
  const preview = await page.evaluate(() =>
    Array.from(document.querySelectorAll("#bulkRenamePreview li")).map((node) => node.textContent),
  );
  results.renamePreview = preview;
  step("rename preview", preview);
  if (preview.length !== 2) fail("the preview must list every selected file");
  if (!preview.every((line) => line.includes("bulk-renamed-"))) {
    fail("the preview must apply find/replace and the prefix: " + JSON.stringify(preview));
  }
  await page.screenshot({ path: `${shots}/bulk-rename-dialog-${STAMP}.png` });
  await page.click("[data-testid='bulk-rename-ok']");
  await page.waitForFunction(
    () => !document.getElementById("bulkRenameModal").classList.contains("show"),
    null,
    { timeout: 30000 },
  );
  await page.waitForTimeout(500);
  const renamed = await documents();
  results.renamed = renamed.map((doc) => doc.name);
  step("renamed documents", results.renamed);
  if (!renamed.every((doc) => doc.name.startsWith("bulk-renamed-"))) {
    fail("every selected document must be renamed: " + JSON.stringify(results.renamed));
  }

  // 7. Delete the selection through the confirm dialog.
  await page.click("[data-testid='doc-select-all']");
  await page.click("[data-testid='bulk-delete']");
  await page.waitForSelector("#bulkDeleteModal.show", { timeout: 10000 });
  const confirm = await page.evaluate(() => ({
    title: document.getElementById("bulkDeleteTitle").textContent,
    tip: document.querySelector("#bulkDeleteModal .bulk-operate-tip").textContent,
    count: document.getElementById("bulkDeleteCount").textContent,
  }));
  results.deleteDialog = confirm;
  step("delete dialog", confirm);
  if (confirm.title !== "Delete files") fail("the dialog must carry the upstream title");
  if (confirm.count !== "Selected 2 files") {
    fail("the dialog must name the selection: " + confirm.count);
  }
  await page.screenshot({ path: `${shots}/bulk-delete-dialog-${STAMP}.png` });
  await page.click("[data-testid='bulk-delete-ok']");
  await page.waitForFunction(
    () => document.getElementById("batchBar").hidden,
    null,
    { timeout: 90000 },
  );
  await page.waitForTimeout(500);
  const remaining = await documents();
  step("remaining documents", remaining.length);
  if (remaining.length) fail("the selection must be gone: " + JSON.stringify(remaining));

  // 8. Nothing of the probe's own is left behind.
  const removed = await page.evaluate(
    async (id) =>
      fetch(`/api/v1/datasets`, {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ids: [id] }),
      }).then((res) => res.json()),
    kbId,
  );
  step("dataset removed", { code: removed.code });
  if (removed.code !== 0) fail("the probe dataset must be removed: " + JSON.stringify(removed));
  kbId = null;

  if (pageErrors.length) fail("page errors: " + pageErrors.join("; "));
  results.ok = true;
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  results.error = String(error.message);
  console.error("probe failed:", error.message);
  try {
    await page.screenshot({ path: `${shots}/bulk-operate-failure-${STAMP}.png` });
  } catch {}
  process.exitCode = 1;
} finally {
  // Leave the instance as it was found: the probe's dataset and its files go away.
  try {
    if (kbId) {
      await page.evaluate(
        async (id) =>
          fetch(`/api/v1/datasets`, {
            method: "DELETE",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ ids: [id] }),
          }).then((res) => res.json()),
        kbId,
      );
    }
  } catch {}
  await page.close();
  await b.close();
}
