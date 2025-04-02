use std::{sync::Arc, time::Duration};

use anyhow::Context;
use reqwest::{Client, Request, Response};
use serde::de::DeserializeOwned;
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::app_state::AppError;

/// Request that is send to limited request client
#[derive(Debug)]
struct LimitedRequest {
    req: Request,
    res: oneshot::Sender<reqwest::Result<Response>>,
    /// This cancellation token is needed to eliminate requests from the request queue
    cancellation_token: CancellationToken,
}

/// Rate limited HTTP request client.
///
/// Note that cloned instances of this struct will "share" rate limit
#[derive(Debug, Clone)]
pub struct LimitedRequestClient {
    request_tx: mpsc::Sender<LimitedRequest>,
}

impl LimitedRequestClient {
    /// Create new limited client.
    ///
    /// Number argument is the allowed "concurrency", [Duration] argument is rate.
    ///
    /// For example arguments (50, [std::time::Duration::SECOND]) mean that rate limit is 50 requests per second
    pub fn new(client: Client, limit_number: usize, limit_duration: Duration) -> Self {
        let (tx, mut rx) = mpsc::channel::<LimitedRequest>(100);
        tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(limit_number));
            while let Some(LimitedRequest {
                req,
                res,
                cancellation_token,
            }) = rx.recv().await
            {
                let semaphore = semaphore.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let permit = tokio::select! {
                        biased;
                        _ = cancellation_token.cancelled() => {
                            // When request is cancelled before being sent there is no need to wait
                            return;
                        }
                        Ok(permit) = semaphore.acquire() => permit,
                    };
                    tokio::select! {
                        response = client.execute(req) => {
                            if let Err(_) = res.send(response) {
                                tracing::error!("Failed to send response: channel closed")
                            };
                        },
                        _ = cancellation_token.cancelled() => {}
                    }
                    tokio::time::sleep(limit_duration).await;
                    drop(permit);
                });
            }
        });
        Self { request_tx: tx }
    }

    pub async fn request<T>(&self, req: Request) -> Result<T, AppError>
    where
        T: DeserializeOwned,
    {
        let url = req.url().to_string();
        let response = self.request_raw(req).await?;
        match response.json().await {
            Ok(res) => Ok(res),
            Err(e) => {
                tracing::error!(url, "Failed to deserialize fetch response: {e}");
                Err(AppError::internal_error(
                    "failed to deserialize response json body",
                ))
            }
        }
    }

    pub async fn request_raw(&self, req: Request) -> Result<Response, AppError> {
        let (tx, rx) = oneshot::channel::<Result<Response, reqwest::Error>>();
        let cancellation_token = CancellationToken::new();
        // Its important to drop this guard after getting reqwest::Response
        //
        // This is here because axum handlers will drop handler future where we call this method.
        // Usually when that happens we don't need queued requests to succeed.
        // After this guard gets dropped request will not be made.
        let _guard = cancellation_token.clone().drop_guard();
        let url = req.url().to_string();
        let payload = LimitedRequest {
            req,
            res: tx,
            cancellation_token,
        };
        tracing::trace!("Sending request: {}", url);
        self.request_tx
            .send(payload)
            .await
            .context("Failed to send request")?;
        let response = rx
            .await
            .map_err(|e| anyhow::anyhow!("failed to receive response: {e}"))?
            .map_err(|e| {
                tracing::error!("Request to {} failed: {}", url, e);
                anyhow::anyhow!("Request failed: {}", e)
            })?;
        tracing::trace!(
            status = response.status().as_u16(),
            url,
            "Provider response"
        );
        match response.status().as_u16() {
            200 => Ok(response),
            404 => Err(AppError::not_found("Provider responded with 404")),
            rest => Err(anyhow::anyhow!("provider responded with status {}", rest).into()),
        }
    }
}
