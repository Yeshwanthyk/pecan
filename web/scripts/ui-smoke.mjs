#!/usr/bin/env node
/**
 * Phone-viewport UI smoke for a running, fixture-backed `pecan serve`.
 *
 * Drives the real user path — empty state → new session → send → streamed
 * reply → rich markdown — in headless Chrome at iPhone size, saves a
 * screenshot per step, and prints a JSON scorecard with timings, initial JS
 * bytes, and layout checks (no horizontal page scroll).
 *
 * Usage: node web/scripts/ui-smoke.mjs --url http://127.0.0.1:PORT --out DIR --pair CODE
 *
 * `--pair` takes a fresh `pecan pair --json` code: the smoke first proves the
 * unpaired pair screen, then pairs through it the way a phone would.
 *
 * The server must use the fixture model (PECAN_PI_ARGS) and have at least one
 * project added. Exit status is 0 only when every step passes.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";

import { chromium } from "playwright-core";

const { values } = parseArgs({
  options: {
    url: { type: "string" },
    out: { type: "string", default: "ui-smoke" },
    chrome: { type: "string" },
    headed: { type: "boolean", default: false },
    pair: { type: "string" },
  },
});
if (!values.url || !values.pair) {
  process.stderr.write("usage: ui-smoke.mjs --url <server> --pair <code> [--out <dir>]\n");
  process.exit(2);
}
mkdirSync(values.out, { recursive: true });

const STEP_TIMEOUT = 30_000;
const steps = [];
const browser = await chromium.launch({
  headless: !values.headed,
  ...(values.chrome ? { executablePath: values.chrome } : { channel: "chrome" }),
});
const context = await browser.newContext({
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 2,
  isMobile: true,
  hasTouch: true,
});
const page = await context.newPage();
const consoleErrors = [];
page.on("console", (message) => {
  if (message.type() === "error") consoleErrors.push(message.text());
});
let scriptBytes = 0;
page.on("response", async (response) => {
  if (response.request().resourceType() !== "script") return;
  const length = Number(response.headers()["content-length"] ?? 0);
  scriptBytes += length > 0 ? length : (await response.body().catch(() => Buffer.alloc(0))).length;
});

async function step(name, run) {
  const started = performance.now();
  try {
    const detail = (await run()) ?? {};
    const ms = Math.round(performance.now() - started);
    await page.screenshot({ path: join(values.out, `${steps.length + 1}-${name}.png`) });
    steps.push({ name, pass: true, ms, ...detail });
  } catch (error) {
    await page.screenshot({ path: join(values.out, `${steps.length + 1}-${name}-failed.png`) }).catch(() => undefined);
    steps.push({ name, pass: false, error: String(error?.message ?? error).split("\n")[0] });
  }
}

async function noHorizontalScroll() {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  if (overflow > 1) throw new Error(`page scrolls horizontally by ${overflow}px`);
  return { overflowPx: Math.max(0, overflow) };
}

/** Bytes of the scripts index.html itself loads (entry + modulepreloads):
 * what the phone must fetch before first paint. Idle prefetches are excluded. */
async function blockingScriptBytes() {
  const html = await (await fetch(values.url)).text();
  const paths = [...html.matchAll(/(?:src|href)="(\/assets\/[^"]+\.js)"/g)].map((match) => match[1]);
  const sizes = await Promise.all(
    paths.map(async (path) => (await (await fetch(new URL(path, values.url))).arrayBuffer()).byteLength),
  );
  return sizes.reduce((sum, size) => sum + size, 0);
}

const lastAssistantText = () => page.getByTestId("thread").innerText();

