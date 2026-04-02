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

/**
 * Helper: update a file's content via the API.
 * Uses POST /api/files/{id}/content — archives the current blob as a new version.
 */
async function updateFileContentViaAPI(
  request: any,
  fileId: string,
  fileName: string,
  content: string
): Promise<void> {
  const response = await request.post(`/api/files/${fileId}/content`, {
    multipart: {
      file: {
        name: fileName,
        mimeType: "text/plain",
        buffer: Buffer.from(content),
      },
    },
  });
  expect(
    response.ok(),
    `Update content of file ${fileId} should succeed`
  ).toBeTruthy();
}

test.describe("Version history", () => {
  test("version history shows previous versions after content update", async ({
    page,
    request,
  }) => {
    const fileName = `version-test-${randomUUID().slice(0, 8)}.txt`;

    // Upload the file (creates the initial content)
    const file = await uploadFileViaAPI(request, fileName, "version 1 content");

    // Update the file's content — archives the old blob as v1
    await updateFileContentViaAPI(request, file.id, fileName, "version 2 content");

    await page.goto("/");
    await page.waitForSelector(`[id="file-${file.id}"]`, { timeout: 10000 });

    // Right-click to open context menu
    await page.locator(`[id="file-${file.id}"]`).click({ button: "right" });

    // Click "Version history" in the context menu
    await page.locator("ul.menu li a", { hasText: "Version history" }).click();

    // Wait for the version history modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await expect(modal.locator("h3")).toContainText("Version History", {
      timeout: 5000,
    });

    // Verify at least one version row is shown
    const versionRows = modal.locator("table tbody tr");
    await expect(versionRows.first()).toBeVisible({ timeout: 10000 });

    // Verify "v1" text appears — the archived version
    await expect(modal).toContainText("v1", { timeout: 5000 });
  });

  test("restore a previous version", async ({ page, request }) => {
    const fileName = `restore-ver-${randomUUID().slice(0, 8)}.txt`;

    // Upload the file then update its content to create a version
    const file = await uploadFileViaAPI(request, fileName, "original content");
    await updateFileContentViaAPI(request, file.id, fileName, "updated content");

    await page.goto("/");
    await page.waitForSelector(`[id="file-${file.id}"]`, { timeout: 10000 });

    // Right-click to open context menu
    await page.locator(`[id="file-${file.id}"]`).click({ button: "right" });

    // Click "Version history"
    await page.locator("ul.menu li a", { hasText: "Version history" }).click();

    // Wait for the version history modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await expect(modal.locator("table tbody tr").first()).toBeVisible({
      timeout: 10000,
    });

    // Click the Restore button on the first version row
    const restoreButton = modal.getByRole("button", { name: "Restore" });
    await expect(restoreButton.first()).toBeVisible({ timeout: 10000 });
    await restoreButton.first().click();

    // After restore the modal should close
    await expect(modal).toBeHidden({ timeout: 10000 });

    // The file should still be visible
    await expect(page.locator(`[id="file-${file.id}"]`)).toBeVisible({
      timeout: 10000,
    });
  });
});
