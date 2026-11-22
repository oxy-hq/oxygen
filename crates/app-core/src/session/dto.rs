use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserInfo,
    pub orgs: Vec<OrgInfo>,
}

#[derive(Serialize)]
pub struct OrgInfo {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
}

/// Global profile fields. Role / admin status are per-org and live on
/// `OrgInfo` in the login response or on `GET /orgs`. `is_owner` reflects
/// the `OXY_OWNER` allow-list (Oxy staff). `is_app_admin` reflects
/// membership in the `app_admins` table and gates the customer-apps
/// surface.
#[derive(Serialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
    pub is_owner: bool,
    pub is_app_admin: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub exp: usize,
    pub iat: usize,
}
