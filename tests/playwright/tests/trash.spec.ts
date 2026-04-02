import { test, expect } from "@playwright/test";
import { randomUUID } from "crypto";
import path from "path";
import fs from "fs";

/**
 * Helper: upload a file via the API (multipart POST to /api/uploads/simple).
 * Returns the created file's response JSON.
 */
async function uploadFileViaAPI(
  request: ReturnType<typeof test.info>["_"] extends never
    ? never
    : Awaited<ReturnType<typeof import("@playwright/test").request.newContext>>,
  fileName: string,
  content: string,
  parentId?: string
): Promise<{ id: string; name: string }> {
  const formData = request.constructor
    ? undefined
    : undefined;

  // Use Playwright's request context multipart support
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

  const response = await request.post("/api/uploads/simple", {
    multipart,
  });

  expect(response.ok(), `Upload of ${fileName} should succeed`).toBeTruthy();
  return response.json();
}

/**
 * Helper: delete a file via the API.
 */
async function deleteFileViaAPI(
  request: any,
  fileId: string
): Promise<void> {
  const response = await request.delete(`/api/files/${fileId}`);
  expect(
    response.ok(),
    `Delete of file ${fileId} should succeed`
  ).toBeTruthy();
}

test.describe("Trash conflict resolution", () => {
  // Use a unique filename per test run to avoid interference
  const testFileBase = `e2e-trash-test`;

  test("restore from trash with name conflict shows rename prompt, accept suggestion", async ({
    page,
    request,
  }) => {
    const uniqueName = `${testFileBase}-${randomUUID().slice(0, 8)}.txt`;

    // Step 1: Upload a file via API
    const file1 = await uploadFileViaAPI(
      request,
      uniqueName,
      "original content"
    );
    expect(file1.id).toBeTruthy();

    // Step 2: Delete it via the UI context menu
    await page.goto("/");
    // Wait for the file browser to load and show our file
    await page.waitForSelector(`[id="file-${file1.id}"]`, { timeout: 10000 });

    // Right-click the file to open context menu
    await page.locator(`[id="file-${file1.id}"]`).click({ button: "right" });

    // Click "Delete" in the context menu (the text-error styled link)
    await page
      .locator("ul.menu li a.text-error", { hasText: "Delete" })
      .click();

    // Confirm deletion in the modal — button text is "Delete"
    await page
      .locator(".modal.modal-open .btn-error", { hasText: "Delete" })
      .click();

    // Wait for the file to disappear from the browser
    await expect(page.locator(`[id="file-${file1.id}"]`)).toBeHidden({
      timeout: 10000,
    });

    // Step 3: Upload a new file with the same name
    const file2 = await uploadFileViaAPI(
      request,
      uniqueName,
      "replacement content"
    );
    expect(file2.id).toBeTruthy();

    // Step 4: Go to Trash via sidebar
    await page.goto("/trash");
    await page.waitForSelector("table", { timeout: 10000 });

    // Step 5: Click Restore on the original (trashed) file
    // Find the row containing our file name, then click its Restore button
    const trashRow = page.locator("tr", { hasText: uniqueName }).first();
    await expect(trashRow).toBeVisible({ timeout: 5000 });
    await trashRow.getByRole("button", { name: "Restore" }).click();

    // Step 6: Verify the inline conflict row appears
    // The conflict row has class "bg-base-200" and contains "Name taken"
    const conflictRow = page.locator("tr.bg-base-200", {
      hasText: "Name taken",
    });
    await expect(conflictRow).toBeVisible({ timeout: 10000 });

    // Verify the input has a pre-filled suggested name
    const renameInput = conflictRow.locator("input.input-bordered");
    await expect(renameInput).toBeVisible();
    const suggestedName = await renameInput.inputValue();
    expect(suggestedName).toBeTruthy();
    expect(suggestedName).not.toBe(uniqueName); // Should be different from original

    // Step 7: Accept the suggestion by clicking "Restore as..."
    await conflictRow
      .getByRole("button", { name: "Restore as..." })
      .click();

    // Verify navigation to home folder and the file is visible
    await page.waitForURL(/\/$|\/folder\//, { timeout: 10000 });

    // The restored file should be visible with the suggested name
    // Check for a file item containing the suggested name in the page
    await expect(page.locator("body")).toContainText(suggestedName, {
      timeout: 10000,
    });
  });

  test("restore from trash with custom rename", async ({ page, request }) => {
    const uniqueName = `${testFileBase}-${randomUUID().slice(0, 8)}.txt`;
    const customName = `custom-${randomUUID().slice(0, 8)}.txt`;

    // Upload and delete a file via API
    const file1 = await uploadFileViaAPI(
      request,
      uniqueName,
      "original content"
    );
    await deleteFileViaAPI(request, file1.id);

    // Upload a replacement with the same name
    const file2 = await uploadFileViaAPI(
      request,
      uniqueName,
      "replacement content"
    );

    // Go to Trash
    await page.goto("/trash");
    await page.waitForSelector("table", { timeout: 10000 });

    // Click Restore on the trashed file
    const trashRow = page.locator("tr", { hasText: uniqueName }).first();
    await expect(trashRow).toBeVisible({ timeout: 5000 });
    await trashRow.getByRole("button", { name: "Restore" }).click();

    // Wait for conflict row
    const conflictRow = page.locator("tr.bg-base-200", {
      hasText: "Name taken",
    });
    await expect(conflictRow).toBeVisible({ timeout: 10000 });

    // Clear the input and type a custom name
    const renameInput = conflictRow.locator("input.input-bordered");
    await renameInput.clear();
    await renameInput.fill(customName);

    // Click "Restore as..."
    await conflictRow
      .getByRole("button", { name: "Restore as..." })
      .click();

    // Verify navigation to home folder
    await page.waitForURL(/\/$|\/folder\//, { timeout: 10000 });

    // The restored file should be visible with the custom name
    await expect(page.locator("body")).toContainText(customName, {
      timeout: 10000,
    });
  });
});
