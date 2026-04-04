use crate::config::{AppConfig, LlmProvider};
use crate::domain::models::ParsedQuery;
use crate::error::AppError;

/// LLM client supporting multiple providers:
/// - Ollama (local, default)
/// - LM Studio (local, OpenAI-compatible API)
/// - Anthropic Claude (cloud)
/// - OpenAI GPT (cloud)
pub struct LlmClient {
    http: reqwest::Client,
    provider: LlmProvider,
    model: String,
    // Provider-specific config (all from env vars, no hardcoding)
    ollama_url: String,
    lmstudio_url: String,
    lmstudio_model: Option<String>,
    anthropic_api_url: String,
    openai_api_url: String,
    api_key: Option<String>,
}

impl LlmClient {
    /// Create a new LLM client based on configuration.
    pub fn new(config: &AppConfig) -> Self {
        let model = match config.llm_provider {
            LlmProvider::Ollama => config.ollama_model.clone(),
            LlmProvider::LmStudio => config.lmstudio_model.clone()
                .or_else(|| config.cloud_llm_model.clone())
                .unwrap_or_else(|| "default".to_string()),
            _ => config.cloud_llm_model.clone()
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
            lmstudio_url: config.lmstudio_url.clone(),
            lmstudio_model: config.lmstudio_model.clone(),
            anthropic_api_url: config.anthropic_api_url.clone(),
            openai_api_url: config.openai_api_url.clone(),
            api_key: config.cloud_llm_api_key.clone(),
        }
    }

    /// Parse a natural language query into a structured MongoDB query.
    pub async fn parse_query(
        &self,
        user_query: &str,
        schema_context: &str,
        model_override: Option<&str>,
    ) -> Result<ParsedQuery, AppError> {
        let prompt = build_prompt(user_query, schema_context);

        let raw = match self.provider {
            LlmProvider::Ollama => self.call_ollama(&prompt, model_override).await?,
            LlmProvider::LmStudio => self.call_lmstudio(&prompt, model_override).await?,
            LlmProvider::Anthropic => self.call_anthropic(&prompt, model_override).await?,
            LlmProvider::OpenAI => self.call_openai(&prompt, model_override).await?,
        };

        parse_llm_response(&raw)
    }

    /// List available models based on the current provider.
    pub async fn list_models(&self) -> Result<Vec<String>, AppError> {
        match self.provider {
            LlmProvider::Ollama => self.list_ollama_models().await,
            LlmProvider::LmStudio => self.list_openai_compat_models(&self.lmstudio_url).await,
            LlmProvider::OpenAI => self.list_openai_compat_models(&self.openai_api_url).await,
            LlmProvider::Anthropic => {
                // Anthropic doesn't have a model list API — return common models
                Ok(vec![
                    "claude-sonnet-4-20250514".into(),
                    "claude-haiku-4-5-20251001".into(),
                    "claude-opus-4-20250514".into(),
                ])
            }
        }
    }

    // ── Ollama ──────────────────────────────────────────────────

    async fn list_ollama_models(&self) -> Result<Vec<String>, AppError> {
        let url = format!("{}/api/tags", self.ollama_url.trim_end_matches('/'));
        let response = self.http.get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send().await
            .map_err(|e| AppError::Internal(format!("Failed to list Ollama models: {e}")))?;

        #[derive(serde::Deserialize)]
        struct TagsResponse { models: Vec<ModelInfo> }
        #[derive(serde::Deserialize)]
        struct ModelInfo { name: String }

        let tags: TagsResponse = response.json().await
            .map_err(|e| AppError::Internal(format!("Failed to parse Ollama models: {e}")))?;
        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }

