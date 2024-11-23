use std::{sync::Arc, time::Duration};

use anyhow::Context;
use reqwest::{Client, Request, Response};
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::app_state::AppError;

#[derive(Debug, Clone)]
pub struct LimitedRequestClient {
    request_tx: mpsc::Sender<(Request, oneshot::Sender<reqwest::Result<Response>>)>,
}

impl LimitedRequestClient {
    pub fn new(client: Client, limit_number: usize, limit_duration: Duration) -> Self {
        let (tx, mut rx) =
            mpsc::channel::<(Request, oneshot::Sender<reqwest::Result<Response>>)>(100);
        tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(limit_number));
            while let Some((req, resp_tx)) = rx.recv().await {
                let semaphore = semaphore.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let permit = semaphore.acquire().await.unwrap();
                    let response = client.execute(req).await;

                    if let Err(_) = resp_tx.send(response) {
                        tracing::error!("Failed to send response: channel closed")
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
        let (tx, rx) = oneshot::channel::<Result<Response, reqwest::Error>>();
        let url = req.url().to_string();
        tracing::trace!("Sending request: {}", url);
        self.request_tx
            .send((req, tx))
            .await
            .context("Failed to send request")?;
        let response = rx
            .await
            .map_err(|e| anyhow::anyhow!("failed to receive response: {e}"))?
            .map_err(|e| {
                tracing::error!("Request in {} failed: {}", url, e);
                anyhow::anyhow!("Request failed: {}", e)
            })?;
        tracing::trace!(
            status = response.status().as_u16(),
            url,
            "Provider response"
        );
        match response.status().as_u16() {
            200 => Ok(response.json().await.unwrap()),
            404 => Err(AppError::not_found("Provider responded with 404")),
            rest => Err(anyhow::anyhow!("provider responded with status {}", rest).into()),
        }
    }
}
