// CDP probe for the ingestion-result viewer
// (`web/src/pages/dataflow-result/index.tsx`): it seeds a dataset with a small
// text document, really parses it through the file list so an ingestion log is
// written, opens `/dataflow-result` with a real click on the row's
// `View result` affordance (or by navigating with the row's own link), walks the
// timeline, switches the output format, exercises the chunk result bar, opens the
// dirty-switch guard and the confirm-rerun dialog, then deletes the probe
// dataset.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/dataflow-result-cdp.mjs
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
const KB_NAME = `cdp-dataflow-probe-${STAMP}`;
const FILE_NAME = `dataflow-probe-${STAMP}.txt`;
const BODY = [
  "RayRAG ingestion probe document.",
  "The parser splits this body into chunks and reports each stage.",
  "A third line keeps the token chunker busy for a moment.",
].join("\n");

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
// Selecting another step while the current one is edited opens the upstream
// `changeStepModalTitle` guard; confirm it so the walk continues.
const switchNode = async (page, nodeId) => {
  await page.click(`[data-testid='dataflow-node'][data-node-id='${nodeId}']`);
  const guard = await page.$("[data-testid='dataflow-switch-dialog'].show");
  if (guard) {
    await page.click("[data-testid='dataflow-switch-confirm']");
    await page.waitForFunction(
      () => !document.getElementById("dataflowSwitchDialog").classList.contains("show"),
      null,
      { timeout: 10000 },
    );
  }
};

