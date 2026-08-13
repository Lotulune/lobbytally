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
    const viewportHeight = window.innerHeight;
    const root = document.documentElement;
    const selectors = ["header.topbar", ".brand", ".topbar-controls", "main.main"];
    const outside = selectors.flatMap((selector) =>
      Array.from(document.querySelectorAll(selector))
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          return rect.left < -1 || rect.right > viewportWidth + 1 || rect.width <= 0;
        })
        .map((element) => element.className || element.tagName),
    );
    const clippedTabs = Array.from(document.querySelectorAll(".tabs .tab"))
      .filter((element) => {
        const nav = element.closest(".tabs");
        if (!nav) return true;
        const rect = element.getBoundingClientRect();
        const navRect = nav.getBoundingClientRect();
        return rect.left < navRect.left - 1 || rect.right > navRect.right + 1;
      })
      .map((element) => element.getAttribute("data-testid") ?? element.textContent?.trim());
    const frame = document.querySelector(".window-frame")?.getBoundingClientRect();
    const topbar = document.querySelector(".topbar")?.getBoundingClientRect();
    const main = document.querySelector("main.main");
    const mainRect = main?.getBoundingClientRect();
    const firstRowCards = Array.from(document.querySelectorAll("article.card"))
      .map((card) => ({ card, rect: card.getBoundingClientRect() }))
      .filter(({ rect }, _index, cards) => Math.abs(rect.top - (cards[0]?.rect.top ?? rect.top)) <= 1);
    const actionBottoms = firstRowCards
      .map(({ card }) => card.querySelector(".card-actions")?.getBoundingClientRect().bottom)
      .filter((bottom) => typeof bottom === "number");
    return {
      viewportWidth,
      documentOverflow: root.scrollWidth - viewportWidth,
      documentVerticalOverflow: root.scrollHeight - viewportHeight,
      outside,
      clippedTabs,
      frameCoversViewport:
        frame != null &&
        frame.left <= 1 &&
        frame.top <= 1 &&
        frame.right >= viewportWidth - 1 &&
        frame.bottom >= viewportHeight - 1,
      mainOwnsScroll:
        main != null &&
        mainRect != null &&
        topbar != null &&
        getComputedStyle(main).overflowY === "auto" &&
        mainRect.top >= topbar.bottom - 1 &&
        mainRect.bottom >= viewportHeight - 1 &&
        mainRect.bottom <= viewportHeight + 1,
      cardActionBottomSpread:
        actionBottoms.length > 1 ? Math.max(...actionBottoms) - Math.min(...actionBottoms) : 0,
    };
  });
  // Allow minor scrollbar/subpixel slack from the native WebView.
  expect(layout.documentOverflow).toBeLessThanOrEqual(8);
  expect(layout.documentVerticalOverflow).toBeLessThanOrEqual(1);
  expect(layout.outside).toEqual([]);
  expect(layout.clippedTabs).toEqual([]);
  expect(layout.frameCoversViewport).toBe(true);
  expect(layout.mainOwnsScroll).toBe(true);
  expect(layout.cardActionBottomSpread).toBeLessThanOrEqual(1);
}

async function dismissAuthDialogIfOpen() {
  const backdrop = await $(".modal-backdrop");
  if (await backdrop.isExisting()) {
    const close = await $(".auth-dialog button[aria-label='关闭']");
    if (await close.isExisting()) {
      await close.click();
    } else {
      await browser.keys("Escape");
    }
    await browser.waitUntil(async () => !(await $(".modal-backdrop").isExisting()), {
      timeout: 10_000,
      timeoutMsg: "expected auth dialog to close",
    });
  }
}

/** Feedback and play-intent require an account; anonymous clicks only open the auth gate. */
async function ensureRegisteredAccount() {
  await dismissAuthDialogIfOpen();
  // Already signed in — topbar shows avatar menu instead of 登录.
  if (await $("button[aria-label='账户菜单']").isExisting()) return;

  await clickTestId("auth-open-login");
  await $(".auth-dialog").waitForExist({ timeout: 15_000 });
  await clickTestId("auth-mode-register");
  await browser.waitUntil(
    async () => (await $("[data-testid='auth-mode-register']").getAttribute("aria-pressed")) === "true",
    { timeout: 10_000, timeoutMsg: "expected register mode" },
  );
  const suffix = Date.now().toString(36).slice(-6);
  const username = `e2e_${suffix}`;
  const password = `E2ePass_${suffix}9x`;
  await (await $("[data-testid='auth-username']")).setValue(username);
  await (await $("[data-testid='auth-display-name']")).setValue(`E2E ${suffix}`);
  await (await $("[data-testid='auth-password']")).setValue(password);
  await clickTestId("auth-submit");
  await browser.waitUntil(async () => !(await $(".modal-backdrop").isExisting()), {
    timeout: 20_000,
    timeoutMsg: "expected registration to close auth dialog",
  });
  // Avatar menu replaces 登录 when account session is active.
  await browser.waitUntil(
    async () => (await $("button[aria-label='账户菜单']").isExisting()),
    {
      timeout: 15_000,
      timeoutMsg: "expected account session after registration",
    },
  );
}

