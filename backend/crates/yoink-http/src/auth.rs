use std::sync::{Arc, OnceLock};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, Request, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use yoink_auth::{AuthConfig, JwtProfile};
use yoink_identity::{IdentityError, IdentityStore, TrustedProfile, UserProfile, UserSettings};

/// Routes exist only when explicitly enabled; dependencies are installed after schema checks.
#[derive(Clone)]
pub struct AuthState {
    service: Arc<OnceLock<AuthService>>,
    identities: Arc<OnceLock<Arc<IdentityStore>>>,
    downloader_settings: Arc<OnceLock<yoink_downloads::UserSettingsService>>,
    pub(crate) cookies: Arc<OnceLock<yoink_downloads::DownloadService>>,
    dev_enabled: bool,
}

pub struct AuthService {
    pub identities: Arc<IdentityStore>,
    pub crypto: AuthConfig,
}

impl AuthState {
    pub fn new(dev_enabled: bool) -> Self {
        Self {
            service: Arc::new(OnceLock::new()),
            identities: Arc::new(OnceLock::new()),
            downloader_settings: Arc::new(OnceLock::new()),
            cookies: Arc::new(OnceLock::new()),
            dev_enabled,
        }
    }

    pub fn install_cookies(
        &self,
        service: yoink_downloads::DownloadService,
    ) -> Result<(), Box<yoink_downloads::DownloadService>> {
        self.cookies.set(service).map_err(Box::new)
    }

    pub fn install_downloader_settings(
        &self,
        service: yoink_downloads::UserSettingsService,
    ) -> Result<(), yoink_downloads::UserSettingsService> {
        self.downloader_settings.set(service)
    }
    pub(crate) fn downloader_settings(
        &self,
    ) -> Result<&yoink_downloads::UserSettingsService, ApiError> {
        self.downloader_settings.get().ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Downloader settings unavailable",
        ))
    }

    pub fn install(&self, service: AuthService) -> Result<(), AuthService> {
        self.service.set(service)
    }

    pub fn install_identities(
        &self,
        identities: Arc<IdentityStore>,
    ) -> Result<(), Arc<IdentityStore>> {
        self.identities.set(identities)
    }

    pub(crate) fn identities(&self) -> Result<&Arc<IdentityStore>, ApiError> {
        self.identities.get().ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Authentication unavailable",
        ))
    }

    pub(crate) fn service(&self) -> Result<&AuthService, ApiError> {
        self.service.get().ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Authentication unavailable",
        ))
    }
}

pub(crate) fn router(state: AuthState, system: yoink_application::SystemService) -> Router {
    let mut router = Router::new()
        .route("/api/v1/auth/token", post(token))
        .route("/api/v1/users/me", get(profile))
        .route("/api/v1/settings", get(settings).patch(update_settings));
    if state.dev_enabled {
        router = router.route("/api/v1/auth/dev", post(dev_token));
    }
    router
        .merge(crate::configuration::router())
        .merge(crate::admin::router())
        .merge(crate::groups::router())
        .merge(crate::nsfw::router())
        .merge(crate::downloader_settings::router())
        .merge(crate::download_history::router())
        .merge(crate::cookie_web::router())
        .route_layer(middleware::from_fn(move |request: Request, next: Next| {
            let ready = system.snapshot().ready;
            async move {
                let mut response = if ready {
                    next.run(request).await
                } else {
                    ApiError(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Authentication unavailable",
                    )
                    .into_response()
                };
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                if response.status() == StatusCode::UNAUTHORIZED {
                    response.headers_mut().insert(
                        header::WWW_AUTHENTICATE,
                        header::HeaderValue::from_static("Bearer"),
                    );
                }
                response
            }
        }))
        .layer(DefaultBodyLimit::max(20 * 1024))
        .with_state(state)
}

#[derive(Debug)]
pub(crate) struct ApiError(pub(crate) StatusCode, pub(crate) &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(serde_json::json!({"detail": self.1}))).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        if self.0 == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

impl From<IdentityError> for ApiError {
    fn from(error: IdentityError) -> Self {
        match error {
            IdentityError::Unauthorized => Self(StatusCode::UNAUTHORIZED, "Invalid identity"),
            IdentityError::Forbidden => Self(StatusCode::FORBIDDEN, "Access denied"),
            IdentityError::InvalidId
            | IdentityError::InvalidProfile
            | IdentityError::InvalidLanguage
            | IdentityError::InvalidTheme => {
                Self(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request")
            }
            _ => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
            ),
        }
    }
}

