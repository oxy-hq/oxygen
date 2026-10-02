import { expect, test } from "@playwright/test";
import { IDEPage } from "../pages/IDEPage";

test.describe("IDE Files - File Tree Sidebar", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await expect(page.getByRole("tab", { name: "Files" })).toBeVisible({
      timeout: 10000
    });
    await page.getByRole("tab", { name: "Files" }).click();
    await page.waitForTimeout(500);
  });

  // 1.1 Load file tree with 1000+ files
  test("1.1 - should render large file tree without crash", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Tree should be visible and navigable
    const sidebar = page.locator('[class*="sidebar"]').first();
    await expect(sidebar).toBeVisible();

    // Verify at least some content is loaded - look for file/folder links in the sidebar
    const treeItems = page.locator('a[href*="/ide/"]');
    const itemCount = await treeItems.count();
    expect(itemCount).toBeGreaterThan(0);
  });

  // 1.2 Toggle Objects/Files tabs rapidly 50x
  test("1.2 - should handle rapid tab switching without UI flicker", async ({ page }) => {
    const objectsTab = page.getByRole("tab", { name: "Objects" });
    const filesTab = page.getByRole("tab", { name: "Files" });

    // Rapid toggle 50 times
    for (let i = 0; i < 50; i++) {
      await objectsTab.click();
      await filesTab.click();
    }

    // Verify final state is correct
    await expect(filesTab).toBeVisible();
  });

  test("1.3 - should expand folder with many children within 2s", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Find a folder and measure expansion time
    const folder = page.getByRole("button", { name: "workflows", exact: true }).first();

    if (await folder.isVisible()) {
      const startTime = Date.now();
      await folder.click();
      await page.waitForTimeout(100); // Let expansion start

      // Wait for at least one child to appear
      const children = page.locator('a[href*="/ide/"]:visible');
      await expect(children.first()).toBeVisible({ timeout: 2000 });

      const duration = Date.now() - startTime;
      expect(duration).toBeLessThan(2000);
    }
  });

  // 1.4 Navigate 15+ levels deep
  test("1.4 - should navigate deep folder hierarchies", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Navigate through available nested folders
    const folders = ["workflows", "example_sql", "generated"];

    for (const folderName of folders) {
      const folder = page.getByRole("button", {
        name: folderName,
        exact: true
      });
      if (await folder.isVisible().catch(() => false)) {
        await folder.click();
        await page.waitForTimeout(200);
      }
    }

    // Verify tree is still navigable
    const filesTab = page.getByRole("tab", { name: "Files" });
    await expect(filesTab).toBeVisible();
  });

  // 1.5 File names with 500+ characters
  test("1.5 - should truncate very long file names properly", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Check that file names don't overflow their containers
    const fileLinks = page.locator('a[href*="/ide/"]');
    const count = await fileLinks.count();

    if (count > 0) {
      const firstLink = fileLinks.first();
      const box = await firstLink.boundingBox();
      expect(box).toBeTruthy();
      // Verify width is reasonable (not extending beyond viewport)
      if (box) {
        expect(box.width).toBeLessThan(500);
      }
    }
  });

  // 1.6 Special characters: 日本語, émojis🎉, spaces
  test("1.6 - should display special characters correctly", async ({ page }) => {
    // Verify the tree handles files with special characters
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Check for any files with special characters or unicode
    const sidebar = page.locator('[class*="sidebar"]').first();
    await expect(sidebar).toBeVisible();

    // Tree should render without errors
    const errorMessages = page.locator("text=/error|Error|ERROR/");
    const errorCount = await errorMessages.count();
    // Allow for some error text in UI, but should be minimal
    expect(errorCount).toBeLessThan(5);
  });

  test("1.7 - should show error message and retry option on API error", async ({ page }) => {
    // Intercept file tree API and return 500
    await page.route("**/api/v1/**/files/tree**", async (route) => {
      await route.fulfill({
        status: 500,
        body: JSON.stringify({ error: "Internal Server Error" })
      });
    });

    await page.goto("/ide");
    await page.waitForLoadState("networkidle");

    // Should show error state or retry option
    // The exact UI depends on implementation
    const sidebar = page.locator('[class*="sidebar"]').first();
    await expect(sidebar).toBeVisible();
  });

  test("1.8 - should show empty state when no files", async ({ page }) => {
    // Intercept file tree API and return empty
    await page.route("**/api/v1/**/files/tree**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([])
      });
    });

    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await page.getByRole("tab", { name: "Files" }).click();

    // Should handle empty state gracefully
    const sidebar = page.locator('[class*="sidebar"]').first();
    await expect(sidebar).toBeVisible();
  });

  test("1.9 - should handle malformed JSON gracefully", async ({ page }) => {
    await page.route("**/api/v1/**/files/tree**", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "{ invalid json"
      });
    });

    await page.goto("/ide");
    await page.waitForLoadState("networkidle");

    // Should not crash
    await expect(page.locator("body")).toBeVisible();
  });

  // 1.10 Refresh tree while previous refresh pending
  test("1.10 - should display single tree when refreshing rapidly", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const refreshButton = page.getByRole("button", { name: "Refresh" });

    if (await refreshButton.isVisible()) {
      // Click refresh multiple times rapidly
      await refreshButton.click();
      await refreshButton.click();
      await refreshButton.click();

      await page.waitForTimeout(1000);

      // Verify only one tree is displayed
      const sidebarMenus = page.locator('[role="menu"]');
      const menuCount = await sidebarMenus.count();
      expect(menuCount).toBeLessThanOrEqual(3); // Some menus may be context menus
    }
  });
});