await step("pair", async () => {
  await page.goto(values.url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("pair-screen").waitFor({ timeout: STEP_TIMEOUT });
  const layout = await noHorizontalScroll();
  await page.getByLabel("Pairing code").fill("0000-0000");
  await page.getByRole("button", { name: "Pair" }).click();
  await page.getByRole("alert").waitFor({ timeout: STEP_TIMEOUT });
  await page.screenshot({ path: join(values.out, "0-pair-rejected.png") });
  await page.getByLabel("Pairing code").fill(values.pair);
  await page.getByRole("button", { name: "Pair" }).click();
  await page.getByTestId("empty-state").waitFor({ timeout: STEP_TIMEOUT });
  // The unpaired 401s above are the point of this step, not app errors.
  consoleErrors.length = 0;
  return layout;
});

await step("load", async () => {
  await page.goto(values.url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("empty-state").waitFor({ timeout: STEP_TIMEOUT });
  return {
    initialScriptBytes: await blockingScriptBytes(),
    loadedScriptBytes: scriptBytes,
    ...(await noHorizontalScroll()),
  };
});

await step("new-session", async () => {
  await page.getByTestId("empty-new-session").first().click();
  await page.getByTestId("composer-input").waitFor({ timeout: STEP_TIMEOUT });
  await page.getByText("New session").first().waitFor({ timeout: STEP_TIMEOUT });
  return noHorizontalScroll();
});

await step("send-echo", async () => {
  await page.getByTestId("composer-input").fill("hello from the phone");
  const sent = performance.now();
  await page.getByTestId("send").click();
  await page.getByTestId("working").or(page.getByText("Echo:")).first().waitFor({ timeout: STEP_TIMEOUT });
  const feedbackMs = Math.round(performance.now() - sent);
  await page.waitForFunction(
    () => document.querySelector('[data-testid="thread"]')?.textContent?.includes("Echo: hello from the phone"),
    undefined,
    { timeout: STEP_TIMEOUT },
  );
  const replyMs = Math.round(performance.now() - sent);
  await page.getByTestId("working").waitFor({ state: "detached", timeout: STEP_TIMEOUT });
  const settledMs = Math.round(performance.now() - sent);
  return { feedbackMs, replyMs, settledMs, ...(await noHorizontalScroll()) };
});

await step("rich-markdown", async () => {
  await page.getByTestId("composer-input").fill("/rich");
  await page.getByTestId("send").click();
  await page.locator('[data-testid="thread"] table').first().waitFor({ timeout: STEP_TIMEOUT });
  const diagramStarted = performance.now();
  await page.locator('[data-testid="mermaid"][data-state="ready"] svg').first().waitFor({ timeout: STEP_TIMEOUT });
  const diagramMs = Math.round(performance.now() - diagramStarted);
  const text = await lastAssistantText();
  if (!text.includes("Fixture report")) throw new Error("rich reply missing heading");
  return { diagramMs, ...(await noHorizontalScroll()) };
});

await step("turn-error", async () => {
  await page.getByTestId("composer-input").fill("/error");
  await page.getByTestId("send").click();
  await page.getByTestId("turn-error").filter({ hasText: "fixture provider error" }).first()
    .waitFor({ timeout: STEP_TIMEOUT });
  return noHorizontalScroll();
});

await step("manifest", async () => {
  const linked = await page.evaluate(() => Boolean(document.querySelector('link[rel="manifest"]')));
  if (!linked) throw new Error("no <link rel=manifest> in document");
  const hasThemeColor = await page.evaluate(() => Boolean(document.querySelector('meta[name="theme-color"]')));
  if (!hasThemeColor) throw new Error("no theme-color meta tag");
  const manifest = await page.evaluate(async () => {
    const res = await fetch("/manifest.webmanifest");
    return { ok: res.ok, contentType: res.headers.get("content-type") };
  });
  if (!manifest.ok) throw new Error("manifest.webmanifest did not fetch");
  if (!manifest.contentType?.includes("application/manifest+json")) {
    throw new Error(`unexpected manifest content-type: ${manifest.contentType}`);
  }
  return noHorizontalScroll();
});

await step("draft-persists", async () => {
  const draftText = "draft survives a reload";
  await page.getByTestId("composer-input").fill(draftText);
  await page.waitForTimeout(500); // clears the ~300ms debounce before reloading
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByTestId("composer-input").waitFor({ timeout: STEP_TIMEOUT });
  await page.waitForFunction(
    (expected) => document.querySelector('[data-testid="composer-input"]')?.value === expected,
    draftText,
    { timeout: STEP_TIMEOUT },
  );
  return noHorizontalScroll();
});

await step("prompt-history-recall", async () => {
  const input = page.getByTestId("composer-input");
  await input.fill(""); // empty caret-at-start composer is required to trigger recall
  await input.click();
  const valueIs = (expected) =>
    page.waitForFunction(
      (want) => document.querySelector('[data-testid="composer-input"]')?.value === want,
      expected,
      { timeout: STEP_TIMEOUT },
    );
  // Most recent first: /error, then /rich, then the first echo prompt.
  await page.keyboard.press("ArrowUp");
  await valueIs("/error");
  await page.keyboard.press("ArrowUp");
  await valueIs("/rich");
  await page.keyboard.press("ArrowDown");
  await valueIs("/error");
  return noHorizontalScroll();
});

await step("reconnect-replay", async () => {
  const sessionId = decodeURIComponent((await page.evaluate(() => location.hash)).replace(/^#\/s\//, "").split("?")[0]);
  if (!sessionId) throw new Error("no session in the URL");
  // Another client (here: this script, as the paired phone) finishes a turn
  // while the phone is away.
  const cookie = (await context.cookies(values.url)).map(({ name, value }) => `${name}=${value}`).join("; ");
  await context.setOffline(true);
  const response = await fetch(new URL(`/api/session/${encodeURIComponent(sessionId)}/message`, values.url), {
    method: "POST",
    headers: { "content-type": "application/json", "idempotency-key": `ui-smoke-${Date.now()}`, cookie },
    body: JSON.stringify({ text: "while you were away", mode: "send" }),
  });
  if (!response.ok) throw new Error(`background send failed: ${response.status}`);
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const back = performance.now();
  await context.setOffline(false);
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await page
    .waitForFunction(
      () => document.querySelector('[data-testid="thread"]')?.textContent?.includes("Echo: while you were away"),
      undefined,
      { timeout: STEP_TIMEOUT },
    )
    .catch(async (error) => {
      const tail = (await lastAssistantText()).slice(-300).replace(/\s+/g, " ");
      throw new Error(`${error.message.split("\n")[0]} — thread tail: ${tail}`);
    });
  return { catchUpMs: Math.round(performance.now() - back), ...(await noHorizontalScroll()) };
});

await browser.close();
const scorecard = {
  pass: steps.every((entry) => entry.pass),
  viewport: "390x844",
  steps,
  consoleErrors: consoleErrors.slice(0, 10),
};
writeFileSync(join(values.out, "ui-scorecard.json"), `${JSON.stringify(scorecard, null, 2)}\n`);
process.stdout.write(`${JSON.stringify(scorecard)}\n`);
process.exit(scorecard.pass ? 0 : 1);