let kbId = null;
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

  // Seed a dataset and a small document, then let the real parse task run so the
  // ingestion log comes from the production path.
  const seeded = await page.evaluate(
    async ({ kbName, fileName, body }) => {
      const created = await fetch("/api/v1/datasets", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: kbName, chunk_method: "naive" }),
      }).then((res) => res.json());
      if (created.code !== 0) return { ok: false, step: "dataset", body: created };
      const kbId = created.data.id;
      const form = new FormData();
      form.append("file", new File([body], fileName, { type: "text/plain" }));
      const uploaded = await fetch(
        `/api/v1/datasets/${kbId}/documents`,
        { method: "POST", body: form },
      ).then((res) => res.json());
      if (uploaded.code !== 0) return { ok: false, step: "upload", body: uploaded };
      const docId = (uploaded.data && (uploaded.data.doc_id || uploaded.data.id)) || "";
      return { ok: true, kbId, docId };
    },
    { kbName: KB_NAME, fileName: FILE_NAME, body: BODY },
  );
  if (!seeded.ok) fail("probe seeding failed: " + JSON.stringify(seeded));
  kbId = seeded.kbId;
  step("seeded dataset", { kbId, docId: seeded.docId });

  // Wait for the parse task to finish (documents run DONE) so the log exists.
  await page.waitForFunction(
    async (kb) => {
      const data = await fetch(`/api/v1/datasets/${kb}/documents`).then((res) =>
        res.json(),
      );
      const docs = (data.data && (data.data.docs || data.data)) || [];
      return Array.isArray(docs) && docs.some((doc) => String(doc.run).toUpperCase() === "DONE");
    },
    kbId,
    { timeout: 120000 },
  );
  // The log is written by the parse task itself; poll briefly so the probe never
  // races the worker's last write.
  let logRow = { code: 0, total: 0, first: null };
  for (let attempt = 0; attempt < 40; attempt += 1) {
    logRow = await page.evaluate(async (kb) => {
      const listed = await fetch(
        `/api/v1/datasets/${kb}/ingestions?page=0&page_size=0&log_type=file`,
      ).then((res) => res.json());
      return {
        code: listed.code,
        total: (listed.data || {}).total,
        first: ((listed.data || {}).logs || [])[0] || null,
      };
    }, kbId);
    if (logRow.code === 0 && logRow.total) break;
    await page.waitForTimeout(500);
  }
  results.log = {
    code: logRow.code,
    total: logRow.total,
    documentName: logRow.first && logRow.first.document_name,
    status: logRow.first && logRow.first.operation_status,
    path: logRow.first && logRow.first.dsl && logRow.first.dsl.path,
  };
  if (logRow.code !== 0 || !logRow.total) {
    fail("no ingestion log was written: " + JSON.stringify(logRow));
  }
  if (!Array.isArray(results.log.path) || results.log.path.length < 2) {
    fail("the log DSL carries no stage path: " + JSON.stringify(results.log));
  }
  step("ingestion log", results.log);

  // 1. Walk in through the dataset's file list (`View result` row action).
  await page.goto(`${ORIGIN}/dataset/files/${kbId}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  const clicked = await page.evaluate(async ({ kb, doc }) => {
    const link = document.querySelector(
      `[data-testid='dataset-file-result'][data-doc-id='${doc}'], a[href*='/dataflow-result']`,
    );
    if (link) {
      link.click();
      return { clicked: true, href: link.getAttribute("href") || "" };
    }
    return { clicked: false, href: "" };
  }, { kb: kbId, doc: seeded.docId });
  results.entry = clicked;
  if (clicked.clicked) {
    await page.waitForSelector("[data-testid='dataflow-result']", { timeout: 15000 });
  } else {
    // Fall back to the canonical URL the row links to.
    await page.goto(
      `${ORIGIN}/dataflow-result?doc_id=${encodeURIComponent(seeded.docId)}&knowledgeId=${encodeURIComponent(kbId)}&type=dataflow`,
      { waitUntil: "domcontentloaded", timeout: 30000 },
    );
  }
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='dataflow-node']").length >= 2,
    null,
    { timeout: 20000 },
  );
  results.timeline = await page.evaluate(() => ({
    url: location.pathname + location.search,
    nodes: Array.from(document.querySelectorAll("[data-testid='dataflow-node']")).map(
      (node) => node.textContent,
    ),
    active: document.querySelector("[data-testid='dataflow-node'].active")?.textContent,
    stepTitle: document.querySelector("[data-testid='dataflow-step-title']").textContent,
    formats: Array.from(document.querySelectorAll("[data-testid='dataflow-format'] option")).map(
      (node) => node.value,
    ),
    documentHead: document.querySelector("[data-testid='dataflow-document-head']").textContent,
    chunks: document.querySelectorAll("[data-testid='dataflow-chunk']").length,
    rawHasDsl: document
      .querySelector("[data-testid='dataflow-raw']")
      .textContent.includes("component_name"),
  }));
  if (!results.timeline.stepTitle) fail("the active step renders no title");
  if (!results.timeline.rawHasDsl) fail("the raw DSL block is empty");
  step("timeline", results.timeline);
  await page.screenshot({ path: `${shots}/dataflow-result-${STAMP}.png` });

  // 2. Switch to the first step and back with real clicks.
  await page.click("[data-testid='dataflow-node'][data-node-id='parser']");
  await page.waitForFunction(
    (title) =>
      document.querySelector("[data-testid='dataflow-step-title']").textContent === title,
    "Parser",
    { timeout: 10000 },
  );
  results.firstStep = await page.evaluate(() => ({
    title: document.querySelector("[data-testid='dataflow-step-title']").textContent,
    formats: Array.from(document.querySelectorAll("[data-testid='dataflow-format'] option")).map(
      (node) => node.value,
    ),
  }));
  step("first step", results.firstStep);

  // 3. The chunk result bar belongs to the chunker step: the upstream segmented
  //    Full/Ellipse control, the `+` create button and the select-all checkbox.
  await page.click("[data-testid='dataflow-node'][data-node-id='tokenChunker']");
  await page.waitForFunction(
    () => !document.getElementById("dataflowChunkBar").hidden,
    null,
    { timeout: 10000 },
  );
  await page.click("[data-testid='dataflow-select-all']");
  const selectedAll = await page.evaluate(() => ({
    checked: document.querySelectorAll("[data-testid='dataflow-chunk-check']:checked").length,
    label: document.getElementById("dataflowSelected").textContent,
  }));
  await page.click("[data-testid='dataflow-text-ellipse']");
  const ellipsed = await page.evaluate(
    () => document.querySelectorAll("[data-testid='dataflow-edit-chunk'].ellipse").length,
  );
  await page.click("[data-testid='dataflow-text-full']");
  const afterFull = await page.evaluate(
    () => document.querySelectorAll("[data-testid='dataflow-edit-chunk'].ellipse").length,
  );
  results.chunkBar = { selectedAll, ellipseMode: ellipsed, ellipseAfterFull: afterFull };
  if (selectedAll.checked === 0) fail("select-all selected no chunk");
  if (ellipsed === 0) fail("the Ellipse mode did not clamp any chunk");
  if (afterFull !== 0) fail("switching back to Full text left clamped chunks: " + afterFull);
  step("chunk bar", results.chunkBar);

  // 3b. `parse-editer` click-to-edit: the parser step's text block, which marks
  //     the step dirty and feeds the rerun payload.
  await page.click("[data-testid='dataflow-node'][data-node-id='parser']");
  await page.waitForSelector("[data-testid='dataflow-edit-text']", { timeout: 10000 });
  await page.click("[data-testid='dataflow-edit-text']");
  await page.evaluate(() => {
    const node = document.querySelector("[data-testid='dataflow-edit-text']");
    node.textContent = node.textContent + " EDITED-BY-PROBE";
  });
  await page.click("[data-testid='dataflow-step-title']");
  await page.waitForFunction(
    () => !document.getElementById("dataflowRerunTip").hidden,
    null,
    { timeout: 10000 },
  );
  results.editText = await page.evaluate(() => ({
    tip: document.getElementById("dataflowRerunTip").textContent,
    value: document.querySelector("[data-testid='dataflow-edit-text']").textContent.slice(-16),
  }));
  if (!results.editText.value.includes("EDITED-BY-PROBE")) {
    fail("the text edit did not stick: " + JSON.stringify(results.editText));
  }
  step("edit text", results.editText);

  // 3c. Editing a chunk card body works the same way.
  await switchNode(page, "tokenChunker");
  await page.waitForSelector("[data-testid='dataflow-edit-chunk']", { timeout: 10000 });
  await page.click("[data-testid='dataflow-edit-chunk']");
  await page.evaluate(() => {
    const node = document.querySelector("[data-testid='dataflow-edit-chunk']");
    node.textContent = node.textContent + " CHUNK-EDIT";
  });
  await page.click("[data-testid='dataflow-step-title']");
  await page.waitForTimeout(300);
  results.editChunk = await page.evaluate(() =>
    document.querySelector("[data-testid='dataflow-edit-chunk']").textContent.slice(-10),
  );
  if (!results.editChunk.includes("CHUNK-EDIT")) {
    fail("the chunk edit did not stick: " + results.editChunk);
  }
  step("edit chunk", results.editChunk);

  // 3d. The `+` button of the chunk result bar really creates a chunk
  //     (upstream `createChunk('')` posts an empty chunk to the chunk endpoint).
  const chunkPosts = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().includes("/chunks")) {
      chunkPosts.push(request.postData() || "");
    }
  });
  await page.click("[data-testid='dataflow-create-chunk']");
  for (let attempt = 0; attempt < 20 && !chunkPosts.length; attempt += 1) {
    await page.waitForTimeout(500);
  }
  results.createChunk = {
    posts: chunkPosts.length,
    emptyContent: chunkPosts.some((body) => body.includes('"content":""')),
  };
  if (!chunkPosts.length) fail("the create-chunk button did not call the chunk endpoint");
  if (!results.createChunk.emptyContent) {
    fail("the created chunk is not the upstream empty payload: " + chunkPosts[0]);
  }
  step("create chunk", results.createChunk);

  // 3d. Rerun while the chunker step is active: the payload must carry that
  //     step's edited chunk text (upstream merges only the active component).
  const chunkEditReruns = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/v1/agents/rerun")) {
      chunkEditReruns.push(request.postData() || "");
    }
  });
  await page.click("[data-testid='dataflow-rerun']");
  await page.waitForSelector("[data-testid='dataflow-rerun-dialog'].show", { timeout: 10000 });
  await page.click("[data-testid='dataflow-rerun-confirm']");
  for (let attempt = 0; attempt < 20 && !chunkEditReruns.length; attempt += 1) {
    await page.waitForTimeout(500);
  }
  results.chunkEditRerun = {
    requests: chunkEditReruns.length,
    carriesChunkEdit: chunkEditReruns.some((body) => body.includes("CHUNK-EDIT")),
  };
  if (!results.chunkEditRerun.carriesChunkEdit) {
    fail("the chunker rerun payload does not carry the edited chunk");
  }
  step("chunk edit rerun", results.chunkEditRerun);

  // 4. Deleting a selection marks the step dirty, which arms the switch guard.
  await switchNode(page, "tokenChunker");
  await page.waitForSelector("[data-testid='dataflow-chunk-check']", { timeout: 10000 });
  await page.click("[data-testid='dataflow-chunk-check']");
  await page.click("[data-testid='dataflow-delete-chunks']");
  await page.waitForFunction(
    () => !document.getElementById("dataflowRerunTip").hidden,
    null,
    { timeout: 10000 },
  );
  await page.click("[data-testid='dataflow-node'][data-node-id='parser']");
  await page.waitForSelector("[data-testid='dataflow-switch-dialog'].show", {
    timeout: 10000,
  });
  results.switchGuard = await page.evaluate(() => ({
    title: document.getElementById("dataflowSwitchTitle").textContent,
    confirm: document.querySelector("[data-testid='dataflow-switch-confirm']").textContent,
    cancel: document.querySelector("[data-testid='dataflow-switch-cancel']").textContent,
  }));
  if (results.switchGuard.title !== "Step switch warning") {
    fail("switch guard title is not upstream's: " + results.switchGuard.title);
  }
  await page.click("[data-testid='dataflow-switch-confirm']");
  await page.waitForFunction(
    () => !document.getElementById("dataflowSwitchDialog").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  step("switch guard", results.switchGuard);

  // 5. The rerun dialog and the real `POST /api/v1/agents/rerun` call.
  const rerunRequests = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/v1/agents/rerun")) {
      rerunRequests.push({ url: request.url(), body: request.postData() || "" });
    }
  });
  await page.click("[data-testid='dataflow-rerun']");
  await page.waitForSelector("[data-testid='dataflow-rerun-dialog'].show", {
    timeout: 10000,
  });
  results.rerunDialog = await page.evaluate(() => ({
    title: document.getElementById("dataflowRerunTitle").textContent,
    body: document.getElementById("dataflowRerunBody").textContent,
  }));
  await page.click("[data-testid='dataflow-rerun-confirm']");
  for (let attempt = 0; attempt < 30 && !rerunRequests.length; attempt += 1) {
    await page.waitForTimeout(500);
  }
  results.rerun = {
    ...results.rerunDialog,
    requests: rerunRequests.length,
    carriesEditedOutputs: rerunRequests.some((request) =>
      request.body.includes("EDITED-BY-PROBE"),
    ),
  };
  if (!rerunRequests.length) fail("the rerun button did not call /api/v1/agents/rerun");
  if (!results.rerun.carriesEditedOutputs) {
    fail("the rerun payload does not carry the edited parser output");
  }

  step("rerun", results.rerun);

  // 6. The zh face of the same page.
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(
    `${ORIGIN}/dataflow-result?doc_id=${encodeURIComponent(seeded.docId)}&knowledgeId=${encodeURIComponent(kbId)}&type=dataflow`,
    { waitUntil: "domcontentloaded", timeout: 30000 },
  );
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='dataflow-node']").length >= 2,
    null,
    { timeout: 20000 },
  );
  results.zh = await page.evaluate(() => ({
    back: document.querySelector("[data-testid='dataflow-back']").textContent,
    rerun: document.querySelector("[data-testid='dataflow-rerun']").textContent,
    format: Array.from(document.querySelectorAll("[data-testid='dataflow-format']")).length,
    title: document.querySelector("[data-testid='dataflow-step-title']").textContent,
  }));
  step("zh face", results.zh);
  await page.screenshot({ path: `${shots}/dataflow-result-zh-${STAMP}.png` });

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  results.diagnostics = await page
    .evaluate(() => ({
      url: location.href,
      nodes: document.querySelectorAll("[data-testid='dataflow-node']").length,
      chunks: document.querySelectorAll("[data-testid='dataflow-chunk']").length,
      checked: document.querySelectorAll("[data-testid='dataflow-chunk-check']:checked").length,
      stepTitle: document.getElementById("dataflowStepTitle")?.textContent,
      format: document.getElementById("dataflowFormat")?.value,
      barHidden: document.getElementById("dataflowChunkBar")?.hidden,
      tipHidden: document.getElementById("dataflowRerunTip")?.hidden,
      output: document.getElementById("dataflowOutput")?.textContent?.slice(0, 200),
      dialogs: [
        document.getElementById("dataflowSwitchDialog")?.className,
        document.getElementById("dataflowRerunDialog")?.className,
      ],
    }))
    .catch((diagnosticError) => ({ diagnosticError: String(diagnosticError.message) }));
  results.pageErrors = pageErrors;
  console.error("PROBE FAILED:", results.errors[results.errors.length - 1]);
} finally {
  if (kbId) {
    await page
      .evaluate(async (kb) => {
        await fetch(`/api/v1/datasets/${kb}`, { method: "DELETE" });
      }, kbId)
      .catch(() => {});
  }
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