#[derive(Deserialize)]
struct TokenRequest {
    init_data: String,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    user_id: i64,
    role: String,
}

pub(crate) fn no_store<T: Serialize>(value: T) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}

fn issue(service: &AuthService, user: &UserProfile) -> Result<TokenResponse, ApiError> {
    let role = user.role.to_string();
    let access_token = service
        .crypto
        .issue_token(
            JwtProfile {
                user_id: user.id,
                role: &role,
                username: user.username.as_deref(),
                first_name: user.first_name.as_deref(),
            },
            Utc::now().timestamp(),
        )
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Authentication unavailable",
            )
        })?;
    Ok(TokenResponse {
        access_token,
        token_type: "bearer",
        user_id: user.id,
        role,
    })
}

async fn token(
    State(state): State<AuthState>,
    body: Result<Json<TokenRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) =
        body.map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request"))?;
    let service = state.service()?;
    let trusted = service
        .crypto
        .verify_webapp(&body.init_data, Utc::now().timestamp())
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "Invalid Telegram initData"))?;
    let user = service
        .identities
        .register(&TrustedProfile {
            user_id: trusted.user_id,
            username: trusted.username,
            first_name: trusted.first_name,
            photo_url: trusted.photo_url,
        })
        .await?;
    Ok(no_store(issue(service, &user)?))
}

#[derive(Deserialize)]
struct DevRequest {
    user_id: i64,
    role: Option<String>,
}

async fn dev_token(
    State(state): State<AuthState>,
    query: Result<Query<DevRequest>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    if !state.dev_enabled {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found"));
    }
    let Query(body) =
        query.map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request"))?;
    if body
        .role
        .as_deref()
        .is_some_and(|role| role.parse::<yoink_access::Role>().is_err())
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
        ));
    }
    let service = state.service()?;
    // The query role is accepted for compatibility, never used to elevate authority.
    let user = service.identities.current_profile(body.user_id).await?;
    Ok(no_store(issue(service, &user)?))
}

pub(crate) fn authenticated_id(
    service: &AuthService,
    headers: &HeaderMap,
) -> Result<i64, ApiError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "Invalid token"))?;
    if values.next().is_some() {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "Invalid token"));
    }
    let (scheme, token) = value
        .split_once(' ')
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "Invalid token"))?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "Invalid token"));
    }
    service
        .crypto
        .verify_token(token, Utc::now().timestamp())
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "Invalid token"))
}

#[derive(Serialize)]
struct ProfileResponse {
    id: i64,
    username: Option<String>,
    first_name: Option<String>,
    photo_url: Option<String>,
    role: yoink_access::Role,
    language: String,
    theme: String,
    ban_until: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    dl_count: u64,
    dl_last_at: Option<chrono::DateTime<Utc>>,
}

impl From<UserProfile> for ProfileResponse {
    fn from(user: UserProfile) -> Self {
        Self {
            id: user.id,
            username: user.username,
            first_name: user.first_name,
            photo_url: user.photo_url,
            role: user.role,
            language: user.language,
            theme: user.theme,
            ban_until: user.ban_until,
            created_at: user.created_at,
            updated_at: user.updated_at,
            dl_count: 0,
            dl_last_at: None,
        }
    }
}

#[derive(Serialize)]
struct SettingsResponse {
    user_id: i64,
    language: String,
    theme: String,
}

impl From<UserSettings> for SettingsResponse {
    fn from(settings: UserSettings) -> Self {
        Self {
            user_id: settings.user_id,
            language: settings.language,
            theme: settings.theme,
        }
    }
}

async fn profile(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let user = service
        .identities
        .current_profile(authenticated_id(service, &headers)?)
        .await?;
    Ok(no_store(ProfileResponse::from(user)))
}

async fn settings(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let settings = service
        .identities
        .settings(authenticated_id(service, &headers)?)
        .await?;
    Ok(no_store(SettingsResponse::from(settings)))
}

#[derive(Deserialize)]
struct SettingsRequest {
    language: Option<String>,
    theme: Option<String>,
}

async fn update_settings(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Result<Json<SettingsRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let user_id = authenticated_id(service, &headers)?;
    let Json(body) =
        body.map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request"))?;
    let settings = service
        .identities
        .update_settings(user_id, body.language.as_deref(), body.theme.as_deref())
        .await?;
    Ok(no_store(SettingsResponse::from(settings)))
}
