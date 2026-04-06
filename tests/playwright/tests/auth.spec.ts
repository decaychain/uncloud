import { test, expect } from "@playwright/test";
import { randomUUID } from "crypto";

/**
 * Auth tests run WITHOUT the shared storage state — each test starts
 * from a fresh, unauthenticated browser context.
 */
test.use({ storageState: { cookies: [], origins: [] } });

const BASE_URL = process.env.BASE_URL || "http://localhost:3000";

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

test.describe("Registration", () => {
  test("register a new user via the form", async ({ page }) => {
    const username = `reg-${randomUUID().slice(0, 8)}`;

    await page.goto("/register");

    await page.getByPlaceholder("Choose a username").fill(username);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("TestPassword123!");

    await page.getByRole("button", { name: "Create account" }).click();

    // Successful registration in Open mode redirects to home
    await page.waitForURL(/\/$/, { timeout: 15000 });

    // The sidebar or navbar should show the username
    await expect(page.locator("body")).toContainText(username, {
      timeout: 10000,
    });
  });

  test("register with email", async ({ page }) => {
    const username = `reg-email-${randomUUID().slice(0, 8)}`;
    const email = `${username}@test.local`;

    await page.goto("/register");

    await page.getByPlaceholder("Choose a username").fill(username);
    await page.getByPlaceholder("Enter your email").fill(email);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("TestPassword123!");

    await page.getByRole("button", { name: "Create account" }).click();

    await page.waitForURL(/\/$/, { timeout: 15000 });
    await expect(page.locator("body")).toContainText(username, {
      timeout: 10000,
    });
  });

  test("registration shows error for duplicate username", async ({ page }) => {
    // Register a user first via API
    const username = `dup-${randomUUID().slice(0, 8)}`;
    await fetch(`${BASE_URL}/api/auth/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        username,
        password: "TestPassword123!",
      }),
    });

    // Try to register the same username via the form
    await page.goto("/register");

    await page.getByPlaceholder("Choose a username").fill(username);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("TestPassword123!");

    await page.getByRole("button", { name: "Create account" }).click();

    // Should show an error alert
    await expect(page.locator(".alert-error")).toBeVisible({ timeout: 10000 });
  });

  test("password mismatch shows client-side error", async ({ page }) => {
    await page.goto("/register");

    await page
      .getByPlaceholder("Choose a username")
      .fill(`mismatch-${randomUUID().slice(0, 8)}`);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("Different456!");

    await page.getByRole("button", { name: "Create account" }).click();

    // Should stay on register page with an error
    await expect(page.locator(".alert-error")).toBeVisible({ timeout: 5000 });
    await expect(page.locator(".alert-error")).toContainText("match");
  });
});

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

test.describe("Login", () => {
  test("login with valid credentials", async ({ page }) => {
    // Register a user via API
    const username = `login-${randomUUID().slice(0, 8)}`;
    const password = "TestPassword123!";
    await fetch(`${BASE_URL}/api/auth/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });

    await page.goto("/login");

    await page.getByPlaceholder("Enter your username").fill(username);
    await page.getByPlaceholder("Enter your password").fill(password);
    await page.getByRole("button", { name: "Sign in" }).click();

    await page.waitForURL(/\/$/, { timeout: 15000 });
    await expect(page.locator("body")).toContainText(username, {
      timeout: 10000,
    });
  });

  test("login with wrong password shows error", async ({ page }) => {
    const username = `loginfail-${randomUUID().slice(0, 8)}`;
    await fetch(`${BASE_URL}/api/auth/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password: "TestPassword123!" }),
    });

    await page.goto("/login");

    await page.getByPlaceholder("Enter your username").fill(username);
    await page.getByPlaceholder("Enter your password").fill("wrongpassword");
    await page.getByRole("button", { name: "Sign in" }).click();

    await expect(page.locator(".alert-error")).toBeVisible({ timeout: 10000 });
    // Should stay on login page
    expect(page.url()).toContain("/login");
  });

  test("login page shows register link in open mode", async ({ page }) => {
    await page.goto("/login");

    const registerLink = page.getByRole("link", { name: "Create one" });
    await expect(registerLink).toBeVisible({ timeout: 10000 });
    await registerLink.click();

    await page.waitForURL(/\/register/, { timeout: 5000 });
  });
});

// ---------------------------------------------------------------------------
// Invite registration
// ---------------------------------------------------------------------------

test.describe("Invite registration", () => {
  /**
   * Helper: create an admin user and get a session cookie.
   * Uses the e2euser (created by global-setup) since it's the first user
   * and is typically admin. If not, we create a fresh admin via the API.
   */
  async function getAdminCookie(): Promise<string> {
    // Login as e2euser (created by global-setup, first user = admin)
    const loginRes = await fetch(`${BASE_URL}/api/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        username: "e2euser",
        password: "TestPassword123!",
      }),
    });

    if (!loginRes.ok) {
      throw new Error(
        `Admin login failed: ${loginRes.status} ${await loginRes.text()}`
      );
    }

    const setCookie = loginRes.headers.get("set-cookie");
    if (!setCookie) throw new Error("No session cookie in login response");

    // Extract just the session=... part
    const match = setCookie.match(/session=([^;]+)/);
    if (!match) throw new Error("Could not parse session cookie");

    return `session=${match[1]}`;
  }

  test("register via invite link", async ({ page }) => {
    const cookie = await getAdminCookie();

    // Create an invite
    const inviteRes = await fetch(`${BASE_URL}/api/admin/invites`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Cookie: cookie,
      },
      body: JSON.stringify({ comment: "Playwright test" }),
    });

    expect(inviteRes.ok, "Create invite should succeed").toBeTruthy();
    const invite = await inviteRes.json();
    const token = invite.token;
    expect(token).toBeTruthy();

    // Visit the invite registration page
    const username = `invited-${randomUUID().slice(0, 8)}`;
    await page.goto(`/invite/${token}`);

    // Should show the registration form (not "Invalid Invite")
    await expect(
      page.getByRole("heading", { name: "Create account" })
    ).toBeVisible({ timeout: 10000 });

    await page.getByPlaceholder("Choose a username").fill(username);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("TestPassword123!");

    await page.getByRole("button", { name: "Create account" }).click();

    // Should redirect to home after successful registration
    await page.waitForURL(/\/$/, { timeout: 15000 });
    await expect(page.locator("body")).toContainText(username, {
      timeout: 10000,
    });
  });

  test("invalid invite token shows error", async ({ page }) => {
    await page.goto("/invite/invalid-token-12345");

    // Should show the "Invalid Invite" message
    await expect(
      page.getByRole("heading", { name: "Invalid Invite" })
    ).toBeVisible({ timeout: 10000 });
  });
});

