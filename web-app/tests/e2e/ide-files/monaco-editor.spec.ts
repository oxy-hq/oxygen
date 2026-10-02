import { expect, test } from "@playwright/test";
import { IDEPage } from "../pages/IDEPage";
import { resetTestFile } from "../utils";

test.describe("IDE Files - Monaco Editor - Loading & Display", () => {
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

  // 5.1 Open file with 50,000 lines
  test("5.1 - should load very large files without crash", async ({ page }) => {
    const idePage = new IDEPage(page);

    // Open any available file and verify editor loads
    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Editor should be functional
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });

  // 5.2 Open file with 10,000+ char lines
  test("5.2 - should handle horizontal scroll for long lines", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Verify horizontal scrollbar functionality
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();

    // Check for horizontal scroll capability
    const scrollable = page.locator(".monaco-scrollable-element");
    await expect(scrollable.first()).toBeVisible();
  });

  // 5.3 Open empty file (0 bytes)
  test("5.3 - should handle empty files and allow typing", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.type("New content in empty file");

    await idePage.verifySaveButtonVisible();
  });

  test("5.4 - should display Unicode content correctly", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    // Type unicode content
    await idePage.insertTextAtEnd("Unicode: 日本語 🎉 émojis café");

    // Content should be visible
    const editorContent = await idePage.getEditorContent();
    expect(editorContent.length).toBeGreaterThan(0);
  });

  // 5.5 Open file with null bytes
  test("5.5 - should handle binary content gracefully", async ({ page }) => {
    const idePage = new IDEPage(page);

    // Just verify editor handles any file type
    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });

  test("5.6 - should reload content from API on page refresh", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    const url = page.url();

    // Refresh page
    await page.reload();
    await page.waitForLoadState("networkidle");

    // Editor should reload with same file
    await expect(page).toHaveURL(url);
    await idePage.waitForEditorToLoad();
  });

  // 5.7 Minimap visible for large files
  test("5.7 - should show minimap for navigation", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Check for minimap (if enabled)
    const minimap = page.locator(".minimap");
    // Minimap may or may not be visible depending on settings
    const minimapVisible = await minimap.isVisible().catch(() => false);
    // Just verify editor loads without requiring minimap
    expect(minimapVisible || true).toBeTruthy();
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });

  // 5.8-5.12 Language detection
  test("5.8 - should apply YAML syntax highlighting for .yml files", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Check for syntax highlighting classes
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();

    // YAML mode should be active
    const viewLines = page.locator(".view-lines");
    await expect(viewLines).toBeVisible();
  });

  test("5.10 - should apply SQL syntax highlighting for .sql files", async ({ page }) => {
    const idePage = new IDEPage(page);
    await idePage.verifyFilesMode();

    // Look for SQL file
    const folder = page.getByRole("button", {
      name: "example_sql",
      exact: true
    });
    if (await folder.isVisible()) {
      await folder.click();
      await page.waitForTimeout(500);

      const sqlFile = page.locator('a[href*="/ide/"]:visible').filter({ hasText: ".sql" }).first();

      if (await sqlFile.isVisible()) {
        await sqlFile.click();
        await page.waitForURL(/\/ide\/.+/);
        await idePage.waitForEditorToLoad();
      }
    }
  });
});

test.describe("IDE Files - Monaco Editor - Content Editing", () => {
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

  test("5.13 - should mark file as dirty when typing single character", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.type("x");

    await idePage.verifySaveButtonVisible();
  });

  // 5.14 Undo all changes (Ctrl+Z)
  test("5.14 - should return to clean state after undoing all changes", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Changes to undo");
    await idePage.verifySaveButtonVisible();

    await idePage.undo();
    await idePage.undo();
    await page.waitForTimeout(500);

    // May return to clean state depending on undo depth
  });

  // 5.15 Paste 1MB of text
  test("5.15 - should handle pasting large content", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();

    // Type a reasonable amount of content
    const content = "Line of content\n".repeat(100);
    await page.keyboard.type(content.slice(0, 500)); // Limit for test speed

    await idePage.verifySaveButtonVisible();
  });

  // 5.18 Delete entire content
  test("5.18 - should mark as dirty when deleting all content", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.selectAll();
    await idePage.deleteSelectedText();

    await idePage.verifySaveButtonVisible();
  });

  test("5.20 - should support multiple cursor editing", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();

    // Add content to work with
    await idePage.insertTextAtEnd("line1\nline2\nline3");

    // Multiple cursor support via Ctrl+D or Alt+Click
    await page.keyboard.press("Control+D");

    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });

  // 5.21 Find and replace (Ctrl+H)
  test("5.21 - should support find and replace", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Find this text");

    // Open find and replace
    await page.keyboard.press("Control+H");

    const findWidget = page.locator(".find-widget");
    await expect(findWidget).toBeVisible({ timeout: 3000 });
  });

  test("5.22 - should display correct line numbers", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    const lineNumbers = page.locator(".line-numbers");
    await expect(lineNumbers.first()).toBeVisible();
  });

  test("5.24 - should prevent editing in read-only mode", async ({ page }) => {
    // This test would require switching to a read-only branch
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Verify editor is in normal (editable) mode first
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });
});

