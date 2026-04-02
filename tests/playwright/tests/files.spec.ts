import { test, expect } from "@playwright/test";
import { randomUUID } from "crypto";

/**
 * Helper: upload a file via the API.
 */
async function uploadFileViaAPI(
  request: any,
  fileName: string,
  content: string,
  parentId?: string
): Promise<{ id: string; name: string }> {
  const multipart: Record<string, any> = {
    file: {
      name: fileName,
      mimeType: "text/plain",
      buffer: Buffer.from(content),
    },
  };
  if (parentId) {
    multipart.parent_id = parentId;
  }

  const response = await request.post("/api/uploads/simple", { multipart });
  expect(response.ok(), `Upload of ${fileName} should succeed`).toBeTruthy();
  return response.json();
}

test.describe("File management", () => {
  test("upload file via toolbar button", async ({ page }) => {
    const uniqueName = `upload-test-${randomUUID().slice(0, 8)}.txt`;

    await page.goto("/");

    // Use the hidden file input to simulate upload
    await page.setInputFiles("#uc-file-upload", {
      name: uniqueName,
      mimeType: "text/plain",
      buffer: Buffer.from("hello from playwright"),
    });

    // Wait for the uploaded file to appear in the file browser
    await expect(page.locator("body")).toContainText(uniqueName, {
      timeout: 10000,
    });
  });

  test("rename file via context menu", async ({ page, request }) => {
    const originalName = `rename-orig-${randomUUID().slice(0, 8)}.txt`;
    const newName = `rename-new-${randomUUID().slice(0, 8)}.txt`;

    // Upload via API
    const file = await uploadFileViaAPI(request, originalName, "rename me");

    await page.goto("/");
    await page.waitForSelector(`[id="file-${file.id}"]`, { timeout: 10000 });

    // Right-click to open context menu
    await page.locator(`[id="file-${file.id}"]`).click({ button: "right" });

    // Click Rename
    await page.locator("ul.menu li a", { hasText: "Rename" }).click();

    // Wait for the rename modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await expect(modal.locator("h3")).toContainText("Rename");

    // Clear the input and type the new name
    const input = modal.locator("input.input-bordered");
    await input.clear();
    await input.fill(newName);

    // Click Rename (the submit button is labelled "Rename", not "Save")
    await modal.locator(".modal-action .btn-primary", { hasText: "Rename" }).click();

    // Verify the new name is visible and the old name is gone
    await expect(page.locator("body")).toContainText(newName, {
      timeout: 10000,
    });
    await expect(page.locator(`[id="file-${file.id}"]`)).toContainText(
      newName,
      { timeout: 10000 }
    );
  });

  test("delete file via context menu", async ({ page, request }) => {
    const fileName = `delete-test-${randomUUID().slice(0, 8)}.txt`;

    const file = await uploadFileViaAPI(request, fileName, "delete me");

    await page.goto("/");
    await page.waitForSelector(`[id="file-${file.id}"]`, { timeout: 10000 });

    // Right-click to open context menu
    await page.locator(`[id="file-${file.id}"]`).click({ button: "right" });

    // Click Delete in the context menu
    await page
      .locator("ul.menu li a.text-error", { hasText: "Delete" })
      .click();

    // Confirm deletion in the modal
    await page
      .locator(".modal.modal-open .btn-error", { hasText: "Delete" })
      .click();

    // Verify the file disappears
    await expect(page.locator(`[id="file-${file.id}"]`)).toBeHidden({
      timeout: 10000,
    });
  });

  test("toggle view mode between list and grid", async ({ page, request }) => {
    const fileName = `viewmode-${randomUUID().slice(0, 8)}.txt`;

    await uploadFileViaAPI(request, fileName, "view mode test");

    await page.goto("/");
    await expect(page.locator("body")).toContainText(fileName, {
      timeout: 10000,
    });

    // Switch to list view and verify a table appears
    await page.locator("button[title='List view']").click();
    await expect(page.locator("table")).toBeVisible({ timeout: 5000 });

    // Switch to grid view and verify the table is gone
    await page.locator("button[title='Grid view']").click();
    await expect(page.locator("table")).toBeHidden({ timeout: 5000 });
  });
});
