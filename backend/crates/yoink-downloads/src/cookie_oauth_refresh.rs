use std::{future::Future, pin::Pin};

use chrono::Utc;
use yoink_worker_protocol::youtube_auth::YoutubeOAuth;

use crate::{
    cookie_oauth::{OAuthProvider, YouTubeOAuthProvider},
    profile::{ProfileError, oauth_refresh::OAuthRefreshProvider},
};

impl OAuthRefreshProvider for YouTubeOAuthProvider {
    fn exchange<'a>(
        &'a self,
        tokens: &'a YoutubeOAuth,
    ) -> Pin<Box<dyn Future<Output = Result<YoutubeOAuth, ProfileError>> + Send + 'a>> {
        Box::pin(async move {
            self.refresh(tokens, Utc::now().timestamp())
                .await
                .map_err(|_| ProfileError::Unavailable)
        })
    }
}