    async fn call_ollama(&self, prompt: &str, model_override: Option<&str>) -> Result<String, AppError> {
        let url = format!("{}/api/generate", self.ollama_url.trim_end_matches('/'));
        let model = model_override.unwrap_or(&self.model);

        let body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "format": "json"
        });

        tracing::debug!(provider = "ollama", model = %model, "Calling LLM");

        let response = self.http.post(&url).json(&body).send().await
            .map_err(|e| AppError::LlmParse(format!("Ollama request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::LlmParse(format!("Ollama returned {status}: {text}")));
        }

        #[derive(serde::Deserialize)]
        struct OllamaResponse { response: String }

        let parsed: OllamaResponse = response.json().await
            .map_err(|e| AppError::LlmParse(format!("Failed to parse Ollama response: {e}")))?;

        tracing::debug!(response_len = parsed.response.len(), "LLM response received");
        Ok(parsed.response)
    }

    // ── LM Studio (OpenAI-compatible API) ───────────────────────

    async fn call_lmstudio(&self, prompt: &str, model_override: Option<&str>) -> Result<String, AppError> {
        let url = format!("{}/v1/chat/completions", self.lmstudio_url.trim_end_matches('/'));
        let model = model_override
            .or(self.lmstudio_model.as_deref())
            .unwrap_or(&self.model);

        tracing::debug!(provider = "lmstudio", model = %model, "Calling LLM");

        self.call_openai_compatible(&url, model, prompt, None).await
    }

    // ── OpenAI ──────────────────────────────────────────────────

    async fn call_openai(&self, prompt: &str, model_override: Option<&str>) -> Result<String, AppError> {
        let url = format!("{}/v1/chat/completions", self.openai_api_url.trim_end_matches('/'));
        let model = model_override.unwrap_or(&self.model);
        let api_key = self.api_key.as_deref()
            .ok_or_else(|| AppError::Config("CLOUD_LLM_API_KEY required for OpenAI".into()))?;

        tracing::debug!(provider = "openai", model = %model, "Calling LLM");

        self.call_openai_compatible(&url, model, prompt, Some(api_key)).await
    }

    /// Shared implementation for OpenAI-compatible APIs (OpenAI, LM Studio, Azure, etc.)
    async fn call_openai_compatible(
        &self,
        url: &str,
        model: &str,
        prompt: &str,
        api_key: Option<&str>,
    ) -> Result<String, AppError> {
        let body = serde_json::json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a MongoDB query translator. Return ONLY valid JSON. No explanation."
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "temperature": 0.1,
            "response_format": { "type": "json_object" }
        });

        let mut req = self.http.post(url).json(&body);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }

        let response = req.send().await
            .map_err(|e| AppError::LlmParse(format!("OpenAI-compatible request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::LlmParse(format!("LLM returned {status}: {text}")));
        }

        #[derive(serde::Deserialize)]
        struct ChatResponse { choices: Vec<Choice> }
        #[derive(serde::Deserialize)]
        struct Choice { message: Message }
        #[derive(serde::Deserialize)]
        struct Message { content: String }

        let parsed: ChatResponse = response.json().await
            .map_err(|e| AppError::LlmParse(format!("Failed to parse LLM response: {e}")))?;

        let content = parsed.choices.into_iter().next()
            .map(|c| c.message.content)
            .ok_or_else(|| AppError::LlmParse("Empty response from LLM".into()))?;

        tracing::debug!(response_len = content.len(), "LLM response received");
        Ok(content)
    }

    // ── Anthropic Claude ────────────────────────────────────────

    async fn call_anthropic(&self, prompt: &str, model_override: Option<&str>) -> Result<String, AppError> {
        let url = format!("{}/v1/messages", self.anthropic_api_url.trim_end_matches('/'));
        let model = model_override.unwrap_or(&self.model);
        let api_key = self.api_key.as_deref()
            .ok_or_else(|| AppError::Config("CLOUD_LLM_API_KEY required for Anthropic".into()))?;

        tracing::debug!(provider = "anthropic", model = %model, "Calling LLM");

        let body = serde_json::json!({
            "model": model,
            "max_tokens": 4096,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "system": "You are a MongoDB query translator. Return ONLY valid JSON. No explanation.",
            "temperature": 0.1
        });

        let response = self.http.post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::LlmParse(format!("Anthropic request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::LlmParse(format!("Anthropic returned {status}: {text}")));
        }

        #[derive(serde::Deserialize)]
        struct AnthropicResponse { content: Vec<ContentBlock> }
        #[derive(serde::Deserialize)]
        struct ContentBlock { text: Option<String> }

        let parsed: AnthropicResponse = response.json().await
            .map_err(|e| AppError::LlmParse(format!("Failed to parse Anthropic response: {e}")))?;

        let text = parsed.content.into_iter()
            .find_map(|b| b.text)
            .ok_or_else(|| AppError::LlmParse("Empty response from Anthropic".into()))?;

        tracing::debug!(response_len = text.len(), "LLM response received");
        Ok(text)
    }

    // ── OpenAI-compatible model listing ─────────────────────────

    async fn list_openai_compat_models(&self, base_url: &str) -> Result<Vec<String>, AppError> {
        let url = format!("{}/v1/models", base_url.trim_end_matches('/'));

        let mut req = self.http.get(&url)
            .timeout(std::time::Duration::from_secs(5));
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req.send().await
            .map_err(|e| AppError::Internal(format!("Failed to list models: {e}")))?;

        #[derive(serde::Deserialize)]
        struct ModelsResponse { data: Vec<ModelData> }
        #[derive(serde::Deserialize)]
        struct ModelData { id: String }

        let models: ModelsResponse = response.json().await
            .map_err(|e| AppError::Internal(format!("Failed to parse models: {e}")))?;

        Ok(models.data.into_iter().map(|m| m.id).collect())
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
  "operation": "find|count|distinct|aggregate|insert|update",
  "filters": [{{"field":"name","operator":"eq|ne|gt|gte|lt|lte|in|nin|contains|regex","value":"x"}}],
  "projection": [],
  "sort": {{"field":"name","order":1}},
  "limit": 100,
  "document": {{}},
  "update_fields": {{}}
}}

Rules: Use only fields from schema. Default limit 100. For date fields always use ISO 8601 format "YYYY-MM-DDT00:00:00Z" (e.g. "2010-01-01T00:00:00Z"), never bare years. For distinct: put the target field name in "projection" array. For insert: set "document". For update: set "filters" + "update_fields". Prefer indexed fields for filters/sort. For array fields use dot notation (e.g. "transactions.symbol") — when filtering on multiple sub-fields of the same array, list each as a separate filter.

Query: {query}"#
    )
}

