// CDP probe for the document viewer (`pages/document-viewer/index.tsx` +
// `components/document-preview/pdf-preview.tsx` + `components/pdf-drawer`):
// it generates a two-page PDF whose geometry is known, uploads and parses it,
// then drives the real page in Chrome — pdf.js renders the canvases, selecting a
// chunk paints its `positions` boxes on the page, the popup shows the chunk text,
// zoom rescales both canvas and boxes, and the page buttons walk the document.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/document-viewer-cdp.mjs
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
const DATASET = `cdp-docview-probe-${STAMP}`;
const FIRST_TEXT = "Positioned line one";
const SECOND_TEXT = "Positioned line two";
const PAGE_TWO_TEXT = "Second page paragraph";
const FIRST_X0 = 72;
const FIRST_TOP = 792 - (700 + 0.75 * 12);
const PAGE_ONE_WIDTH = 612;
const PAGE_ONE_HEIGHT = 792;

const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

/// Two pages, 12 pt Helvetica, known baselines: page one carries two paragraphs
/// (72 pt margin, baselines 700 and 500) and page two one (margin 72, baseline 700).
function buildPdf() {
  const page = (lines) =>
    `BT /F1 12 Tf ${lines.map(([x, y, text]) => `${x} ${y} Td (${text}) Tj`).join(" ")} ET`;
  const contents = [
    page([
      [FIRST_X0, 700, FIRST_TEXT],
      [0, -200, SECOND_TEXT],
    ]),
    page([[FIRST_X0, 700, PAGE_TWO_TEXT]]),
  ];
  const widths = Array.from({ length: 95 }, (_, index) =>
    index + 32 === 32 ? 250 : 500,
  ).join(" ");
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>",
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${PAGE_ONE_WIDTH} ${PAGE_ONE_HEIGHT}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>`,
    "<< /Length " + contents[0].length + " >>\nstream\n" + contents[0] + "\nendstream",
    `<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 /Widths [${widths}] /Encoding /WinAnsiEncoding >>`,
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${PAGE_ONE_WIDTH} ${PAGE_ONE_HEIGHT}] /Contents 7 0 R /Resources << /Font << /F1 5 0 R >> >> >>`,
    "<< /Length " + contents[1].length + " >>\nstream\n" + contents[1] + "\nendstream",
  ];
  let pdf = "%PDF-1.7\n";
  const offsets = [];
  objects.forEach((body, index) => {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(pdf);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 1000 });
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
const results = { steps: [], errors: [], pageErrors };
const step = (name, value) => {
  results.steps.push({ name, value });
  console.log(`• ${name}: ${JSON.stringify(value)}`);
};
const fail = (message) => {
  throw new Error(message);
};

