import { $, $$, browser, expect } from "@wdio/globals";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { stopSeedServer } from "../support/runtime.mjs";

// Stable ids — do not depend on localized tab labels (SECTION_META may change).
const FEED_SECTIONS = ["recent_release", "upcoming", "popular_legacy", "classic_legacy"];
const packageDir = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const artifactDir = path.resolve(
  process.env.MPGS_E2E_ARTIFACT_DIR ?? path.join(packageDir, "artifacts"),
);

function exactButton(text) {
  return $(`//button[normalize-space(.)=${JSON.stringify(text)}]`);
}

async function clickTestId(testId) {
  const el = await $(`[data-testid=${JSON.stringify(testId)}]`);
  await el.waitForExist({ timeout: 25_000 });
  await el.scrollIntoView({ block: "center", inline: "nearest" });
  // isDisplayed can fail for overflow-x scroll tabs even when clickable.
  try {
    await el.waitForClickable({ timeout: 10_000 });
  } catch {
    // Fall through and try click anyway after scroll.
  }
  await el.click();
  return el;
}

async function clickFeedTab(section) {
  const tab = await clickTestId(`nav-feed-${section}`);
  await browser.waitUntil(async () => (await tab.getAttribute("aria-selected")) === "true", {
    timeout: 20_000,
    timeoutMsg: `expected feed tab ${section} to become selected`,
  });
}

async function clickAuxTab(kind) {
  const tab = await clickTestId(`nav-${kind}`);
  await browser.waitUntil(async () => (await tab.getAttribute("aria-selected")) === "true", {
    timeout: 20_000,
    timeoutMsg: `expected aux tab ${kind} to become selected`,
  });
}

async function expectVisibleText(text, rootSelector = "body") {
  const root = await $(rootSelector);
  await browser.waitUntil(async () => (await root.getText()).includes(text), {
    timeout: 25_000,
    timeoutMsg: `expected visible text: ${text}`,
  });
}

async function waitForFeed() {
  await browser.waitUntil(
    async () => (await $$("article.card")).length > 0,
    { timeout: 25_000, timeoutMsg: "expected at least one recommendation card" },
  );
}

async function cardNames() {
  const names = [];
  for (const heading of await $$("article.card h3")) names.push(await heading.getText());
  return names;
}

async function assertNoCriticalOverflow(width, height) {
  await browser.setWindowSize(width, height);
  await browser.saveScreenshot(path.join(artifactDir, `layout-${width}x${height}.png`));
  const layout = await browser.execute(() => {
    const viewportWidth = window.innerWidth;
    const root = document.documentElement;
    // Tabs intentionally scroll horizontally; only brand/controls must stay in view.
    const selectors = ["header.topbar", ".brand", ".topbar-controls", "main.main"];
    const outside = selectors.flatMap((selector) =>
      Array.from(document.querySelectorAll(selector))
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          return rect.left < -1 || rect.right > viewportWidth + 1 || rect.width <= 0;
        })
        .map((element) => element.className || element.tagName),
    );
    return {
      viewportWidth,
      documentOverflow: root.scrollWidth - viewportWidth,
      outside,
    };
  });
  // Allow minor scrollbar/subpixel slack; tabs use overflow-x:auto inside the topbar.
  expect(layout.documentOverflow).toBeLessThanOrEqual(8);
  expect(layout.outside).toEqual([]);
}

describe("M4 native desktop journey", () => {
  it("completes first-run onboarding and persists preferences", async () => {
    await browser.setWindowSize(1280, 800);
    await expect($("h1=选择你的界面风格")).toBeDisplayed();
    await (await $("button[data-theme='minimal']")).click();
    await (await exactButton("继续 →")).click();
    await expect($("h1=你们通常怎么玩？")).toBeDisplayed();
    await (await exactButton("4 人")).click();
    await (await exactButton("1–2 小时")).click();
    await (await exactButton("¥150 以内")).click();
    await (await exactButton("开始探索")).click();
    await expect($("nav[aria-label='主导航']")).toBeDisplayed();
    await waitForFeed();

    // A new native WebDriver session relaunches the app. Reaching the shell
    // again proves the onboarding marker/preferences survived in client SQLite.
    await browser.reloadSession();
    await expect($("nav[aria-label='主导航']")).toBeDisplayed();
    await expect($("h1=选择你的界面风格")).not.toBeExisting();
    await waitForFeed();
  });

  it("loads all four feeds with recommendation reasons", async () => {
    for (const section of FEED_SECTIONS) {
      await clickFeedTab(section);
      await waitForFeed();
      const firstCard = (await $$("article.card"))[0];
      await expect(firstCard).toBeDisplayed();
      expect((await firstCard.$$("ul.reason-list li")).length).toBeGreaterThan(0);
      await expectVisibleText("数据更新于");
    }
  });

  it("uses the explicit natural-language fallback flow", async () => {
    await clickAuxTab("natural-language");
    const input = await $("#nl-input");
    await input.waitForDisplayed({ timeout: 20_000 });
    await input.setValue("4 人合作，单局一小时以内，不要太竞技");
    await (await exactButton("推荐")).click();
    // AI is disabled in E2E seed mode — UI surfaces deterministic fallback chips.
    await browser.waitUntil(
      async () => {
        const text = await $("body").getText();
        return text.includes("确定性回退") || text.includes("规则解析模式");
      },
      { timeout: 45_000, timeoutMsg: "expected deterministic NL fallback chip" },
    );
    await waitForFeed();
    await expectVisibleText("当前由确定性规则理解输入");
  });

  it("shows upcoming and recent calendar entries with honest early-data context", async () => {
    await clickAuxTab("calendar");
    await expect($("section[aria-label='发售日历']")).toBeDisplayed();
    await expect($("button.cal-row")).toBeDisplayed();
    await expectVisibleText("早期数据", "section[aria-label='发售日历']");
    await expectVisibleText("置信度", "section[aria-label='发售日历']");
    await expectVisibleText("来源更新于", "section[aria-label='发售日历']");

    await (await exactButton("近期发售")).click();
    await expect($("button.cal-row")).toBeDisplayed();
    await expectVisibleText("数据更新于", "section[aria-label='发售日历']");
  });

  it("refreshes ranking after acknowledged feedback", async () => {
    await clickFeedTab("recent_release");
    await waitForFeed();
    const before = await cardNames();
    const firstCard = (await $$("article.card"))[0];
    const dismissedName = await (await firstCard.$("h3")).getText();
    await (await firstCard.$(".//button[normalize-space()='不感兴趣']")).click();

    await browser.waitUntil(
      async () => {
        const names = await cardNames();
        const busy = await $("[aria-busy='true']").isExisting();
        return !busy && !names.includes(dismissedName);
      },
      { timeout: 25_000, timeoutMsg: `expected feedback to remove ${dismissedName} from the refreshed feed` },
    );
    expect(await cardNames()).not.toEqual(before);
  });

  it("has no horizontal or critical topbar overflow at supported minimum sizes", async () => {
    await assertNoCriticalOverflow(1024, 640);
    await assertNoCriticalOverflow(1280, 800);
  });

  it("serves a cached snapshot with data time after the server goes offline", async () => {
    await clickFeedTab("classic_legacy");
    await waitForFeed();
    await expectVisibleText("数据更新于");

    await stopSeedServer();
    await clickFeedTab("popular_legacy");
    await expectVisibleText("离线快照");
    await expectVisibleText("数据更新于");
    await waitForFeed();
  });
});