// ---------------------------------------------------------------------------
// Session / logout
// ---------------------------------------------------------------------------

test.describe("Session", () => {
  test("register then logout returns to login page", async ({ page }) => {
    const username = `sess-${randomUUID().slice(0, 8)}`;

    // Register via UI
    await page.goto("/register");
    await page.getByPlaceholder("Choose a username").fill(username);
    await page.getByPlaceholder("Choose a password").fill("TestPassword123!");
    await page.getByPlaceholder("Confirm your password").fill("TestPassword123!");
    await page.getByRole("button", { name: "Create account" }).click();

    await page.waitForURL(/\/$/, { timeout: 15000 });

    // Open the user dropdown in the navbar and click "Sign out"
    await page.locator(".dropdown .avatar").click();
    await page.getByText("Sign out").click();

    // Should redirect to login
    await page.waitForURL(/\/login/, { timeout: 10000 });
  });

  test("unauthenticated user is redirected to login", async ({ page }) => {
    // Navigate to a protected route without auth
    await page.goto("/");

    // The app checks /api/auth/me, finds no session, and redirects to /login
    await page.waitForURL(/\/login/, { timeout: 15000 });
    await expect(
      page.getByRole("heading", { name: "Welcome back" })
    ).toBeVisible({ timeout: 5000 });
  });
});