/// Parse raw LLM response text into a ParsedQuery struct.
/// Handles common LLM output variations gracefully.
pub(crate) fn parse_llm_response(raw: &str) -> Result<ParsedQuery, AppError> {
    let cleaned = strip_markdown_fences(raw);

    // Try direct parse first
    if let Ok(parsed) = serde_json::from_str::<ParsedQuery>(&cleaned) {
        return Ok(parsed);
    }

    // If direct parse fails, try to fix common LLM issues via loose Value parse
    let value: serde_json::Value = serde_json::from_str(&cleaned)
        .map_err(|e| AppError::LlmParse(format!("LLM returned invalid JSON: {e}")))?;

    let obj = value.as_object()
        .ok_or_else(|| AppError::LlmParse("LLM returned non-object JSON".into()))?;

    // Build ParsedQuery manually from loose JSON
    let collection = obj.get("collection")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if collection.is_empty() {
        return Err(AppError::LlmParse("LLM response missing 'collection' field".into()));
    }

    let operation = obj.get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("find");

    let operation = match operation {
        "count" => crate::domain::models::QueryOperation::Count,
        "distinct" => crate::domain::models::QueryOperation::Distinct,
        "aggregate" => crate::domain::models::QueryOperation::Aggregate,
        "insert" => crate::domain::models::QueryOperation::Insert,
        "update" => crate::domain::models::QueryOperation::Update,
        "delete" => crate::domain::models::QueryOperation::Delete,
        _ => crate::domain::models::QueryOperation::Find,
    };

    // Parse filters flexibly — accept various LLM formats
    let filters = parse_filters_flexible(obj.get("filters"));

    let mut projection: Vec<String> = obj.get("projection")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    // For distinct with empty projection, try to find the field from other keys
    if operation == crate::domain::models::QueryOperation::Distinct && projection.is_empty() {
        // Check "field" key (some LLMs return {"field": "language"} for distinct)
        if let Some(f) = obj.get("field").and_then(|v| v.as_str()) {
            projection.push(f.to_string());
        }
        // Check "key" key
        else if let Some(f) = obj.get("key").and_then(|v| v.as_str()) {
            projection.push(f.to_string());
        }
    }

    let sort = obj.get("sort").and_then(|v| {
        let s = v.as_object()?;
        let field = s.get("field").and_then(|f| f.as_str())?.to_string();
        let order = s.get("order").and_then(|o| o.as_i64()).unwrap_or(1) as i32;
        Some(crate::domain::models::SortSpec { field, order })
    });

    let limit = obj.get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;

    let document = obj.get("document").cloned();
    let update_fields = obj.get("update_fields").cloned();
    let update_many = obj.get("update_many").and_then(|v| v.as_bool()).unwrap_or(false);
    let delete_many = obj.get("delete_many").and_then(|v| v.as_bool()).unwrap_or(false);

    Ok(crate::domain::models::ParsedQuery {
        collection,
        operation,
        filters,
        projection,
        sort,
        limit,
        document,
        update_fields,
        update_many,
        delete_many,
    })
}

