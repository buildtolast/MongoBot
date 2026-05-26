use crate::error::AppError;

/// Minimal Slack client for posting messages.
#[derive(Clone)]
pub struct SlackClient {
    bot_token: Option<String>,
    http: reqwest::Client,
}

impl SlackClient {
    /// Create a new Slack client.
    pub fn new(bot_token: Option<String>) -> Self {
        Self {
            bot_token,
            http: reqwest::Client::new(),
        }
    }

    /// Post a message to a Slack response URL.
    pub async fn post_message(&self, response_url: &str, text: &str) -> Result<(), AppError> {
        let payload = serde_json::json!({
            "text": text,
            "response_type": "in_channel",
        });

        let mut req = self.http.post(response_url).json(&payload);

        // Include bot token as Bearer auth if available
        if let Some(ref token) = self.bot_token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Slack POST failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            tracing::error!(
                status = %status,
                body = %body,
                "Slack response_url returned error"
            );
            return Err(AppError::Internal(format!(
                "Slack API returned {status}"
            )));
        }

        Ok(())
    }

    /// Check if Slack integration is configured.
    pub fn is_configured(&self) -> bool {
        self.bot_token.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_without_token_is_not_configured() {
        let client = SlackClient::new(None);
        assert!(!client.is_configured());
    }

    #[test]
    fn new_with_token_is_configured() {
        let client = SlackClient::new(Some("xoxb-test-token".into()));
        assert!(client.is_configured());
    }
}
