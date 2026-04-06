/**
 * Global setup: register the E2E test user via the API and ensure they are admin.
 * Runs once before all tests. Idempotent — if the user already exists
 * (409 or similar), it continues without error.
 */
import { MongoClient } from "mongodb";

async function globalSetup() {
  const baseURL = process.env.BASE_URL || "http://localhost:3000";
  const mongoURI = process.env.MONGO_URI || "mongodb://localhost:27017";
  const mongoDBName = process.env.MONGO_DB || "uncloud";
  const apiURL = `${baseURL}/api`;

  // 1. Register the e2e user (idempotent)
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
    console.log("Global setup: test user already exists, continuing");
  } else {
    const body = await response.text();
    throw new Error(
      `Global setup: failed to register test user (HTTP ${response.status}): ${body}`
    );
  }

  // 2. Promote e2euser to admin via MongoDB (idempotent)
  const client = new MongoClient(mongoURI);
  try {
    await client.connect();
    const db = client.db(mongoDBName);
    const result = await db
      .collection("users")
      .updateOne({ username: "e2euser" }, { $set: { role: "admin" } });

    if (result.matchedCount === 1) {
      console.log("Global setup: e2euser promoted to admin");
    } else {
      console.warn(
        "Global setup: e2euser not found in DB — admin promotion skipped"
      );
    }
  } finally {
    await client.close();
  }
}

export default globalSetup;
