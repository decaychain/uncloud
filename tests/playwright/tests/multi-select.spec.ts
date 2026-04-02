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

test.describe("Multi-select operations", () => {
  test("multi-select two files and bulk delete", async ({ page, request }) => {
    const fileName1 = `bulk-del-a-${randomUUID().slice(0, 8)}.txt`;
    const fileName2 = `bulk-del-b-${randomUUID().slice(0, 8)}.txt`;

    // Upload two files via API
    const file1 = await uploadFileViaAPI(request, fileName1, "file a");
    const file2 = await uploadFileViaAPI(request, fileName2, "file b");

    await page.goto("/");

    // Wait for both files to appear
    await page.waitForSelector(`[id="file-${file1.id}"]`, { timeout: 10000 });
    await page.waitForSelector(`[id="file-${file2.id}"]`, { timeout: 10000 });

    // Hover over the first file item to reveal its checkbox, then click it
    await page.locator(`[id="file-${file1.id}"]`).hover();
    await page
      .locator(`[id="file-${file1.id}"] input[type="checkbox"]`)
      .click();

    // Hover over the second file item to reveal its checkbox, then click it
    await page.locator(`[id="file-${file2.id}"]`).hover();
    await page
      .locator(`[id="file-${file2.id}"] input[type="checkbox"]`)
      .click();

    // The selection toolbar should now be visible with a Delete button
    const deleteButton = page.getByRole("button", { name: "Delete" });
    await expect(deleteButton).toBeVisible({ timeout: 10000 });

    // Click Delete in the selection toolbar
    await deleteButton.click();

    // Confirm bulk deletion in the modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await modal.locator(".btn-error", { hasText: "Delete" }).click();

    // Verify both files disappear from the file browser
    await expect(page.locator(`[id="file-${file1.id}"]`)).toBeHidden({
      timeout: 10000,
    });
    await expect(page.locator(`[id="file-${file2.id}"]`)).toBeHidden({
      timeout: 10000,
    });
  });
});
