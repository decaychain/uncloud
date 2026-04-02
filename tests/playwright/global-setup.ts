/**
 * Global setup: register the E2E test user via the API.
 * Runs once before all tests. Idempotent — if the user already exists
 * (409 or similar), it continues without error.
 */
async function globalSetup() {
  const baseURL = process.env.BASE_URL || "http://localhost:3000";
  const apiURL = `${baseURL}/api`;

  const response = await fetch(`${apiURL}/auth/register`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      username: "e2euser",
      email: "e2e@test.local",
      password: "TestPassword123!",
    }),
  });

  if (response.ok) {
    console.log("Global setup: test user registered successfully");
  } else if (response.status === 409 || response.status === 400) {
    // User already exists — that's fine
    console.log("Global setup: test user already exists, continuing");
  } else {
    const body = await response.text();
    throw new Error(
      `Global setup: failed to register test user (HTTP ${response.status}): ${body}`
    );
  }
}

export default globalSetup;