/// Parse filters from various LLM formats.
/// Accepts: [{field, operator, value}], [{field: value}], {field: value}, etc.
fn parse_filters_flexible(filters_val: Option<&serde_json::Value>) -> Vec<crate::domain::models::Filter> {
    let Some(val) = filters_val else { return vec![] };

    if let Some(arr) = val.as_array() {
        arr.iter().filter_map(|item| {
            let obj = item.as_object()?;

            // Standard format: {field, operator, value}
            if let (Some(field), Some(value)) = (obj.get("field").and_then(|v| v.as_str()), obj.get("value")) {
                let op = obj.get("operator")
                    .and_then(|v| v.as_str())
                    .unwrap_or("eq");
                let operator = match op {
                    "ne" => crate::domain::models::FilterOperator::Ne,
                    "gt" => crate::domain::models::FilterOperator::Gt,
                    "gte" => crate::domain::models::FilterOperator::Gte,
                    "lt" => crate::domain::models::FilterOperator::Lt,
                    "lte" => crate::domain::models::FilterOperator::Lte,
                    "in" => crate::domain::models::FilterOperator::In,
                    "nin" => crate::domain::models::FilterOperator::Nin,
                    "contains" => crate::domain::models::FilterOperator::Contains,
                    "regex" => crate::domain::models::FilterOperator::Regex,
                    _ => crate::domain::models::FilterOperator::Eq,
                };
                return Some(crate::domain::models::Filter {
                    field: field.to_string(),
                    operator,
                    value: value.clone(),
                });
            }

            // Alternative format: {"fieldname": "value"} or {"fieldname": {"$gt": 5}}
            if obj.len() == 1 {
                let (field, value) = obj.iter().next()?;
                if field.starts_with('$') { return None; } // Skip mongo operators at top level

                // Check if value is a mongo operator object like {"$gt": 5}
                if let Some(inner_obj) = value.as_object() {
                    if let Some((op_key, op_val)) = inner_obj.iter().next() {
                        let operator = match op_key.as_str() {
                            "$ne" => crate::domain::models::FilterOperator::Ne,
                            "$gt" => crate::domain::models::FilterOperator::Gt,
                            "$gte" => crate::domain::models::FilterOperator::Gte,
                            "$lt" => crate::domain::models::FilterOperator::Lt,
                            "$lte" => crate::domain::models::FilterOperator::Lte,
                            "$in" => crate::domain::models::FilterOperator::In,
                            "$regex" => crate::domain::models::FilterOperator::Regex,
                            _ => crate::domain::models::FilterOperator::Eq,
                        };
                        return Some(crate::domain::models::Filter {
                            field: field.clone(),
                            operator,
                            value: op_val.clone(),
                        });
                    }
                }

                return Some(crate::domain::models::Filter {
                    field: field.clone(),
                    operator: crate::domain::models::FilterOperator::Eq,
                    value: value.clone(),
                });
            }

            None
        }).collect()
    } else if let Some(obj) = val.as_object() {
        // Handle filter as a plain object: {"genre": "Sci-Fi", "rating": {"$gt": 8}}
        obj.iter().filter_map(|(field, value)| {
            if field.starts_with('$') { return None; }
            if let Some(inner) = value.as_object() {
                if let Some((op_key, op_val)) = inner.iter().next() {
                    let operator = match op_key.as_str() {
                        "$ne" => crate::domain::models::FilterOperator::Ne,
                        "$gt" => crate::domain::models::FilterOperator::Gt,
                        "$gte" => crate::domain::models::FilterOperator::Gte,
                        "$lt" => crate::domain::models::FilterOperator::Lt,
                        "$lte" => crate::domain::models::FilterOperator::Lte,
                        _ => crate::domain::models::FilterOperator::Eq,
                    };
                    return Some(crate::domain::models::Filter {
                        field: field.clone(), operator, value: op_val.clone(),
                    });
                }
            }
            Some(crate::domain::models::Filter {
                field: field.clone(),
                operator: crate::domain::models::FilterOperator::Eq,
                value: value.clone(),
            })
        }).collect()
    } else {
        vec![]
    }
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
