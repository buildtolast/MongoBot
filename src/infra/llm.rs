use crate::config::{AppConfig, LlmProvider};
use crate::domain::models::ParsedQuery;
use crate::error::AppError;

/// LLM client supporting multiple providers.
pub struct LlmClient {
    http: reqwest::Client,
    provider: LlmProvider,
    model: String,
    ollama_url: String,
    #[allow(dead_code)]
    api_key: Option<String>,
}

impl LlmClient {
    /// Create a new LLM client based on configuration.
    pub fn new(config: &AppConfig) -> Self {
        let model = match config.llm_provider {
            LlmProvider::Ollama => config.ollama_model.clone(),
            _ => config
                .cloud_llm_model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        };

        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .unwrap_or_default(),
            provider: config.llm_provider,
            model,
            ollama_url: config.ollama_url.clone(),
            api_key: config.cloud_llm_api_key.clone(),
        }
    }

    /// Parse a natural language query into a structured MongoDB query.
    pub async fn parse_query(
        &self,
        user_query: &str,
        schema_context: &str,
    ) -> Result<ParsedQuery, AppError> {
        let prompt = build_prompt(user_query, schema_context);

        let raw = match self.provider {
            LlmProvider::Ollama => self.call_ollama(&prompt).await?,
            LlmProvider::Anthropic => self.call_anthropic(&prompt).await?,
            LlmProvider::OpenAI => self.call_openai(&prompt).await?,
        };

        parse_llm_response(&raw)
    }

    /// Call Ollama for text generation via HTTP API directly.
    async fn call_ollama(&self, prompt: &str) -> Result<String, AppError> {
        let url = format!("{}/api/generate", self.ollama_url.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "format": "json"
        });

        tracing::debug!(url = %url, model = %self.model, "Calling Ollama");

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::LlmParse(format!("Ollama request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::LlmParse(format!(
                "Ollama returned {status}: {text}"
            )));
        }

        #[derive(serde::Deserialize)]
        struct OllamaResponse {
            response: String,
        }

        let parsed: OllamaResponse = response
            .json()
            .await
            .map_err(|e| AppError::LlmParse(format!("Failed to parse Ollama response: {e}")))?;

        tracing::debug!(response_len = parsed.response.len(), "Ollama response received");
        Ok(parsed.response)
    }

    /// Call Anthropic API (stub — to be implemented with real API).
    async fn call_anthropic(&self, _prompt: &str) -> Result<String, AppError> {
        todo!("Anthropic API integration not yet implemented")
    }

    /// Call OpenAI API (stub — to be implemented with real API).
    async fn call_openai(&self, _prompt: &str) -> Result<String, AppError> {
        todo!("OpenAI API integration not yet implemented")
    }
}


/// Build the system prompt for NL-to-MongoDB translation.
pub(crate) fn build_prompt(query: &str, schema: &str) -> String {
    format!(
        r#"You are a MongoDB query translator. Return ONLY valid JSON. No explanation.

Collections:
{schema}
JSON format (REQUIRED fields: collection, operation, filters, limit):
{{
  "collection": "name",
  "operation": "find|count|distinct|insert|update",
  "filters": [{{"field":"name","operator":"eq|ne|gt|gte|lt|lte|in|contains","value":"x"}}],
  "projection": [],
  "sort": {{"field":"name","order":1}},
  "limit": 100,
  "document": {{}},
  "update_fields": {{}}
}}

Rules: Use only fields from schema. Default limit 100. For insert: set "document". For update: set "filters" + "update_fields". Prefer indexed fields for filters/sort.

Query: {query}"#
    )
}

/// Parse raw LLM response text into a ParsedQuery struct.
pub(crate) fn parse_llm_response(raw: &str) -> Result<ParsedQuery, AppError> {
    // Strip markdown code fences if present
    let cleaned = strip_markdown_fences(raw);

    serde_json::from_str::<ParsedQuery>(&cleaned)
        .map_err(|e| AppError::LlmParse(format!("Failed to parse LLM response as JSON: {e}")))
}

/// Remove markdown code block markers from LLM output.
fn strip_markdown_fences(text: &str) -> String {
    let trimmed = text.trim();

    // Handle ```json ... ``` or ``` ... ```
    if trimmed.starts_with("```") {
        let without_opening = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };

        let without_closing = without_opening
            .trim()
            .strip_suffix("```")
            .unwrap_or(without_opening);

        return without_closing.trim().to_string();
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown_fences() {
        let input = "```json\n{\"collection\": \"test\"}\n```";
        assert_eq!(strip_markdown_fences(input), "{\"collection\": \"test\"}");
    }

    #[test]
    fn test_strip_no_fences() {
        let input = "{\"collection\": \"test\"}";
        assert_eq!(strip_markdown_fences(input), "{\"collection\": \"test\"}");
    }

    #[test]
    fn test_build_prompt_contains_query() {
        let prompt = build_prompt("find all users", "schema info here");
        assert!(prompt.contains("find all users"));
        assert!(prompt.contains("schema info here"));
    }
}
