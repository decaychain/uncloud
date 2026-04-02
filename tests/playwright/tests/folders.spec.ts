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
 * Helper: create a folder via the API.
 */
async function createFolderViaAPI(
  request: any,
  name: string,
  parentId?: string
): Promise<{ id: string; name: string }> {
  const data: Record<string, string> = { name };
  if (parentId) {
    data.parent_id = parentId;
  }

  const response = await request.post("/api/folders", {
    data,
    headers: { "Content-Type": "application/json" },
  });
  expect(
    response.ok(),
    `Create folder ${name} should succeed`
  ).toBeTruthy();
  return response.json();
}

test.describe("Folder management", () => {
  test("create folder via New Folder button", async ({ page }) => {
    const folderName = `new-folder-${randomUUID().slice(0, 8)}`;

    await page.goto("/");

    // Click the "New Folder" button in the toolbar
    await page.getByRole("button", { name: "New Folder" }).click();

    // Wait for the New Folder modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await expect(modal.locator("h3")).toContainText("New Folder");

    // Fill in the folder name
    const input = modal.locator("input.input-bordered");
    await input.fill(folderName);

    // Click Create
    await modal
      .locator(".btn-primary", { hasText: "Create" })
      .click();

    // Verify the folder appears in the file browser
    await expect(page.locator("body")).toContainText(folderName, {
      timeout: 10000,
    });
  });

  test("navigate into folder and back via breadcrumb", async ({
    page,
    request,
  }) => {
    const folderName = `nav-folder-${randomUUID().slice(0, 8)}`;

    // Create folder via API
    const folder = await createFolderViaAPI(request, folderName);

    await page.goto("/");
    await page.waitForSelector(`[id="file-${folder.id}"]`, {
      timeout: 10000,
    });

    // Click the folder to navigate into it
    await page.locator(`[id="file-${folder.id}"]`).click();

    // Verify URL contains the folder ID
    await page.waitForURL(new RegExp(`/folder/${folder.id}`), {
      timeout: 10000,
    });

    // Verify the breadcrumb shows the folder name
    const breadcrumbs = page.locator(".breadcrumbs");
    await expect(breadcrumbs).toContainText(folderName, { timeout: 10000 });

    // Click "Files" in the breadcrumb to go back to root
    await breadcrumbs.locator("li a", { hasText: "Files" }).click();

    // Verify we are back at root (URL should be / or not contain /folder/)
    await page.waitForURL(/\/$/, { timeout: 10000 });

    // The folder should still be visible at root
    await expect(page.locator(`[id="file-${folder.id}"]`)).toBeVisible({
      timeout: 10000,
    });
  });

  test("move file into folder via context menu", async ({
    page,
    request,
  }) => {
    const fileName = `move-file-${randomUUID().slice(0, 8)}.txt`;
    const folderName = `move-target-${randomUUID().slice(0, 8)}`;

    // Create file and folder via API
    const file = await uploadFileViaAPI(request, fileName, "move me");
    const folder = await createFolderViaAPI(request, folderName);

    await page.goto("/");
    await page.waitForSelector(`[id="file-${file.id}"]`, { timeout: 10000 });
    await page.waitForSelector(`[id="file-${folder.id}"]`, {
      timeout: 10000,
    });

    // Right-click the file to open context menu
    await page.locator(`[id="file-${file.id}"]`).click({ button: "right" });

    // Click "Move to..." in the context menu
    await page
      .locator("ul.menu li a", { hasText: "Move to" })
      .click();

    // Wait for the MoveDialog modal
    const modal = page.locator(".modal.modal-open");
    await expect(modal).toBeVisible({ timeout: 10000 });
    await expect(modal.locator("h3")).toContainText("Move", { timeout: 5000 });

    // Click the target folder in the folder picker
    await modal
      .locator("ul.menu li a", { hasText: folderName })
      .click();

    // Click "Move Here"
    await modal
      .locator(".btn-primary", { hasText: "Move Here" })
      .click();

    // Wait for modal to close
    await expect(modal).toBeHidden({ timeout: 10000 });

    // Verify the file is no longer visible at root
    await expect(page.locator(`[id="file-${file.id}"]`)).toBeHidden({
      timeout: 10000,
    });

    // Navigate into the folder to verify the file is there
    await page.locator(`[id="file-${folder.id}"]`).click();
    await page.waitForURL(new RegExp(`/folder/${folder.id}`), {
      timeout: 10000,
    });

    await expect(page.locator(`[id="file-${file.id}"]`)).toBeVisible({
      timeout: 10000,
    });
  });
});
