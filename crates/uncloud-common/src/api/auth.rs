use serde::{Deserialize, Serialize};

use super::preferences::UserPreferences;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Pending,
    Disabled,
}

impl Default for UserStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    Open,
    Approval,
    InviteOnly,
    Disabled,
    Demo,
}

impl Default for RegistrationMode {
    fn default() -> Self {
        Self::Open
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    pub password: String,
    #[serde(default)]
    pub invite_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(flatten)]
    pub user: Option<UserResponse>,
    /// When true, the client must call POST /api/auth/totp/verify with the totp_token.
    #[serde(default)]
    pub totp_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpVerifyRequest {
    pub totp_token: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub otpauth_uri: String,
    pub qr_svg: String,
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpEnableRequest {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpDisableRequest {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    pub role: UserRole,
    #[serde(default)]
    pub status: UserStatus,
    pub quota_bytes: Option<i64>,
    pub used_bytes: i64,
    #[serde(default)]
    pub totp_enabled: bool,
    #[serde(default)]
    pub features_enabled: Vec<String>,
    #[serde(default)]
    pub preferences: UserPreferences,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfoResponse {
    pub registration_mode: RegistrationMode,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteResponse {
    pub id: String,
    pub token: String,
    #[serde(default)]
    pub comment: Option<String>,
    pub role: Option<UserRole>,
    pub expires_at: Option<String>,
    pub used: bool,
    #[serde(default)]
    pub used_by_username: Option<String>,
    #[serde(default)]
    pub used_by_email: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    #[serde(default)]
    pub comment: Option<String>,
    pub role: Option<UserRole>,
    pub expires_in_hours: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteInfoResponse {
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminResetPasswordRequest {
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRoleRequest {
    pub role: UserRole,
}