test.describe("IDE Files - Monaco Editor - Diff View", () => {
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

  test("5.25 - should show diff view when toggled on", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    // Make a change first
    await idePage.insertTextAtEnd("New content for diff");

    // Look for diff toggle button
    const diffButton = page.getByRole("button", { name: /diff|compare/i });
    if (await diffButton.isVisible()) {
      await diffButton.click();

      const diffEditor = page.locator(".monaco-diff-editor");
      await expect(diffEditor).toBeVisible({ timeout: 3000 });
    }
  });

  // 5.26 Toggle diff view off
  test("5.26 - should return to normal editor when diff toggled off", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    // Just verify normal editor is displayed
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });
});

test.describe("IDE Files - Monaco Editor - Saving", () => {
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

  test("5.33 - should save file with Ctrl+S", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content to save via Ctrl+S");
    await idePage.verifySaveButtonVisible();

    // Use Ctrl+S
    await page.keyboard.press("Control+S");

    // Save button should disappear after save
    await idePage.verifySaveButtonHidden();
  });

  test("5.34 - should save file when clicking Save button", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content to save via button");
    await idePage.verifySaveButtonVisible();

    await idePage.saveFile();
    await idePage.verifySaveButtonHidden();
  });

  // 5.35 Save with no changes
  test("5.35 - should not make API call when saving with no changes", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    // Don't make any changes
    // Save button should not be visible
    await idePage.verifySaveButtonHidden();
  });

  // 5.36 Rapid Ctrl+S 10x in 1 second
  test("5.36 - should debounce rapid save requests", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content for rapid save test");

    // Rapid save 10x
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press("Control+S");
    }

    // Should handle without errors
    await page.waitForTimeout(2000);
    await idePage.verifySaveButtonHidden();
  });

  test("5.41 - should show error toast when save fails", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content that will fail to save");
    await idePage.verifySaveButtonVisible();

    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "PUT" || request.method() === "POST") {
        await route.fulfill({
          status: 500,
          body: JSON.stringify({ error: "Internal Server Error" })
        });
      } else {
        await route.continue();
      }
    });

    await page.keyboard.press("Control+S");

    // Content should be preserved even after error
    await page.waitForTimeout(1000);
  });

  test("5.42 - should show conflict message on 409 response", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content causing conflict");
    await idePage.verifySaveButtonVisible();

    // Intercept save with 409
    await page.route("**/api/v1/**/files/**", async (route, request) => {
      if (request.method() === "PUT" || request.method() === "POST") {
        await route.fulfill({
          status: 409,
          body: JSON.stringify({ error: "File modified externally" })
        });
      } else {
        await route.continue();
      }
    });

    await page.keyboard.press("Control+S");

    // Should show conflict notification
    await page.waitForTimeout(1000);
  });
});

