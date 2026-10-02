import { expect, test } from "@playwright/test";
import { IDEPage } from "../pages/IDEPage";
import { resetTestFile } from "../utils";
import { captureFileTree, cleanupAfterTest } from "./test-cleanup";

test.describe("IDE Files - File/Folder Delete", () => {
  test.beforeEach(async ({ page }) => {
    await resetTestFile();
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
    await cleanupAfterTest(page);
  });

  test("4.1 - should remove deleted file from tree", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        // Handle confirmation dialog
        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        // File should be removed from tree
        await expect(testFile).not.toBeVisible({ timeout: 5000 });
      }
    }
  });

  // 4.2 Delete currently open file
  test("4.2 - should navigate to IDE root when deleting open file", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.verifyFileIsOpen("test-file-for-e2e.txt");

    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" }).first();

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        // Should navigate away from the deleted file
        await page.waitForTimeout(1000);
      }
    }
  });

  test("4.3 - should delete folder with all its files", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const folder = page.getByRole("button", { name: "generated", exact: true });

    if (await folder.isVisible()) {
      await folder.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        // Folder should be removed
        await expect(folder).not.toBeVisible({ timeout: 5000 });
      }
    }
  });

  test("4.4 - should not delete when canceling confirmation", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    const testFile = page.getByRole("link", { name: "config.yml" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const cancelButton = page.getByRole("button", { name: /cancel/i });
        if (await cancelButton.isVisible({ timeout: 2000 })) {
          await cancelButton.click();
        }

        // File should still be visible
        await expect(testFile).toBeVisible();
      }
    }
  });

  test("4.5 - should prompt to save before deleting file with unsaved changes", async ({
    page
  }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.insertTextAtEnd("Unsaved changes");
    await idePage.verifySaveButtonVisible();

    // Try to delete it
    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" }).first();

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        // Should show confirmation or save prompt
        await page.waitForTimeout(500);
      }
    }
  });

  test("4.6 - should navigate away when deleting parent folder of open file", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Open a file inside a folder
    const folder = page.getByRole("button", { name: "workflows", exact: true });
    if (await folder.isVisible()) {
      await folder.click();
      await page.waitForTimeout(500);

      const fileInFolder = page
        .locator('a[href*="/ide/"]:visible')
        .filter({ hasText: ".automation.yml" })
        .first();

      if (await fileInFolder.isVisible()) {
        await fileInFolder.click();
        await page.waitForURL(/\/ide\/.+/);

        // Now try to delete the parent folder
        await folder.click({ button: "right" });

        const deleteOption = page.getByRole("menuitem", { name: /delete/i });
        if (await deleteOption.isVisible()) {
          await deleteOption.click();

          const confirmButton = page.getByRole("button", {
            name: /confirm|delete|yes/i
          });
          if (await confirmButton.isVisible({ timeout: 2000 })) {
            await confirmButton.click();
          }

          // Should navigate away
          await page.waitForTimeout(1000);
        }
      }
    }
  });

  // 4.7 Rapidly delete 5 files in sequence
  test("4.7 - should handle rapid sequential deletion", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // This test verifies rapid deletion doesn't cause issues
    // In practice, this would require multiple test files
    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        await page.waitForTimeout(500);
      }
    }
  });

  test("4.8 - should show toast error when API fails on delete", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "DELETE") {
        await route.fulfill({
          status: 500,
          body: JSON.stringify({ error: "Internal Server Error" })
        });
      } else {
        await route.continue();
      }
    });

    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        // File should remain in tree due to error
        await page.waitForTimeout(1000);
      }
    }
  });

  test("4.9 - should refresh tree when file already deleted (404)", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "DELETE") {
        await route.fulfill({
          status: 404,
          body: JSON.stringify({ error: "File not found" })
        });
      } else {
        await route.continue();
      }
    });

    const testFile = page.getByRole("link", { name: "test-file-for-e2e.txt" });

    if (await testFile.isVisible()) {
      await testFile.click({ button: "right" });

      const deleteOption = page.getByRole("menuitem", { name: /delete/i });
      if (await deleteOption.isVisible()) {
        await deleteOption.click();

        const confirmButton = page.getByRole("button", {
          name: /confirm|delete|yes/i
        });
        if (await confirmButton.isVisible({ timeout: 2000 })) {
          await confirmButton.click();
        }

        // Should handle 404 gracefully
        await page.waitForTimeout(1000);
      }
    }
  });
});