let datasetId = null;
try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const seeded = await page.evaluate(
    async ({ password, name, bytes }) => {
      const login = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      }).then((res) => res.json());
      const token = login.data.access_token;
      localStorage.setItem("rayrag_token", token);
      document.cookie = `rayrag_token=${token};path=/`;
      const kb = await fetch("/api/v1/datasets", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name, chunk_method: "naive" }),
      }).then((res) => res.json());
      const form = new FormData();
      form.append("file", new Blob([new Uint8Array(bytes)], { type: "application/pdf" }), "viewer.pdf");
      const uploaded = await fetch(`/api/v1/datasets/${kb.data.id}/documents`, {
        method: "POST",
        body: form,
      }).then((res) => res.json());
      const documentId = uploaded.data.id;
      await fetch(`/api/v1/datasets/${kb.data.id}/documents/${documentId}/reparse`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
      return { datasetId: kb.data.id, documentId, token };
    },
    { password: PASSWORD, name: DATASET, bytes: Array.from(buildPdf()) },
  );
  datasetId = seeded.datasetId;
  const documentId = seeded.documentId;
  step("seeded dataset", { datasetId, documentId });

  // Wait for the parse to finish so the chunk list has positions.
  let parsed = null;
  for (let attempt = 0; attempt < 90; attempt += 1) {
    parsed = await page.evaluate(
      async ({ datasetId, documentId }) =>
        fetch(`/api/v1/datasets/${datasetId}/documents/${documentId}`)
          .then((res) => res.json())
          .then((payload) => payload.data),
      { datasetId, documentId },
    );
    if (parsed?.run === "DONE" && parsed.progress >= 1) break;
    if (parsed?.run === "FAIL") fail(`parse failed: ${JSON.stringify(parsed)}`);
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  if (parsed?.run !== "DONE") fail(`parse never finished: ${JSON.stringify(parsed)}`);
  step("document parsed", { run: parsed.run, chunkCount: parsed.chunk_count });

  // 1. The viewer renders every page with pdf.js.
  await page.goto(`${ORIGIN}/document/${documentId}?ext=pdf`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForFunction(
    () => document.querySelectorAll(".docview-page canvas").length === 2,
    null,
    { timeout: 30000 },
  );
  await page.waitForFunction(
    () => {
      const canvas = document.querySelector(".docview-page canvas");
      if (!canvas || !canvas.width) return false;
      const context = canvas.getContext("2d");
      const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
      for (let index = 0; index < pixels.length; index += 4) {
        if (pixels[index] < 200 || pixels[index + 1] < 200 || pixels[index + 2] < 200) return true;
      }
      return false;
    },
    null,
    { timeout: 30000 },
  );
  results.render = await page.evaluate(() => {
    const canvases = Array.from(document.querySelectorAll(".docview-page canvas"));
    return {
      pages: document.querySelectorAll(".docview-page").length,
      canvasSizes: canvases.map((canvas) => `${canvas.width}x${canvas.height}`),
      pageLabel: document.getElementById("docviewPageLabel").textContent,
      zoom: document.getElementById("docviewZoom").textContent,
      download: document.getElementById("docviewDownload").getAttribute("href"),
      status: document.getElementById("docviewStatus").textContent,
    };
  });
  if (results.render.pages !== 2) fail("every page must render: " + JSON.stringify(results.render));
  if (results.render.status) fail("viewer reported an error: " + results.render.status);
  step("pdf.js render", results.render);

  // 2. Clicking a chunk row paints its positions on the page.
  await page.waitForFunction(
    () => document.querySelectorAll(".docview-chunk").length > 0,
    null,
    { timeout: 20000 },
  );
  const firstRow = await page.evaluate(() => {
    const row = document.querySelector(".docview-chunk");
    const chunks = window.DOC_CHUNKS || [];
    return {
      id: row.dataset.chunk,
      count: chunks.length,
      positions: (chunks.find((chunk) => chunk.id === row.dataset.chunk) || {}).positions || [],
      content: (chunks.find((chunk) => chunk.id === row.dataset.chunk) || {}).content || "",
    };
  });
  if (!firstRow.positions.length) fail("the parsed chunk has no positions to paint");
  await page.click(".docview-chunk");
  await page.waitForFunction(
    () => document.querySelectorAll(".docview-highlight").length > 0,
    null,
    { timeout: 10000 },
  );
  results.highlight = await page.evaluate(() => {
    const boxes = Array.from(document.querySelectorAll(".docview-highlight"));
    const canvas = document.querySelector(".docview-page canvas");
    return {
      boxes: boxes.length,
      page: boxes[0].dataset.page,
      left: Math.round(parseFloat(boxes[0].style.left)),
      top: Math.round(parseFloat(boxes[0].style.top)),
      width: Math.round(parseFloat(boxes[0].style.width)),
      height: Math.round(parseFloat(boxes[0].style.height)),
      canvasWidth: canvas.width,
      popup: boxes[0].querySelector(".docview-popup").textContent.slice(0, 40),
      activeRows: document.querySelectorAll(".docview-chunk.active").length,
    };
  });
  if (results.highlight.boxes !== firstRow.positions.length) {
    fail("one box per position: " + JSON.stringify(results.highlight));
  }
  const scale = results.highlight.canvasWidth / PAGE_ONE_WIDTH;
  for (const [label, actual, expected] of [
    ["left", results.highlight.left, firstRow.positions[0][1] * scale],
    ["top", results.highlight.top, firstRow.positions[0][3] * scale],
  ]) {
    if (Math.abs(actual - expected) > 2) {
      fail(`highlight ${label} ${actual} != positions * scale ${expected}`);
    }
  }
  if (Math.round(firstRow.positions[0][1]) !== FIRST_X0 || Math.round(firstRow.positions[0][3]) !== FIRST_TOP) {
    fail("chunk positions lost the generated geometry: " + JSON.stringify(firstRow.positions[0]));
  }
  if (!results.highlight.popup.length) fail("the highlight popup is empty");
  if (!results.highlight.activeRows) fail("the selected chunk row is not marked");
  step("chunk highlight", results.highlight);
  await page.screenshot({ path: `${shots}/document-viewer-highlight-${STAMP}.png` });

  // 3. Zoom rescales canvas and boxes together (upstream `ZOOM_STEPS`).
  const before = results.highlight.canvasWidth;
  await page.click("[data-testid='docview-zoom-in']");
  await page.waitForFunction(
    (width) => document.querySelector(".docview-page canvas").width > width,
    before,
    { timeout: 10000 },
  );
  results.zoom = await page.evaluate(() => {
    const canvas = document.querySelector(".docview-page canvas");
    const box = document.querySelector(".docview-highlight");
    return {
      label: document.getElementById("docviewZoom").textContent,
      canvasWidth: canvas.width,
      boxWidth: Math.round(parseFloat(box.style.width)),
      ratio: Number((canvas.width / 612).toFixed(3)),
    };
  });
  const expectedBoxWidth =
    (firstRow.positions[0][2] - firstRow.positions[0][1]) * results.zoom.ratio;
  if (Math.abs(results.zoom.boxWidth - expectedBoxWidth) > 2) {
    fail("the highlight did not follow the zoom: " + JSON.stringify(results.zoom));
  }
  step("zoom", results.zoom);

  // 4. Page navigation walks the two pages.
  await page.click("[data-testid='docview-next']");
  await page.waitForFunction(
    () => document.getElementById("docviewPageLabel").textContent.startsWith("2"),
    null,
    { timeout: 10000 },
  );
  results.pages = await page.evaluate(() => ({
    label: document.getElementById("docviewPageLabel").textContent,
  }));
  await page.click("[data-testid='docview-prev']");
  await page.waitForFunction(
    () => document.getElementById("docviewPageLabel").textContent.startsWith("1"),
    null,
    { timeout: 10000 },
  );
  results.pages.back = await page.evaluate(
    () => document.getElementById("docviewPageLabel").textContent,
  );
  if (!results.pages.back.startsWith("1")) fail("prev page did not return");
  step("page navigation", results.pages);
  await page.screenshot({ path: `${shots}/document-viewer-pages-${STAMP}.png` });

  // 5. The `?chunk=` deep link selects that chunk on load (the drawer's link).
  const deepLink = `${ORIGIN}/document/${documentId}?ext=pdf&chunk=${firstRow.id}`;
  await page.goto(deepLink, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(
    () => document.querySelectorAll(".docview-highlight").length > 0,
    null,
    { timeout: 30000 },
  );
  results.deepLink = await page.evaluate(() => ({
    boxes: document.querySelectorAll(".docview-highlight").length,
    active: document.querySelectorAll(".docview-chunk.active").length,
  }));
  if (!results.deepLink.boxes) fail("the ?chunk= deep link painted nothing");
  step("chunk deep link", results.deepLink);

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  results.diagnostics = await page
    .evaluate(() => ({
      url: location.href,
      pages: document.querySelectorAll(".docview-page").length,
      chunks: document.querySelectorAll(".docview-chunk").length,
      highlights: document.querySelectorAll(".docview-highlight").length,
      status: document.getElementById("docviewStatus")?.textContent,
    }))
    .catch((diagnosticError) => ({ diagnosticError: String(diagnosticError.message) }));
  console.error("PROBE FAILED:", results.errors[results.errors.length - 1]);
} finally {
  if (datasetId) {
    await page
      .evaluate(
        async (id) => {
          await fetch("/api/v1/datasets", {
            method: "DELETE",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ ids: [id] }),
          }).catch(() => {});
        },
        datasetId,
      )
      .catch(() => {});
  }
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
