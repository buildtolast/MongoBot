use crate::config::{AppConfig, LlmProvider};
use crate::domain::models::ParsedQuery;
use crate::error::AppError;

/// LLM client supporting multiple providers.
pub struct LlmClient {
    ollama: Option<ollama_rs::Ollama>,
    #[allow(dead_code)] // Used when cloud LLM fallback is implemented
    http: reqwest::Client,
    provider: LlmProvider,
    model: String,
    #[allow(dead_code)]
    api_key: Option<String>,
}

impl LlmClient {
    /// Create a new LLM client based on configuration.
    pub fn new(config: &AppConfig) -> Self {
        let ollama = if config.llm_provider == LlmProvider::Ollama {
            // Parse host and port from the ollama_url
            let url = &config.ollama_url;
            let (host, port) = parse_ollama_url(url);
            Some(ollama_rs::Ollama::new(host, port))
        } else {
            None
        };

        let model = match config.llm_provider {
            LlmProvider::Ollama => config.ollama_model.clone(),
            _ => config
                .cloud_llm_model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        };

        Self {
            ollama,
            http: reqwest::Client::new(),
            provider: config.llm_provider,
            model,
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

    /// Call Ollama for text generation.
    async fn call_ollama(&self, prompt: &str) -> Result<String, AppError> {
        let ollama = self
            .ollama
            .as_ref()
            .ok_or_else(|| AppError::Internal("Ollama client not initialized".into()))?;

        use ollama_rs::generation::completion::request::GenerationRequest;

        let request = GenerationRequest::new(self.model.clone(), prompt.to_string())
            .format(ollama_rs::generation::parameters::FormatType::Json);

        let response = ollama
            .generate(request)
            .await
            .map_err(|e| AppError::LlmParse(format!("Ollama generation failed: {e}")))?;

        Ok(response.response)
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

/// Parse an Ollama URL like "http://localhost:11434" into (host, port).
fn parse_ollama_url(url: &str) -> (String, u16) {
    // Manual parsing to avoid a `url` crate dependency.
    // Expected format: "http://host:port" or "http://host"
    let (scheme, rest) = if let Some(stripped) = url.strip_prefix("https://") {
        ("https", stripped)
    } else if let Some(stripped) = url.strip_prefix("http://") {
        ("http", stripped)
    } else {
        ("http", url)
    };

    let (host_part, port) = if let Some(colon_pos) = rest.rfind(':') {
        let port_str = &rest[colon_pos + 1..];
        if let Ok(p) = port_str.parse::<u16>() {
            (&rest[..colon_pos], p)
        } else {
            (rest, 11434)
        }
    } else {
        (rest, 11434)
    };

    (format!("{scheme}://{host_part}"), port)
}

/// Build the system prompt for NL-to-MongoDB translation.
pub(crate) fn build_prompt(query: &str, schema: &str) -> String {
    format!(
        r#"You are a MongoDB query translator. Convert natural language queries into structured JSON.

## Available Collections and Schemas

{schema}

## Output Format

Respond with ONLY valid JSON matching this structure:
{{
  "collection": "collection_name",
  "operation": "find" | "count" | "aggregate" | "distinct",
  "filters": [
    {{
      "field": "field_name",
      "operator": "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "nin" | "contains" | "regex",
      "value": <value>
    }}
  ],
  "projection": ["field1", "field2"],
  "sort": {{ "field": "field_name", "order": 1 }},
  "limit": 100
}}

Rules:
- Use only collections and fields from the schema above.
- Default limit to 100 if not specified.
- For text search use "contains" or "regex" operators.
- For date comparisons use ISO 8601 strings.
- "in" and "nin" operators expect array values.

## User Query

{query}"#
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
    fn test_parse_ollama_url() {
        let (host, port) = parse_ollama_url("http://localhost:11434");
        assert_eq!(host, "http://localhost");
        assert_eq!(port, 11434);

        let (host, port) = parse_ollama_url("http://myhost:9999");
        assert_eq!(host, "http://myhost");
        assert_eq!(port, 9999);
    }

    #[test]
    fn test_build_prompt_contains_query() {
        let prompt = build_prompt("find all users", "schema info here");
        assert!(prompt.contains("find all users"));
        assert!(prompt.contains("schema info here"));
    }
}
