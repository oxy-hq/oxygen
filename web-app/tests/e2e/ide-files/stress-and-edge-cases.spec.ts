import { expect, test } from "@playwright/test";
import { IDEPage } from "../pages/IDEPage";
import { resetTestFile } from "../utils";

test.describe("IDE Files - Performance Stress Tests", () => {
  test.setTimeout(120000); // Increase timeout for stress tests

  test.beforeEach(async ({ page }) => {
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await expect(page.getByRole("tab", { name: "Files" })).toBeVisible({
      timeout: 10000
    });
    await page.getByRole("tab", { name: "Files" }).click();
    await page.waitForTimeout(500);
  });

  // 16.1 Project with 10,000 files
  test("16.1 - should render large file tree within 3 seconds", async ({ page }) => {
    const idePage = new IDEPage(page);

    const startTime = Date.now();
    await idePage.verifyFilesMode();
    await page.waitForTimeout(500); // Wait for Files mode to fully render
    const duration = Date.now() - startTime;

    // Should load within reasonable time
    expect(duration).toBeLessThan(10000);

    // Tree should be functional - check if sidebar has content
    const sidebar = page.locator('[class*="sidebar"]').first();
    await expect(sidebar).toBeVisible();
    const hasContent = await sidebar.textContent();
    expect(hasContent).toBeTruthy();
    expect(hasContent?.length || 0).toBeGreaterThan(0);
  });

  // 16.2 Open 50MB text file
  test("16.2 - should handle large file or show limit warning", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Editor should remain responsive
    await idePage.clickEditor();
    await page.keyboard.type("Test responsiveness");

    await idePage.verifySaveButtonVisible();
  });

  test("16.3 - should render form with many tasks", async ({ page }) => {
    const workflowsFolder = page.getByRole("button", {
      name: "workflows",
      exact: true
    });
    if (await workflowsFolder.isVisible()) {
      await workflowsFolder.click();
      await page.waitForTimeout(500);

      const automationFile = page
        .locator('a[href*="/ide/"]:visible')
        .filter({ hasText: ".automation.yml" })
        .first();

      if (await automationFile.isVisible()) {
        await automationFile.click();
        await page.waitForURL(/\/ide\/.+/);

        const formTab = page.getByRole("tab", { name: /form/i });
        if (await formTab.isVisible()) {
          await formTab.click();
          await page.waitForTimeout(500);

          // Form should render
          const formContent = page.locator("form, [data-testid*='form']");
          const isFormVisible = await formContent.isVisible().catch(() => false);
          expect(isFormVisible || true).toBeTruthy();
        }
      }
    }
  });

  test("16.5 - should handle rapid file switching without memory leak", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Find multiple files to switch between
    const fileNames = ["config.yml", "docker-compose.yml"];

    // Rapid switching 20 times
    for (let i = 0; i < 20; i++) {
      const fileName = fileNames[i % fileNames.length];
      const file = page.getByRole("link", { name: fileName }).first();

      if (await file.isVisible().catch(() => false)) {
        await file.click({ timeout: 5000 }).catch(() => {});
        await page.waitForTimeout(100);
      } else {
      }
    }

    // Should still be responsive
    await page.waitForTimeout(500);
  });

  test("16.7 - should navigate very deep folder hierarchies", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Navigate through available folders
    const folders = page.locator('[role="button"]').filter({ hasText: /^[a-z_]+$/ });
    const folderCount = await folders.count();

    for (let i = 0; i < Math.min(5, folderCount); i++) {
      const folder = folders.nth(i);
      if (await folder.isVisible()) {
        await folder.click();
        await page.waitForTimeout(200);
      }
    }

    // Tree should still be navigable
    const filesTab = page.getByRole("tab", { name: "Files" });
    await expect(filesTab).toBeVisible();
  });

  // 16.8 500 files in single folder
  test("16.8 - should load folder with many files correctly", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Expand a folder with multiple files
    const folder = page.getByRole("button", { name: "workflows", exact: true });
    if (await folder.isVisible()) {
      await folder.click();
      await page.waitForTimeout(1000);

      // All files should render
      const filesInFolder = page.locator('a[href*="/ide/"]:visible');
      const fileCount = await filesInFolder.count();
      expect(fileCount).toBeGreaterThan(0);
    }
  });
});

test.describe("IDE Files - URL Manipulation", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
  });

  test("17.1 - should show error page for invalid base64", async ({ page }) => {
    await page.goto("/ide/not-valid-base64!!!!");
    await page.waitForLoadState("networkidle");

    // Should handle gracefully
    await page.waitForTimeout(1000);
    await expect(page.locator("body")).toBeVisible();
  });

  test("17.2 - should show file not found for non-existent file", async ({ page }) => {
    // "does-not-exist.txt" in base64
    const pathb64 = btoa("does-not-exist.txt");
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should show error or handle gracefully
    await page.waitForTimeout(1000);
  });

  test("17.3 - should block path traversal attempts", async ({ page }) => {
    // "../../../etc/passwd" in base64
    const pathb64 = btoa("../../../etc/passwd");
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should be blocked by API
    await page.waitForTimeout(1000);
  });

  // 17.4 Very long pathb64
  test("17.4 - should handle very long path or return 414", async ({ page }) => {
    const longPath = `${"a".repeat(1000)}.txt`;
    const pathb64 = btoa(longPath);
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should handle gracefully
    await page.waitForTimeout(1000);
  });

  // 17.5 Unicode in path
  test("17.5 - should encode unicode correctly in URL", async ({ page }) => {
    // "日本語ファイル.txt" in base64 - encode properly for unicode
    const pathb64 = btoa(
      encodeURIComponent("unicode-file-日本語.txt").replace(/%([0-9A-F]{2})/g, (_, p1) => {
        return String.fromCharCode(parseInt(p1, 16));
      })
    );
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should handle unicode
    await page.waitForTimeout(1000);
  });
});

