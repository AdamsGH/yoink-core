//! Concrete confined anonymous content primitives. Coordinator owns proxy selection
//! and constructs an Executor using the fixed Landlock launcher (default args).
use crate::{
    Error,
    contracts::Call,
    enrich::{Extraction, Stages},
};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
use yoink_worker_protocol::{ContentRequest, ContentStage, HttpSourceUrl, PROTOCOL_VERSION};

pub struct ContentStages {
    executor: yoink_executor::Executor,
    proxy_url: Option<String>,
    cancel: CancellationToken,
}
impl ContentStages {
    pub fn new(
        executor: yoink_executor::Executor,
        proxy_url: Option<String>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            executor,
            proxy_url,
            cancel,
        }
    }
    async fn run(&self, task: ContentStage) -> crate::Result<yoink_worker_protocol::ContentResult> {
        let request = ContentRequest {
            version: PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            task,
            proxy_url: self.proxy_url.clone(),
        };
        self.executor
            .execute_content(&request, self.cancel.child_token())
            .await
            .map_err(|e| match e {
                yoink_executor::ExecutorError::ContentDenied => Error::Forbidden,
                yoink_executor::ExecutorError::Cancelled => Error::Cancelled,
                _ => Error::External,
            })
    }
}
impl Stages for ContentStages {
    fn fetch<'a>(
        &'a self,
        _user: i64,
        url: &'a str,
        headers: &'a BTreeMap<String, String>,
    ) -> Call<'a, String> {
        Box::pin(async move {
            let url = HttpSourceUrl::parse(url).map_err(|_| Error::Forbidden)?;
            Ok(self
                .run(ContentStage::ContentGet {
                    url,
                    headers: headers.clone(),
                })
                .await?
                .text)
        })
    }
    fn readability<'a>(&'a self, html: &'a str, url: &'a str) -> Call<'a, Extraction> {
        Box::pin(async move {
            let url = HttpSourceUrl::parse(url).map_err(|_| Error::Invalid)?;
            let r = self
                .run(ContentStage::ContentReadability {
                    html: html.into(),
                    url,
                })
                .await?;
            Ok(Extraction {
                text: r.text,
                title: r.title,
            })
        })
    }
    fn trafilatura<'a>(&'a self, html: &'a str) -> Call<'a, String> {
        Box::pin(async move {
            Ok(self
                .run(ContentStage::ContentTrafilatura { html: html.into() })
                .await?
                .text)
        })
    }
}