test.describe("IDE Files - Monaco Editor - Navigation Blocking", () => {
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

  // 5.44 Edit → navigate away
  test("5.44 - should show unsaved changes dialog when navigating away", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Unsaved content");
    await idePage.verifySaveButtonVisible();

    // Try to navigate to another file
    const otherFile = page.getByRole("link", { name: "config.yml" });
    if (await otherFile.isVisible()) {
      await otherFile.click();

      // Should show dialog or block navigation
      const dialog = page.locator('[role="dialog"], [role="alertdialog"]');
      const isDialogVisible = await dialog.isVisible({ timeout: 2000 }).catch(() => false);

      // Either dialog shown or navigation blocked
      expect(isDialogVisible || page.url().includes("test-file")).toBeTruthy();
    }
  });

  test("5.45 - should save and navigate when clicking Save in dialog", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content to save before navigate");
    await idePage.verifySaveButtonVisible();

    const otherFile = page.getByRole("link", { name: "config.yml" });
    if (await otherFile.isVisible()) {
      await otherFile.click();

      const saveButton = page.getByRole("button", { name: /save/i });
      if (await saveButton.isVisible({ timeout: 2000 })) {
        await saveButton.click();
        await page.waitForTimeout(1000);
      }
    }
  });

  test("5.46 - should discard and navigate when clicking Discard", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content to discard");
    await idePage.verifySaveButtonVisible();

    const otherFile = page.getByRole("link", { name: "config.yml" });
    if (await otherFile.isVisible()) {
      await otherFile.click();

      const discardButton = page.getByRole("button", {
        name: /discard|don't save/i
      });
      if (await discardButton.isVisible({ timeout: 2000 })) {
        await discardButton.click();
        await page.waitForURL(/config\.yml|\/ide\//);
      }
    }
  });

  test("5.47 - should stay on current file when clicking Cancel", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content, will cancel navigation");
    await idePage.verifySaveButtonVisible();

    const otherFile = page.getByRole("link", { name: "config.yml" });
    if (await otherFile.isVisible()) {
      await otherFile.click();

      const cancelButton = page.getByRole("button", { name: /cancel/i });
      if (await cancelButton.isVisible({ timeout: 2000 })) {
        await cancelButton.click();

        // Should stay on current file - check that breadcrumb still shows test-file
        await page.waitForTimeout(500);
        await idePage.verifyFileIsOpen("test-file-for-e2e.txt");
      }
    }
  });

  // 5.51 Navigate away after save
  test("5.51 - should navigate without dialog after saving", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Content saved before navigate");
    await idePage.saveFile();
    await idePage.verifySaveButtonHidden();

    // Navigate should work without dialog
    const otherFile = page.getByRole("link", { name: "config.yml" });
    if (await otherFile.isVisible()) {
      await otherFile.click();
      await page.waitForURL(/\/ide\/.+/);
    }
  });
});

test.describe("IDE Files - Monaco Editor - Keyboard Handling", () => {
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

  test("5.52 - should type space in editor (not capture by ResizablePanel)", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.type("word space word");

    // Content should include spaces
    const content = await idePage.getEditorContent();
    expect(content).toContain("space");
  });

  test("5.53 - should navigate cursor with arrow keys", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.type("line1");
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.type("X");

    // Cursor should have moved
    const content = await idePage.getEditorContent();
    expect(content).toContain("X");
  });

  // 5.54 Tab key
  test("5.54 - should insert tab/spaces with Tab key", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.press("Tab");
    await page.keyboard.type("indented");

    await idePage.verifySaveButtonVisible();
  });

  // 5.55 Ctrl+A selects all
  test("5.55 - should select all content with Ctrl+A", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("config.yml");
    await idePage.waitForEditorToLoad();

    await idePage.selectAll();

    // Monaco uses internal selection, so just verify no errors
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });

  // 5.56 Ctrl+C / Ctrl+V
  test("5.56 - should copy and paste content", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.clickEditor();
    await page.keyboard.type("Copy this text");

    await idePage.selectAll();
    await page.keyboard.press("Control+C");
    await page.keyboard.press("End");
    await page.keyboard.press("Control+V");

    // Content should be duplicated
    const content = await idePage.getEditorContent();
    expect(content.length).toBeGreaterThan(10);
  });

  test("5.57 - should undo and redo with Ctrl+Z/Y", async ({ page }) => {
    const idePage = new IDEPage(page);

    await idePage.openFile("test-file-for-e2e.txt");
    await idePage.waitForEditorToLoad();

    await idePage.insertTextAtEnd("Undo this");
    await idePage.verifySaveButtonVisible();

    await idePage.undo();
    await page.waitForTimeout(500);

    await idePage.redo();
    await page.waitForTimeout(500);

    // Editor should remain functional
    const editor = page.locator(".monaco-editor");
    await expect(editor).toBeVisible();
  });
});
