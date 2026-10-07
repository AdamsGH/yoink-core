//! Shared bearer authentication and current identity authority for native Inbox.
use std::sync::Arc;

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::IntoResponse,
};
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_identity::IdentityStore;
use yoink_inbox::{
    Error,
    contracts::{Authorization, Call},
    http::{Actor, HttpState},
};

use crate::auth::{ApiError, AuthState, authenticated_id};

/// Reusable by the composition root for Inbox HTTP and background services.
/// Every check reads current bans, roles and effective grants from shared identity.
pub struct InboxAuthorization {
    identities: Arc<IdentityStore>,
}
impl InboxAuthorization {
    pub fn new(identities: Arc<IdentityStore>) -> Self {
        Self { identities }
    }

    async fn check(
        &self,
        user: i64,
        min_role: Role,
        feature: Option<(&str, &str)>,
    ) -> yoink_inbox::Result<()> {
        let snapshot = self
            .identities
            .snapshot(user, None)
            .await
            .map_err(|_| Error::External)?
            .ok_or(Error::Forbidden)?;
        let policy = AccessPolicy {
            min_role,
            scopes: vec![AccessScope::Private],
            check_group_enabled: false,
            check_thread_policy: false,
        };
        if snapshot.authorize(&policy, feature).decision.allowed {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
}
impl Authorization for InboxAuthorization {
    fn require<'a>(&'a self, user: i64, feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            let (plugin, feature) = feature.split_once(':').ok_or(Error::Forbidden)?;
            if plugin != "inbox" {
                return Err(Error::Forbidden);
            }
            self.check(user, Role::Restricted, Some((plugin, feature)))
                .await
        })
    }
    fn require_admin(&self, user: i64) -> Call<'_, ()> {
        // Legacy Inbox administration is Moderator-or-higher, not inbox:admin's
        // Admin default feature threshold. Shared access still enforces bans.
        Box::pin(self.check(user, Role::Moderator, None))
    }
}

pub(crate) fn router(
    system: yoink_application::SystemService,
    auth: AuthState,
    mut inbox: HttpState,
) -> Router {
    // Do not trust even a caller-supplied authority implementation for HTTP.
    // Resolve the same IdentityStore used to authenticate this request.
    inbox.auth = Arc::new(HttpAuthorization(auth.clone()));
    Router::new()
        .nest("/api/v1/inbox", yoink_inbox::http::router(inbox))
        .route_layer(middleware::from_fn(
            move |mut request: Request, next: Next| {
                let auth = auth.clone();
                let ready = system.snapshot().ready;
                async move {
                    let verified = if ready {
                        auth.service()
                            .and_then(|service| authenticated_id(service, request.headers()))
                    } else {
                        Err(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Service unavailable",
                        ))
                    };
                    let mut response = match verified {
                        Ok(id) => {
                            request.extensions_mut().insert(Actor(id));
                            next.run(request).await
                        }
                        Err(error) => error.into_response(),
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
            },
        ))
}

// AuthState supports delayed installation after schema/readiness checks.
struct HttpAuthorization(AuthState);
impl Authorization for HttpAuthorization {
    fn require<'a>(&'a self, user: i64, feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            let service = self.0.service().map_err(|_| Error::External)?;
            InboxAuthorization::new(service.identities.clone())
                .require(user, feature)
                .await
        })
    }
    fn require_admin(&self, user: i64) -> Call<'_, ()> {
        Box::pin(async move {
            let service = self.0.service().map_err(|_| Error::External)?;
            InboxAuthorization::new(service.identities.clone())
                .require_admin(user)
                .await
        })
    }
}
