import { ADMIN, expect, test } from "./support";

// Config's contents and detail are direct flex siblings so each owns its own
// scroll in the fixed frame; a wrapper here would let the document scroll.
test("config: direct-load keeps the grouped contents pane beside a scrolling detail pane", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?sw=off`);
  await expect(
    page.getByRole("heading", { level: 1, name: "Config" }),
  ).toBeVisible();

  const body = page.locator(".settings__body");
  const toc = page.locator(".settings__toc");
  await expect(toc.getByText("Bot", { exact: true })).toBeVisible();
  await expect(toc.getByText("Members", { exact: true })).toBeVisible();
  await expect(toc.getByText("Server", { exact: true })).toBeVisible();
  await expect(toc.getByText("Read-only", { exact: true })).toBeVisible();

  const geometry = await body.evaluate((element) => {
    const [toc, detail] = [...element.children] as HTMLElement[];
    const documentScroll = document.scrollingElement!;
    return {
      direct:
        toc?.classList.contains("settings__toc") &&
        detail?.classList.contains("settings__detail"),
      display: getComputedStyle(element).display,
      tocWidth: toc.getBoundingClientRect().width,
      sideBySide:
        toc.getBoundingClientRect().right <=
        detail.getBoundingClientRect().left + 1,
      tocScroll: getComputedStyle(toc).overflowY,
      detailScroll: getComputedStyle(detail).overflowY,
      bodyHeight: element.getBoundingClientRect().height,
      detailHeight: detail.getBoundingClientRect().height,
      documentScroll: documentScroll.scrollHeight - documentScroll.clientHeight,
    };
  });
  expect(geometry.direct).toBe(true);
  expect(geometry.display).toBe("flex");
  expect(geometry.tocWidth).toBeGreaterThanOrEqual(229);
  expect(geometry.tocWidth).toBeLessThanOrEqual(231);
  expect(geometry.sideBySide).toBe(true);
  expect(geometry.tocScroll).toBe("auto");
  expect(geometry.detailScroll).toBe("auto");
  expect(geometry.detailHeight).toBe(geometry.bodyHeight);
  expect(geometry.documentScroll).toBeLessThanOrEqual(0);
});

test("config: section deep links survive reload and setting search jumps to a matching section", async ({
  page,
}) => {
  await page.goto(`${ADMIN}/config?section=access&sw=off`);
  await expect(
    page.getByRole("tabpanel", { name: "Channel access" }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("tab", { name: /Channel access/ }),
  ).toHaveAttribute("aria-selected", "true");

  const search = page.getByRole("searchbox", { name: "Find a setting" });
  await search.fill("manage messages");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/section=access/);
  await expect(
    page.getByRole("tabpanel", { name: "Channel access" }),
  ).toBeVisible();
});

test("config: search filters the contents, keeps one tab stop and leaves the open panel labelled", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=pings&sw=off`);
  const tablist = page.getByRole("tablist", { name: "Settings sections" });
  await expect(page.getByRole("tab", { name: /^Pings/ })).toContainText(
    /\d\d:\d\d/,
  );
  await expect(page.getByRole("tab", { name: /^Theme/ })).not.toHaveText(
    /^Theme$/,
  );

  const search = page.getByRole("searchbox", { name: "Find a setting" });
  await search.fill("messages");
  // Pings is filtered out: its panel stays open and labelled, and the tab stop moves to the first match.
  await expect(page.getByRole("tabpanel", { name: /Pings/ })).toBeVisible();
  const visible = tablist.getByRole("tab");
  await expect(visible).toHaveText([/Chat watching/, /Channel access/]);
  await expect(tablist.getByText("Members", { exact: true })).toBeHidden();
  await expect(tablist.locator('[role="tab"][tabindex="0"]')).toHaveCount(1);
  await expect(tablist.locator('[role="tab"][tabindex="0"]')).toHaveText(
    /Chat watching/,
  );

  // Arrow keys move only through the matches.
  await page.getByRole("tab", { name: /Chat watching/ }).focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("tab", { name: /Channel access/ })).toBeFocused();
  await expect(page).toHaveURL(/section=access/);
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("tab", { name: /Chat watching/ })).toBeFocused();

  await search.fill("zzz");
  await expect(page.getByText("No settings match “zzz”.")).toBeVisible();
  await search.fill("");
  await expect(visible).toHaveCount(13);
});

test("config: the access problem is a risk chip on the page line that opens Channel access", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?sw=off`);
  await expect(page.locator(".settings__banner, .flash")).toHaveCount(0);
  const chip = page
    .locator(".pageline")
    .getByRole("link", { name: /Manage Messages missing in 2 channels/ });
  await expect(chip).toBeVisible();
  await expect(page.getByRole("tab", { name: /Channel access/ })).toContainText(
    "⚠ 2",
  );
  await chip.click();
  await expect(page).toHaveURL(/section=access/);
  await expect(page.getByRole("tab", { name: /Channel access/ })).toBeFocused();
  await expect(page.getByText(/without it the reminders/)).toBeVisible();
  // A search that hides Channel access is cleared and the tab still gets focus.
  await page.getByRole("searchbox", { name: "Find a setting" }).fill("pings");
  await chip.click();
  await expect(page.getByRole("tab", { name: /Channel access/ })).toBeFocused();
  // Config only: other pages carry no such chip.
  await page.goto(`${ADMIN}/members?sw=off`);
  await expect(page.locator(".settings__problem")).toHaveCount(0);
});

test("config on a phone: the chip stays, the strip scrolls sideways and only the panel scrolls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/config?section=theme&sw=off`);
  await expect(
    page.getByRole("link", { name: /Manage Messages missing/ }),
  ).toBeVisible();
  const metrics = await page.evaluate(() => {
    const toc = document.querySelector<HTMLElement>(".settings__toc")!;
    const detail = document.querySelector<HTMLElement>(".settings__detail")!;
    const doc = document.scrollingElement!;
    return {
      tocX: getComputedStyle(toc).overflowX,
      tocWide: toc.scrollWidth > toc.clientWidth,
      detailY: getComputedStyle(detail).overflowY,
      documentScroll:
        doc.scrollHeight -
        doc.clientHeight +
        (doc.scrollWidth - doc.clientWidth),
    };
  });
  expect(metrics.tocX).toBe("auto");
  expect(metrics.tocWide).toBe(true);
  expect(metrics.detailY).toBe("auto");
  expect(metrics.documentScroll).toBeLessThanOrEqual(0);
  await expect(page.getByRole("tab", { name: /^Theme/ })).toBeInViewport({
    ratio: 1,
  });
});
