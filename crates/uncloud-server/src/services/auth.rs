use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use mongodb::{
    bson::{doc, oid::ObjectId},
    Collection, Database,
};
use rand::RngCore;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::config::{AuthConfig, RegistrationMode};
use crate::error::{AppError, Result};
use crate::models::{Invite, Session, TotpChallenge, User, UserRole, UserStatus};

// ── Return types for login ───────────────────────────────────────────────────

pub enum LoginOutcome {
    /// Login complete — session issued.
    Success(User, Session),
    /// Password correct but TOTP required — client must verify.
    TotpRequired { totp_token: String },
}

// ── TOTP setup data ──────────────────────────────────────────────────────────

pub struct TotpSetupData {
    pub secret: String,
    pub otpauth_uri: String,
    pub qr_svg: String,
    pub recovery_codes: Vec<String>,
}

pub struct AuthService {
    users: Collection<User>,
    sessions: Collection<Session>,
    invites: Collection<Invite>,
    totp_challenges: Collection<TotpChallenge>,
    config: AuthConfig,
}

impl AuthService {
    pub fn new(db: &Database, config: AuthConfig) -> Self {
        Self {
            users: db.collection("users"),
            sessions: db.collection("sessions"),
            invites: db.collection("invites"),
            totp_challenges: db.collection("totp_challenges"),
            config,
        }
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    // ── Password hashing ─────────────────────────────────────────────────────

    pub fn hash_password(&self, password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(format!("Failed to hash password: {}", e)))
    }

    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| AppError::Internal(format!("Invalid password hash: {}", e)))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    fn generate_token(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    // ── Registration ─────────────────────────────────────────────────────────

    /// Public registration. Respects the configured registration mode.
    /// If `invite_token` is provided, validates and consumes it.
    pub async fn register(
        &self,
        username: String,
        email: Option<String>,
        password: String,
        invite_token: Option<String>,
    ) -> Result<User> {
        let invite = if let Some(ref token) = invite_token {
            Some(self.validate_invite(token).await?)
        } else {
            None
        };

        // Determine whether registration is allowed and what status the user gets.
        let status = match self.config.registration {
            RegistrationMode::Disabled => {
                return Err(AppError::Forbidden);
            }
            RegistrationMode::InviteOnly => {
                if invite.is_none() {
                    return Err(AppError::Forbidden);
                }
                UserStatus::Active // invite present → immediate activation
            }
            RegistrationMode::Approval => {
                if invite.is_some() {
                    UserStatus::Active // admin invited them — skip approval
                } else {
                    UserStatus::Pending
                }
            }
            RegistrationMode::Open | RegistrationMode::Demo => UserStatus::Active,
        };

        self.create_user_internal(username, email, password, UserRole::User, status, invite)
            .await
    }

    /// Internal user creation with validation, used by both public register and admin create.
    async fn create_user_internal(
        &self,
        username: String,
        email: Option<String>,
        password: String,
        role: UserRole,
        status: UserStatus,
        invite: Option<Invite>,
    ) -> Result<User> {
        // Validate input
        if username.len() < 3 || username.len() > 32 {
            return Err(AppError::Validation(
                "Username must be between 3 and 32 characters".to_string(),
            ));
        }
        if password.len() < 8 {
            return Err(AppError::Validation(
                "Password must be at least 8 characters".to_string(),
            ));
        }

        // Normalize: treat empty/whitespace-only email as None
        let email = email.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());

        if let Some(ref email) = email {
            if !email.contains('@') {
                return Err(AppError::Validation("Invalid email address".to_string()));
            }
        }

        // Check for existing user
        if self
            .users
            .find_one(doc! { "username": &username })
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("Username already taken".to_string()));
        }
        if let Some(ref email) = email {
            if self
                .users
                .find_one(doc! { "email": email })
                .await?
                .is_some()
            {
                return Err(AppError::Conflict("Email already registered".to_string()));
            }
        }

        let password_hash = self.hash_password(&password)?;
        let mut user = User::new(username, email, password_hash);
        user.role = if let Some(ref inv) = invite {
            inv.role.unwrap_or(role)
        } else {
            role
        };
        user.status = status;

        self.users.insert_one(&user).await?;

        // Consume invite if one was used
        if let Some(inv) = invite {
            self.consume_invite(&inv.token, user.id).await?;
        }

        Ok(user)
    }

    // ── Login ────────────────────────────────────────────────────────────────

    pub async fn login(
        &self,
        username_or_email: &str,
        password: &str,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<LoginOutcome> {
        // Find user by username or email
        let user = self
            .users
            .find_one(doc! {
                "$or": [
                    { "username": username_or_email },
                    { "email": username_or_email }
                ]
            })
            .await?
            .ok_or(AppError::Unauthorized)?;

        // Check user status
        match user.status {
            UserStatus::Pending => {
                return Err(AppError::Forbidden);
            }
            UserStatus::Disabled => {
                return Err(AppError::Forbidden);
            }
            UserStatus::Active => {}
        }

        // Verify password
        if !self.verify_password(password, &user.password_hash)? {
            return Err(AppError::Unauthorized);
        }

        // If TOTP is enabled, issue a challenge instead of a session
        if user.totp_enabled {
            let token = self.generate_token();
            let challenge = TotpChallenge::new(token.clone(), user.id);
            self.totp_challenges.insert_one(&challenge).await?;
            return Ok(LoginOutcome::TotpRequired { totp_token: token });
        }

        // Create session
        let token = self.generate_token();
        let session = Session::new(
            token,
            user.id,
            self.config.session_duration_hours,
            user_agent,
            ip_address,
        );
        self.sessions.insert_one(&session).await?;

        Ok(LoginOutcome::Success(user, session))
    }

    // ── Demo login ───────────────────────────────────────────────────────────

    /// Create an ephemeral demo user and return a session.
    pub async fn demo_login(
        &self,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<(User, Session)> {
        if self.config.registration != RegistrationMode::Demo {
            return Err(AppError::Forbidden);
        }

        let demo_id = &self.generate_token()[..8];
        let username = format!("demo-{}", demo_id);
        let password_hash = self.hash_password(&self.generate_token())?; // random, never used

        let mut user = User::new(username, None, password_hash);
        user.demo = true;
        user.quota_bytes = Some(self.config.demo_quota_bytes);

        self.users.insert_one(&user).await?;

        let token = self.generate_token();
        let session = Session::new(
            token,
            user.id,
            self.config.demo_ttl_hours,
            user_agent,
            ip_address,
        );
        self.sessions.insert_one(&session).await?;

        Ok((user, session))
    }

    /// Purge demo accounts older than `demo_ttl_hours`. Called by a background task.
    pub async fn purge_demo_accounts(&self, db: &Database) -> Result<u64> {
        let cutoff = Utc::now()
            - chrono::Duration::hours(self.config.demo_ttl_hours as i64);
        let cutoff_bson = mongodb::bson::DateTime::from_chrono(cutoff);

        let filter = doc! {
            "demo": true,
            "created_at": { "$lt": cutoff_bson },
        };

        // Find demo users to delete
        let mut cursor = self.users.find(filter.clone()).await?;
        let mut user_ids = Vec::new();
        while cursor.advance().await? {
            let user: User = cursor.deserialize_current()?;
            user_ids.push(user.id);
        }

        if user_ids.is_empty() {
            return Ok(0);
        }

        let ids_bson: Vec<mongodb::bson::Bson> =
            user_ids.iter().map(|id| mongodb::bson::Bson::ObjectId(*id)).collect();

        // Delete related data
        db.collection::<mongodb::bson::Document>("files")
            .delete_many(doc! { "owner_id": { "$in": &ids_bson } })
            .await?;
        db.collection::<mongodb::bson::Document>("folders")
            .delete_many(doc! { "owner_id": { "$in": &ids_bson } })
            .await?;
        db.collection::<mongodb::bson::Document>("sessions")
            .delete_many(doc! { "user_id": { "$in": &ids_bson } })
            .await?;
        db.collection::<mongodb::bson::Document>("shares")
            .delete_many(doc! { "owner_id": { "$in": &ids_bson } })
            .await?;

        let result = self.users.delete_many(filter).await?;
        Ok(result.deleted_count)
    }

    // ── Session / logout ─────────────────────────────────────────────────────

    pub async fn create_session(
        &self,
        user_id: ObjectId,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<Session> {
        let token = self.generate_token();
        let session = Session::new(
            token,
            user_id,
            self.config.session_duration_hours,
            user_agent,
            ip_address,
        );
        self.sessions.insert_one(&session).await?;
        Ok(session)
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        self.sessions.delete_one(doc! { "token": token }).await?;
        Ok(())
    }

    pub async fn validate_session(&self, token: &str) -> Result<(User, Session)> {
        let session = self
            .sessions
            .find_one(doc! { "token": token })
            .await?
            .ok_or(AppError::Unauthorized)?;

        if session.is_expired() {
            self.sessions
                .delete_one(doc! { "_id": session.id })
                .await?;
            return Err(AppError::Unauthorized);
        }

        let user = self
            .users
            .find_one(doc! { "_id": session.user_id })
            .await?
            .ok_or(AppError::Unauthorized)?;

        // Reject disabled/pending users even if they have a valid session
        if user.status != UserStatus::Active {
            return Err(AppError::Unauthorized);
        }

        Ok((user, session))
    }

    pub async fn get_user_sessions(&self, user_id: ObjectId) -> Result<Vec<Session>> {
        let mut cursor = self.sessions.find(doc! { "user_id": user_id }).await?;
        let mut sessions = Vec::new();
        while cursor.advance().await? {
            sessions.push(cursor.deserialize_current()?);
        }
        Ok(sessions)
    }

    pub async fn revoke_session(&self, user_id: ObjectId, session_id: ObjectId) -> Result<()> {
        let result = self
            .sessions
            .delete_one(doc! { "_id": session_id, "user_id": user_id })
            .await?;

        if result.deleted_count == 0 {
            return Err(AppError::NotFound("Session not found".to_string()));
        }
        Ok(())
    }

    // ── User queries ─────────────────────────────────────────────────────────

    pub async fn get_user_by_id(&self, user_id: ObjectId) -> Result<Option<User>> {
        Ok(self.users.find_one(doc! { "_id": user_id }).await?)
    }

    pub async fn update_user_bytes(&self, user_id: ObjectId, delta: i64) -> Result<()> {
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$inc": { "used_bytes": delta } },
            )
            .await?;
        Ok(())
    }

    pub fn registration_mode(&self) -> RegistrationMode {
        self.config.registration
    }

    // ── Password management ────────────────────────────────────────────────────

    /// Change password for the authenticated user (requires current password).
    pub async fn change_password(
        &self,
        user_id: ObjectId,
        current_password: &str,
        new_password: &str,
    ) -> Result<()> {
        if new_password.len() < 8 {
            return Err(AppError::Validation(
                "Password must be at least 8 characters".to_string(),
            ));
        }

        let user = self
            .users
            .find_one(doc! { "_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        if !self.verify_password(current_password, &user.password_hash)? {
            return Err(AppError::BadRequest(
                "Current password is incorrect".to_string(),
            ));
        }

        let new_hash = self.hash_password(new_password)?;
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "password_hash": new_hash,
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;

        Ok(())
    }

    /// Admin: reset a user's password without knowing the current one.
    pub async fn admin_reset_password(
        &self,
        user_id: ObjectId,
        new_password: &str,
    ) -> Result<()> {
        if new_password.len() < 8 {
            return Err(AppError::Validation(
                "Password must be at least 8 characters".to_string(),
            ));
        }

        let new_hash = self.hash_password(new_password)?;
        let result = self
            .users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "password_hash": new_hash,
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;

        if result.matched_count == 0 {
            return Err(AppError::NotFound("User not found".to_string()));
        }

        Ok(())
    }

    /// Admin: change a user's role.
    pub async fn change_role(
        &self,
        user_id: ObjectId,
        role: UserRole,
    ) -> Result<()> {
        let role_str = serde_json::to_value(role)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let result = self
            .users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "role": role_str.as_str().unwrap_or("user"),
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;

        if result.matched_count == 0 {
            return Err(AppError::NotFound("User not found".to_string()));
        }

        Ok(())
    }

    // ── User status management (admin) ───────────────────────────────────────

    pub async fn approve_user(&self, user_id: ObjectId) -> Result<()> {
        self.set_user_status(user_id, UserStatus::Active).await
    }

    pub async fn disable_user(&self, user_id: ObjectId) -> Result<()> {
        self.set_user_status(user_id, UserStatus::Disabled).await
    }

    pub async fn enable_user(&self, user_id: ObjectId) -> Result<()> {
        self.set_user_status(user_id, UserStatus::Active).await
    }

    async fn set_user_status(&self, user_id: ObjectId, status: UserStatus) -> Result<()> {
        let status_str = serde_json::to_value(status)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "status": status_str.as_str().unwrap_or("active"),
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;
        Ok(())
    }

    // ── Invite management ────────────────────────────────────────────────────

    pub async fn create_invite(
        &self,
        created_by: ObjectId,
        comment: Option<String>,
        role: Option<UserRole>,
        expires_in_hours: Option<u64>,
    ) -> Result<Invite> {
        let token = self.generate_token();
        let expires_at =
            expires_in_hours.map(|h| Utc::now() + chrono::Duration::hours(h as i64));

        let invite = Invite::new(token, created_by, comment, role, expires_at);
        self.invites.insert_one(&invite).await?;
        Ok(invite)
    }

    pub async fn list_invites(&self) -> Result<Vec<Invite>> {
        let mut cursor = self.invites.find(doc! {}).await?;
        let mut invites = Vec::new();
        while cursor.advance().await? {
            invites.push(cursor.deserialize_current()?);
        }
        Ok(invites)
    }

    pub async fn delete_invite(&self, invite_id: ObjectId) -> Result<()> {
        let result = self
            .invites
            .delete_one(doc! { "_id": invite_id })
            .await?;
        if result.deleted_count == 0 {
            return Err(AppError::NotFound("Invite not found".to_string()));
        }
        Ok(())
    }

    pub async fn validate_invite(&self, token: &str) -> Result<Invite> {
        let invite = self
            .invites
            .find_one(doc! { "token": token })
            .await?
            .ok_or_else(|| AppError::NotFound("Invalid invite".to_string()))?;

        if !invite.is_valid() {
            return Err(AppError::BadRequest(
                "Invite has expired or already been used".to_string(),
            ));
        }
        Ok(invite)
    }

    pub async fn get_invite_info(&self, token: &str) -> Result<Option<Invite>> {
        Ok(self.invites.find_one(doc! { "token": token }).await?)
    }

    async fn consume_invite(&self, token: &str, user_id: ObjectId) -> Result<()> {
        self.invites
            .update_one(
                doc! { "token": token },
                doc! { "$set": {
                    "used_by": user_id,
                    "used_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;
        Ok(())
    }

    // ── TOTP ─────────────────────────────────────────────────────────────────

    /// Generate a TOTP secret and recovery codes. Stores the secret on the user
    /// but does NOT enable TOTP yet (call `enable_totp` after user verifies).
    pub async fn setup_totp(&self, user_id: ObjectId) -> Result<TotpSetupData> {
        let user = self
            .users
            .find_one(doc! { "_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        if user.totp_enabled {
            return Err(AppError::BadRequest("TOTP is already enabled".to_string()));
        }

        // Generate a random 160-bit secret
        let secret = Secret::generate_secret();
        let secret_base32 = secret.to_encoded().to_string();

        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret.to_bytes().map_err(|e| AppError::Internal(e.to_string()))?,
            Some("Uncloud".to_string()),
            user.username.clone(),
        )
        .map_err(|e| AppError::Internal(format!("TOTP creation failed: {}", e)))?;

        let otpauth_uri = totp.get_url();
        let qr_svg = totp
            .get_qr_png()
            .map_err(|e| AppError::Internal(format!("QR generation failed: {}", e)))?;
        let qr_base64 = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&qr_svg)
        );

        // Generate 8 recovery codes
        let mut recovery_codes_plain = Vec::new();
        let mut recovery_codes_hashed = Vec::new();
        for _ in 0..8 {
            let code = self.generate_recovery_code();
            let hash = self.hash_password(&code)?;
            recovery_codes_plain.push(code);
            recovery_codes_hashed.push(hash);
        }

        // Store secret and hashed recovery codes (but don't enable yet)
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "totp_secret": &secret_base32,
                    "recovery_codes": &recovery_codes_hashed,
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;

        Ok(TotpSetupData {
            secret: secret_base32,
            otpauth_uri,
            qr_svg: qr_base64,
            recovery_codes: recovery_codes_plain,
        })
    }

    /// Verify a TOTP code against the user's stored secret and enable TOTP.
    pub async fn enable_totp(&self, user_id: ObjectId, code: &str) -> Result<()> {
        let user = self
            .users
            .find_one(doc! { "_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        let secret_b32 = user
            .totp_secret
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("TOTP not set up".to_string()))?;

        if !self.verify_totp_code(secret_b32, code)? {
            return Err(AppError::BadRequest("Invalid TOTP code".to_string()));
        }

        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "totp_enabled": true,
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;

        Ok(())
    }

    /// Disable TOTP for a user (requires a valid TOTP code as confirmation).
    pub async fn disable_totp(&self, user_id: ObjectId, code: &str) -> Result<()> {
        let user = self
            .users
            .find_one(doc! { "_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        if !user.totp_enabled {
            return Err(AppError::BadRequest("TOTP is not enabled".to_string()));
        }

        let secret_b32 = user
            .totp_secret
            .as_deref()
            .ok_or_else(|| AppError::Internal("TOTP enabled but no secret".to_string()))?;

        if !self.verify_totp_code(secret_b32, code)? {
            return Err(AppError::BadRequest("Invalid TOTP code".to_string()));
        }

        self.clear_totp(user_id).await
    }

    /// Admin: forcefully reset TOTP for any user (no code required).
    pub async fn admin_reset_totp(&self, user_id: ObjectId) -> Result<()> {
        self.clear_totp(user_id).await
    }

    async fn clear_totp(&self, user_id: ObjectId) -> Result<()> {
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": {
                    "totp_enabled": false,
                    "totp_secret": mongodb::bson::Bson::Null,
                    "recovery_codes": [],
                    "updated_at": mongodb::bson::DateTime::now(),
                }},
            )
            .await?;
        Ok(())
    }

    /// Verify a TOTP challenge token + code, completing the two-step login.
    pub async fn verify_totp_login(
        &self,
        totp_token: &str,
        code: &str,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<(User, Session)> {
        let challenge = self
            .totp_challenges
            .find_one(doc! { "token": totp_token })
            .await?
            .ok_or(AppError::Unauthorized)?;

        if challenge.is_expired() {
            self.totp_challenges
                .delete_one(doc! { "_id": challenge.id })
                .await?;
            return Err(AppError::BadRequest("TOTP challenge expired".to_string()));
        }

        let user = self
            .users
            .find_one(doc! { "_id": challenge.user_id })
            .await?
            .ok_or(AppError::Unauthorized)?;

        let secret_b32 = user
            .totp_secret
            .as_deref()
            .ok_or_else(|| AppError::Internal("TOTP enabled but no secret".to_string()))?;

        // Try TOTP code first, then recovery codes
        let valid = if self.verify_totp_code(secret_b32, code)? {
            true
        } else {
            self.try_recovery_code(user.id, &user.recovery_codes, code)
                .await?
        };

        if !valid {
            return Err(AppError::BadRequest("Invalid TOTP code".to_string()));
        }

        // Delete the challenge
        self.totp_challenges
            .delete_one(doc! { "_id": challenge.id })
            .await?;

        // Create session
        let token = self.generate_token();
        let session = Session::new(
            token,
            user.id,
            self.config.session_duration_hours,
            user_agent,
            ip_address,
        );
        self.sessions.insert_one(&session).await?;

        Ok((user, session))
    }

    fn verify_totp_code(&self, secret_b32: &str, code: &str) -> Result<bool> {
        let secret = Secret::Encoded(secret_b32.to_string());
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret.to_bytes().map_err(|e| AppError::Internal(e.to_string()))?,
            None,
            String::new(),
        )
        .map_err(|e| AppError::Internal(format!("TOTP error: {}", e)))?;

        Ok(totp.check_current(code).unwrap_or(false))
    }

    /// Try each stored recovery code hash. If one matches, remove it.
    async fn try_recovery_code(
        &self,
        user_id: ObjectId,
        hashed_codes: &[String],
        code: &str,
    ) -> Result<bool> {
        for (i, hash) in hashed_codes.iter().enumerate() {
            if self.verify_password(code, hash)? {
                // Remove the used recovery code
                let mut remaining = hashed_codes.to_vec();
                remaining.remove(i);
                self.users
                    .update_one(
                        doc! { "_id": user_id },
                        doc! { "$set": {
                            "recovery_codes": remaining,
                            "updated_at": mongodb::bson::DateTime::now(),
                        }},
                    )
                    .await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn generate_recovery_code(&self) -> String {
        let mut bytes = [0u8; 5]; // 10 hex chars
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }
}