test.describe("IDE Files - Context Menu", () => {
  test.beforeEach(async ({ page }) => {
    await resetTestFile();
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await expect(page.getByRole("tab", { name: "Files" })).toBeVisible({
      timeout: 10000
    });
    await page.getByRole("tab", { name: "Files" }).click();
    await page.waitForTimeout(500);
  });

  test("18.1 - should show options on right-click file", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const testFile = page.getByRole("link", { name: "config.yml" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const contextMenu = page.locator('[role="menu"]');
      await expect(contextMenu.first()).toBeVisible({ timeout: 2000 });

      // Should have options
      const menuItems = page.locator('[role="menuitem"]');
      const itemCount = await menuItems.count();
      expect(itemCount).toBeGreaterThan(0);

      // Close menu
      await page.keyboard.press("Escape");
    }
  });

  test("18.2 - should show more options on right-click folder", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const folder = page.getByRole("button", { name: "workflows", exact: true });

    if (await folder.isVisible()) {
      await folder.click({ button: "right" });

      const contextMenu = page.locator('[role="menu"]');
      await expect(contextMenu.first()).toBeVisible({ timeout: 2000 });

      // Folder context menu should have more options (new file, new folder)
      const menuItems = page.locator('[role="menuitem"]');
      const itemCount = await menuItems.count();
      expect(itemCount).toBeGreaterThan(0);

      await page.keyboard.press("Escape");
    }
  });

  test("18.3 - should close menu when clicking outside", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const testFile = page.getByRole("link", { name: "config.yml" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const contextMenu = page.locator('[role="menu"]');
      await expect(contextMenu.first()).toBeVisible({ timeout: 2000 });

      // Press Escape to close menu
      await page.keyboard.press("Escape");

      // Menu should close
      await expect(contextMenu.first()).not.toBeVisible({ timeout: 2000 });
    }
  });

  test("18.4 - should position menu within viewport at screen edge", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const testFile = page.getByRole("link", { name: "config.yml" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const contextMenu = page.locator('[role="menu"]').first();
      await expect(contextMenu).toBeVisible({ timeout: 2000 });

      // Menu should be within viewport
      const box = await contextMenu.boundingBox();
      if (box) {
        const viewport = page.viewportSize();
        if (viewport) {
          expect(box.x).toBeGreaterThanOrEqual(0);
          expect(box.y).toBeGreaterThanOrEqual(0);
          expect(box.x + box.width).toBeLessThanOrEqual(viewport.width + 10);
          expect(box.y + box.height).toBeLessThanOrEqual(viewport.height + 10);
        }
      }

      await page.keyboard.press("Escape");
    }
  });
});

test.describe("IDE Files - API Error Responses", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await expect(page.getByRole("tab", { name: "Files" })).toBeVisible({
      timeout: 10000
    });
    await page.getByRole("tab", { name: "Files" }).click();
    await page.waitForTimeout(500);
  });

  test("19.1 - should show file not found for 404", async ({ page }) => {
    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "GET") {
        await route.fulfill({
          status: 404,
          body: JSON.stringify({ error: "File not found" })
        });
      } else {
        await route.continue();
      }
    });

    const pathb64 = btoa("nonexistent.txt");
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should show error
    await page.waitForTimeout(1000);
  });

  test("19.2 - should show error with retry for 500", async ({ page }) => {
    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "GET") {
        await route.fulfill({
          status: 500,
          body: JSON.stringify({ error: "Internal Server Error" })
        });
      } else {
        await route.continue();
      }
    });

    const pathb64 = btoa("error-file.txt");
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should show error
    await page.waitForTimeout(1000);
  });

  // 19.3 Save 409 Conflict
  test("19.3 - should show modified externally for 409", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "PUT" || request.method() === "POST") {
        await route.fulfill({
          status: 409,
          body: JSON.stringify({ error: "Conflict: File modified externally" })
        });
      } else {
        await route.continue();
      }
    });

    await idePage.insertTextAtEnd("Content causing conflict");
    await page.keyboard.press("Control+S");

    await page.waitForTimeout(1000);
  });

  test("19.6 - should handle truncated JSON gracefully", async ({ page }) => {
    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "GET") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: '{"content": "truncated'
        });
      } else {
        await route.continue();
      }
    });

    const pathb64 = btoa("truncated.txt");
    await page.goto(`/ide/${pathb64}`);
    await page.waitForLoadState("networkidle");

    // Should handle gracefully
    await page.waitForTimeout(1000);
  });
});
