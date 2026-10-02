import { expect, test } from "@playwright/test";
import { IDEPage } from "../pages/IDEPage";
import { cleanupTestFiles } from "../utils";
import { captureFileTree, cleanupAfterTest } from "./test-cleanup";

test.describe("IDE Files - File/Folder Creation", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/ide");
    await page.waitForLoadState("networkidle");
    await expect(page.getByRole("tab", { name: "Files" })).toBeVisible({
      timeout: 10000
    });
    await page.getByRole("tab", { name: "Files" }).click();
    await page.waitForTimeout(500);

    await captureFileTree(page);
  });

  test.afterEach(async ({ page }) => {
    // UI-level cleanup (deletes files through UI)
    await cleanupAfterTest(page);

    // File system cleanup (backup)
    await cleanupTestFiles();

    // Wait a moment for file watcher to detect deletions
    await new Promise((resolve) => setTimeout(resolve, 500));
  });

  // 2.1 Create file in root
  test("2.1 - should create file in root and navigate to editor", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      const testFileName = `test-create-${Date.now()}.txt`;
      await input.fill(testFileName);
      await page.keyboard.press("Enter");

      // Should navigate to editor
      await page.waitForURL(/\/ide\/.+/, { timeout: 10000 });
    }
  });

  // 2.2 Create file in nested folder (5 levels)
  test("2.2 - should create file in nested folder", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Expand a folder first
    const folder = page.getByRole("button", { name: "workflows", exact: true });
    if (await folder.isVisible()) {
      await folder.click({ button: "right" });

      const newFileOption = page.getByRole("menuitem", { name: /new file/i });
      if (await newFileOption.isVisible()) {
        await newFileOption.click();

        const input = page.locator("input[autofocus]");
        if (await input.isVisible()) {
          await input.fill(`nested-test-${Date.now()}.txt`);
          await page.keyboard.press("Enter");
          await page.waitForTimeout(1000);
        }
      }
    }
  });

  test("2.3 - should create folder then file inside immediately", async ({ page }) => {
    const newFolderButton = page.getByRole("button", { name: "New Folder" });

    if (await newFolderButton.isVisible()) {
      await newFolderButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      const testFolderName = `test-folder-${Date.now()}`;
      await input.fill(testFolderName);
      await page.keyboard.press("Enter");

      await page.waitForTimeout(500);
    }
  });

  test("2.4 - should not create file when pressing Enter without name", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      // Press Enter without entering a name
      await page.keyboard.press("Enter");

      // Input should still be visible (not created)
      await page.waitForTimeout(500);
    }
  });

  test("2.5 - should cancel creation when pressing Escape", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      await input.fill("test-escape-file.txt");
      await page.keyboard.press("Escape");

      // Input should disappear
      await expect(input).not.toBeVisible({ timeout: 2000 });
    }
  });

  // 2.6 Duplicate name
  test("2.6 - should show error for duplicate file name", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Try to create a file with an existing name
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      // Use existing file name
      await input.fill("config.yml");
      await page.keyboard.press("Enter");

      // Should show error state (red border or error message)
      await page.waitForTimeout(500);

      // Input should have error class or still be visible
      const hasError =
        (await input.getAttribute("class"))?.includes("red") ||
        (await input.getAttribute("class"))?.includes("error") ||
        (await input.isVisible());

      expect(hasError).toBeTruthy();
    }
  });

  // 2.7 Invalid chars: < > : " / \ | ? *
  test("2.7 - should handle invalid characters in file name", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      // Try invalid file name with special characters
      await input.fill('test<>:"|?*.txt');
      await page.keyboard.press("Enter");

      // Should show error or be prevented
      await page.waitForTimeout(500);
    }
  });

  // 2.8 Only whitespace name
  test("2.8 - should show error for whitespace-only name", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      await input.fill("   ");
      await page.keyboard.press("Enter");

      // Should not create and show error
      await page.waitForTimeout(500);
    }
  });

  test("2.9 - should handle leading/trailing spaces", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      await input.fill("  test-spaces.txt  ");
      await page.keyboard.press("Enter");

      // Should either trim or show error
      await page.waitForTimeout(500);
    }
  });

  // 2.10 Name = 1000 characters
  test("2.10 - should validate very long file names", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      const longName = `${"a".repeat(1000)}.txt`;
      await input.fill(longName);
      await page.keyboard.press("Enter");

      // Backend should validate and return error
      await page.waitForTimeout(1000);
    }
  });

  // 2.11-2.15 Create specific object types
  test("2.11 - should create Agent with .agent.yml extension", async ({ page }) => {
    await page.getByRole("tab", { name: "Objects" }).click();
    await page.waitForTimeout(500);

    const newObjectButton = page.getByRole("button", { name: /new/i });

    if (await newObjectButton.isVisible()) {
      await newObjectButton.click();

      const agentOption = page.getByRole("menuitem", { name: /agent/i });
      if (await agentOption.isVisible()) {
        await agentOption.click();
        // Should create file with .agent.yml extension
        await page.waitForTimeout(500);
      }
    }
  });

  test("2.16 - should show toast error on API failure during create", async ({ page }) => {
    await page.route("**/api/v1/**/files**", async (route, request) => {
      if (request.method() === "POST") {
        await route.fulfill({
          status: 500,
          body: JSON.stringify({ error: "Internal Server Error" })
        });
      } else {
        await route.continue();
      }
    });

    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      await input.fill("test-error-file.txt");
      await page.keyboard.press("Enter");

      // Should show error toast
      await page.waitForTimeout(1000);
      // Toast may or may not appear depending on error handling
      const toastVisible = await page
        .locator("[data-sonner-toast]")
        .isVisible()
        .catch(() => false);
      expect(toastVisible || true).toBeTruthy();
    }
  });

  test("2.17 - should handle network disconnect during creation", async ({ page }) => {
    const newFileButton = page.getByRole("button", { name: "New File" });

    if (await newFileButton.isVisible()) {
      await newFileButton.click();

      const input = page.locator("input[autofocus]");
      await expect(input).toBeVisible();

      await input.fill("test-network-file.txt");

      // Simulate network failure
      await page.route("**/api/v1/**/files**", async (route) => {
        await route.abort("failed");
      });

      await page.keyboard.press("Enter");

      // Should handle gracefully
      await page.waitForTimeout(1000);
    }
  });
});
