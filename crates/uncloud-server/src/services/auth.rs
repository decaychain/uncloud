use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mongodb::{bson::doc, Collection, Database};
use rand::RngCore;

use crate::config::AuthConfig;
use crate::error::{AppError, Result};
use crate::models::{Session, User};

pub struct AuthService {
    users: Collection<User>,
    sessions: Collection<Session>,
    config: AuthConfig,
}

impl AuthService {
    pub fn new(db: &Database, config: AuthConfig) -> Self {
        Self {
            users: db.collection("users"),
            sessions: db.collection("sessions"),
            config,
        }
    }

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

    pub async fn register(
        &self,
        username: String,
        email: String,
        password: String,
    ) -> Result<User> {
        if !self.config.registration_enabled {
            return Err(AppError::Forbidden);
        }

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
        if !email.contains('@') {
            return Err(AppError::Validation("Invalid email address".to_string()));
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
        if self
            .users
            .find_one(doc! { "email": &email })
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("Email already registered".to_string()));
        }

        let password_hash = self.hash_password(&password)?;
        let user = User::new(username, email, password_hash);

        self.users.insert_one(&user).await?;
        Ok(user)
    }

    pub async fn login(
        &self,
        username_or_email: &str,
        password: &str,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<(User, Session)> {
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

        // Verify password
        if !self.verify_password(password, &user.password_hash)? {
            return Err(AppError::Unauthorized);
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

        Ok((user, session))
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

        Ok((user, session))
    }

    pub async fn get_user_sessions(
        &self,
        user_id: mongodb::bson::oid::ObjectId,
    ) -> Result<Vec<Session>> {
        let mut cursor = self.sessions.find(doc! { "user_id": user_id }).await?;
        let mut sessions = Vec::new();
        while cursor.advance().await? {
            sessions.push(cursor.deserialize_current()?);
        }
        Ok(sessions)
    }

    pub async fn revoke_session(
        &self,
        user_id: mongodb::bson::oid::ObjectId,
        session_id: mongodb::bson::oid::ObjectId,
    ) -> Result<()> {
        let result = self
            .sessions
            .delete_one(doc! { "_id": session_id, "user_id": user_id })
            .await?;

        if result.deleted_count == 0 {
            return Err(AppError::NotFound("Session not found".to_string()));
        }
        Ok(())
    }

    pub async fn get_user_by_id(
        &self,
        user_id: mongodb::bson::oid::ObjectId,
    ) -> Result<Option<User>> {
        Ok(self.users.find_one(doc! { "_id": user_id }).await?)
    }

    pub async fn update_user_bytes(
        &self,
        user_id: mongodb::bson::oid::ObjectId,
        delta: i64,
    ) -> Result<()> {
        self.users
            .update_one(
                doc! { "_id": user_id },
                doc! { "$inc": { "used_bytes": delta } },
            )
            .await?;
        Ok(())
    }

    pub fn is_registration_enabled(&self) -> bool {
        self.config.registration_enabled
    }
}