describe("M4 native desktop journey", () => {
  it("provides usable controls for the frameless window", async () => {
    await expect($("[aria-label='窗口控制']")).toBeDisplayed();
    await expect($("button[aria-label='最小化窗口']")).toBeDisplayed();
    await expect($("button[aria-label='最大化或还原窗口']")).toBeDisplayed();
    await expect($("button[aria-label='关闭窗口']")).toBeDisplayed();
  });

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
    // Theme control is a custom menu (full-hit trigger), not a native <select>.
    const themeTrigger = await $("header.topbar .theme-menu-trigger");
    await expect(themeTrigger).toBeDisplayed();
    const themeAria = await themeTrigger.getAttribute("aria-label");
    expect(themeAria ?? "").toContain("当前主题");
    await expectVisibleText("极简白线", "header.topbar .theme-menu-trigger");
    await themeTrigger.click();
    await expect($(".theme-menu-popover")).toBeDisplayed();
    await expectVisibleText("极简白线", ".theme-menu-popover [role='option'][aria-selected='true']");
    // Close the menu so later cases are not blocked by the popover.
    await browser.keys("Escape");
    await browser.waitUntil(async () => !(await $(".theme-menu-popover").isExisting()), {
      timeout: 5_000,
      timeoutMsg: "expected Escape to close theme menu",
    });
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

  it("closes account login with Escape without leaving game detail", async () => {
    await clickFeedTab("recent_release");
    await waitForFeed();
    await (await $("article.card h3")).click();
    await expect($(".detail-screen")).toBeDisplayed();
    await (await $(".detail-screen .vote-btn")).click();
    await expect($(".auth-dialog")).toBeDisplayed();

    await browser.keys("Escape");
    await browser.waitUntil(async () => !(await $(".auth-dialog").isExisting()), {
      timeout: 10_000,
      timeoutMsg: "expected Escape to close the account dialog",
    });
    await expect($(".detail-screen")).toBeDisplayed();
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
    const bodyText = await $("body").getText();
    expect(bodyText).not.toContain("party_size");
    expect(bodyText).not.toContain("session_minutes");
  });

  it("shows upcoming and recent calendar entries with honest date context", async () => {
    await clickAuxTab("calendar");
    await expect($("section[aria-label='发售日历']")).toBeDisplayed();
    await expect($("button.cal-row")).toBeDisplayed();
    // Every row keeps a precision-faithful date cell; review/confidence noise
    // is gone for unreleased entries.
    await expect($("button.cal-row .cal-when-primary")).toBeDisplayed();
    const calendarText = await $("section[aria-label='发售日历']").getText();
    expect(calendarText).not.toContain("置信度");
    expect(calendarText).not.toContain("早期数据");
    await expectVisibleText("数据更新于", "section[aria-label='发售日历']");

    await (await exactButton("近期发售")).click();
    await expect($("button.cal-row")).toBeDisplayed();
    await expectVisibleText("数据更新于", "section[aria-label='发售日历']");
  });

  it("refreshes ranking after acknowledged feedback", async () => {
    await ensureRegisteredAccount();
    await clickFeedTab("recent_release");
    await waitForFeed();
    const before = await cardNames();
    const firstCard = (await $$("article.card"))[0];
    const dismissedName = await (await firstCard.$("h3")).getText();
    await (await firstCard.$(".//button[normalize-space()='不感兴趣']")).click();

    await browser.waitUntil(
      async () => {
        if (await $(".modal-backdrop").isExisting()) return false;
        const names = await cardNames();
        const busy = await $("[aria-busy='true']").isExisting();
        return !busy && !names.includes(dismissedName);
      },
      { timeout: 25_000, timeoutMsg: `expected feedback to remove ${dismissedName} from the refreshed feed` },
    );
    expect(await cardNames()).not.toEqual(before);
  });

  it("has no horizontal or critical topbar overflow at supported minimum sizes", async () => {
    await dismissAuthDialogIfOpen();
    await assertNoCriticalOverflow(820, 520);
    await assertNoCriticalOverflow(1024, 640);
    await assertNoCriticalOverflow(1280, 800);
  });

  it("keeps the full settings page reachable in a compact window", async () => {
    await browser.setWindowSize(820, 520);
    await clickAuxTab("settings");
    await expect($("section[aria-label='设置']")).toBeDisplayed();
    const cacheHeading = await $("h4=数据与缓存");
    await cacheHeading.scrollIntoView({ block: "center" });
    await expect(cacheHeading).toBeDisplayed();
    await browser.setWindowSize(1280, 800);
  });

  it("serves a cached snapshot with data time after the server goes offline", async () => {
    await dismissAuthDialogIfOpen();
    // Account registration clears the snapshot cache; warm both sections again
    // while the seed server is still up so offline browsing has entries.
    await clickFeedTab("popular_legacy");
    await waitForFeed();
    await expectVisibleText("数据更新于");
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
