import { test as setup, expect } from "@playwright/test";
import path from "path";

const authFile = path.join(__dirname, ".auth", "user.json");

setup("authenticate as e2e user", async ({ page }) => {
  const baseURL = process.env.BASE_URL || "http://localhost:3000";

  // Navigate to the login page
  await page.goto(`${baseURL}/login`);

  // Fill in login form using the actual placeholder text from login.rs
  await page.getByPlaceholder("Enter your username").fill("e2euser");
  await page.getByPlaceholder("Enter your password").fill("TestPassword123!");

  // Submit the form — button text is "Sign in"
  await page.getByRole("button", { name: "Sign in" }).click();

  // Wait for navigation away from /login (path match avoids http://host:80 vs http://host port normalisation)
  await page.waitForURL(/\/$/, { timeout: 15000 });

  // Save the storage state (cookies + localStorage)
  await page.context().storageState({ path: authFile });
});
