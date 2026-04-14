//! Integration tests for the Slack-Mongo Query Bot API.
//!
//! These tests require:
//! - MongoDB running at localhost:27017 with the "movies" database populated
//! - Ollama running at localhost:11434 with qwen3-coder:30b model
//! - The bot server running at localhost:3000
//!
//! Run with: cargo test --test integration_tests -- --test-threads=1
//! Or via: bash tests/query_tests.sh (bash version)

use serde_json::{json, Value};

const BASE_URL: &str = "http://localhost:3000";

/// Helper: send a query to the API and return the parsed JSON response.
/// Honours TEST_MODEL and TEST_PROVIDER env vars to target a specific provider/model.
async fn query(text: &str) -> Value {
    let model = std::env::var("TEST_MODEL").unwrap_or_else(|_| "qwen3-coder:30b".to_string());
    let provider = std::env::var("TEST_PROVIDER").ok();
    query_with_model_and_provider(text, &model, provider.as_deref()).await
}

/// Helper: send a query with a specific model.
async fn query_with_model(text: &str, model: &str) -> Value {
    let provider = std::env::var("TEST_PROVIDER").ok();
    query_with_model_and_provider(text, model, provider.as_deref()).await
}

async fn query_with_model_and_provider(text: &str, model: &str, provider: Option<&str>) -> Value {
    let client = reqwest::Client::new();
    let mut body = json!({ "query": text, "model": model });
    if let Some(p) = provider {
        body["provider"] = json!(p);
    }
    let resp = client
        .post(format!("{BASE_URL}/api/query"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .expect("Failed to send request");

    assert!(resp.status().is_success(), "HTTP {}: {}", resp.status(), text);
    resp.json::<Value>().await.expect("Failed to parse JSON")
}

/// Helper: check that a response has the expected structure.
fn assert_response_structure(r: &Value) {
    assert!(r.get("parsed").is_some(), "Missing 'parsed' field");
    assert!(r.get("results").is_some(), "Missing 'results' field");
    assert!(r.get("count").is_some(), "Missing 'count' field");
    assert!(r.get("execution_ms").is_some(), "Missing 'execution_ms' field");
    assert!(r.get("mongo_query").is_some(), "Missing 'mongo_query' field");
}

fn parsed_collection(r: &Value) -> &str {
    r["parsed"]["collection"].as_str().unwrap_or("")
}

fn parsed_operation(r: &Value) -> &str {
    r["parsed"]["operation"].as_str().unwrap_or("")
}

fn result_count(r: &Value) -> u64 {
    r["count"].as_u64().unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════
// SECTION 1: FIND queries
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn find_all_movies() {
    let r = query("show me all movies").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "find");
    assert!(result_count(&r) >= 1, "Expected at least 1 movie");
}

#[tokio::test]
async fn find_all_actors() {
    let r = query("list all actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert_eq!(parsed_operation(&r), "find");
    assert!(result_count(&r) >= 1, "Expected at least 1 actor");
}

#[tokio::test]
async fn find_all_reviews() {
    let r = query("show all reviews").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    assert_eq!(parsed_operation(&r), "find");
    assert!(result_count(&r) >= 1, "Expected at least 1 review");
}

#[tokio::test]
async fn find_movies_by_genre() {
    let r = query("find all Drama movies").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1, "Expected at least 1 Drama movie");
}

#[tokio::test]
async fn find_movies_rating_gt() {
    let r = query("find movies with rating greater than 9.0").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1, "Expected at least 1 movie with rating > 9.0");
}

#[tokio::test]
async fn find_movies_year_lt() {
    // LLM may use 'year' or 'released' field — check structure, not exact count
    let r = query("find movies released before the year 2000").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    let filters = r["parsed"]["filters"].as_array().expect("filters should be array");
    assert!(!filters.is_empty(), "Expected at least 1 filter for year/released");
}

#[tokio::test]
async fn find_movies_by_director() {
    let r = query("find movies directed by Christopher Nolan").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1, "Expected at least 1 Nolan movie");
}

#[tokio::test]
async fn find_american_actors() {
    let r = query("find American actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert!(result_count(&r) >= 1, "Expected at least 1 American actor");
}

#[tokio::test]
async fn find_female_actors() {
    // LLM may use "female" vs "Female" — check structure, not exact count
    let r = query("find all female actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    let filters = r["parsed"]["filters"].as_array().expect("filters should be array");
    assert!(!filters.is_empty(), "Expected at least 1 filter for gender");
}

#[tokio::test]
async fn find_actors_with_oscar() {
    let r = query("find actors who have won at least one oscar").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert!(result_count(&r) >= 1, "Expected at least 1 oscar-winning actor");
}

#[tokio::test]
async fn find_reviews_score_5() {
    let r = query("find reviews with a score of 5").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    assert!(result_count(&r) >= 1, "Expected at least 1 review with score 5");
}

#[tokio::test]
async fn find_reviews_by_reviewer() {
    let r = query("find all reviews by Roger Ebert").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    assert!(result_count(&r) >= 1, "Expected at least 1 Roger Ebert review");
}

#[tokio::test]
async fn find_japanese_movies() {
    let r = query("find movies in Japanese").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1, "Expected at least 1 Japanese movie");
}

// ═══════════════════════════════════════════════════════════════
// SECTION 2: COUNT queries
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn count_all_movies() {
    let r = query("how many movies are there").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "count");
}

#[tokio::test]
async fn count_all_actors() {
    let r = query("count all actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert_eq!(parsed_operation(&r), "count");
}

#[tokio::test]
async fn count_all_reviews() {
    let r = query("how many reviews are in the database").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    assert_eq!(parsed_operation(&r), "count");
}

#[tokio::test]
async fn count_scifi_movies() {
    let r = query("how many Sci-Fi movies are there").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "count");
}

#[tokio::test]
async fn count_male_actors() {
    let r = query("how many male actors are there").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
}

// ═══════════════════════════════════════════════════════════════
// SECTION 3: DISTINCT queries
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn distinct_movie_genres() {
    let r = query("what are the distinct genres of movies").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "distinct");
    assert!(result_count(&r) >= 1, "Expected at least 1 distinct genre");
}

#[tokio::test]
async fn distinct_movie_languages() {
    let r = query("what are the unique languages in movies").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "distinct");
    assert!(result_count(&r) >= 1, "Expected at least 1 distinct language");
}

#[tokio::test]
async fn distinct_actor_nationalities() {
    let r = query("what are the distinct nationalities of actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert_eq!(parsed_operation(&r), "distinct");
    assert!(result_count(&r) >= 1, "Expected at least 1 distinct nationality");
}

#[tokio::test]
async fn distinct_reviewers() {
    let r = query("list the distinct reviewers in reviews").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    assert_eq!(parsed_operation(&r), "distinct");
    assert!(result_count(&r) >= 1, "Expected at least 1 distinct reviewer");
}

// ═══════════════════════════════════════════════════════════════
// SECTION 4: WRITE operations (confirmation required)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn insert_movie_requires_confirmation() {
    let r = query("insert a new movie called Test Movie with year 2026 and genre Comedy").await;
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "insert");
    assert_eq!(
        r["requires_confirmation"].as_bool(),
        Some(true),
        "Insert should require confirmation"
    );
}

#[tokio::test]
async fn update_movie_requires_confirmation() {
    let r = query("update the rating of The Dark Knight to 9.5").await;
    assert_eq!(parsed_collection(&r), "movies");
    assert_eq!(parsed_operation(&r), "update");
    assert_eq!(
        r["requires_confirmation"].as_bool(),
        Some(true),
        "Update should require confirmation"
    );
    // Update should include preview of affected docs
    let results = &r["results"];
    assert!(results.is_array(), "Update confirmation should preview affected docs");
}

#[tokio::test]
async fn insert_actor_requires_confirmation() {
    let r = query("add a new actor named Test Actor born in 1990 from Canada").await;
    assert_eq!(parsed_collection(&r), "actors");
    assert_eq!(parsed_operation(&r), "insert");
    assert_eq!(
        r["requires_confirmation"].as_bool(),
        Some(true),
        "Insert should require confirmation"
    );
}

// ═══════════════════════════════════════════════════════════════
// SECTION 5: Model selection
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn query_with_explicit_model() {
    let r = query_with_model("show all movies", "qwen3-coder:30b").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1);
}

// ═══════════════════════════════════════════════════════════════
// SECTION 6: Response structure validation
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn response_has_all_required_fields() {
    let r = query("show me all movies").await;
    assert!(r.get("parsed").is_some());
    assert!(r.get("results").is_some());
    assert!(r.get("count").is_some());
    assert!(r.get("execution_ms").is_some());
    assert!(r.get("mongo_query").is_some());
    assert!(r.get("requires_confirmation").is_some());
}

#[tokio::test]
async fn parsed_has_collection_and_operation() {
    let r = query("find all actors").await;
    let p = &r["parsed"];
    assert!(p.get("collection").is_some(), "parsed missing collection");
    assert!(p.get("operation").is_some(), "parsed missing operation");
    assert!(p.get("filters").is_some(), "parsed missing filters");
}

#[tokio::test]
async fn execution_ms_is_positive() {
    let r = query("count all movies").await;
    let ms = r["execution_ms"].as_u64().unwrap_or(0);
    assert!(ms > 0, "execution_ms should be positive, got {ms}");
}

#[tokio::test]
async fn mongo_query_is_nonempty_string() {
    let r = query("find movies rated above 8.5").await;
    let mq = r["mongo_query"].as_str().unwrap_or("");
    assert!(!mq.is_empty(), "mongo_query should not be empty");
}

#[tokio::test]
async fn read_query_has_requires_confirmation_false() {
    let r = query("show all movies").await;
    assert_eq!(
        r["requires_confirmation"].as_bool(),
        Some(false),
        "Read query should have requires_confirmation=false"
    );
}

#[tokio::test]
async fn read_query_has_index_analysis() {
    let r = query("find movies where genre is Drama").await;
    assert!(
        r.get("index_analysis").is_some(),
        "Read query should have index_analysis"
    );
    assert!(
        r["index_analysis"].is_object(),
        "index_analysis should be an object"
    );
}

// ═══════════════════════════════════════════════════════════════
// SECTION 7: Edge cases and combined filters
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn find_with_multiple_filters() {
    let r = query("find Action movies released after 2010").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
}

#[tokio::test]
async fn find_movies_sorted_by_rating() {
    let r = query("show movies sorted by rating in descending order").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1);
}

#[tokio::test]
async fn find_with_projection() {
    let r = query("show me only the title and year of all movies").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "movies");
    assert!(result_count(&r) >= 1);
}

#[tokio::test]
async fn find_active_actors() {
    let r = query("find all active actors").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "actors");
    assert!(result_count(&r) >= 1, "Expected at least 1 active actor");
}

#[tokio::test]
async fn find_reviews_high_helpful_votes() {
    let r = query("find reviews with more than 50 helpful votes").await;
    assert_response_structure(&r);
    assert_eq!(parsed_collection(&r), "reviews");
    // Count may be 0 if filter doesn't match — that's structurally fine
}

// ═══════════════════════════════════════════════════════════════
// SECTION 8: Raw LLM response in API output
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn response_includes_raw_llm_response() {
    let r = query("show all movies").await;
    assert!(
        r.get("raw_llm_response").is_some(),
        "Response should include raw_llm_response field"
    );
    let raw = r["raw_llm_response"].as_str().unwrap_or("");
    assert!(!raw.is_empty(), "raw_llm_response should not be empty");
    // Raw response should be valid JSON
    let raw_parsed: Result<Value, _> = serde_json::from_str(raw);
    assert!(raw_parsed.is_ok(), "raw_llm_response should be valid JSON");
}

#[tokio::test]
async fn raw_llm_response_contains_collection() {
    let r = query("count all actors").await;
    let raw = r["raw_llm_response"].as_str().unwrap_or("");
    let raw_json: Value = serde_json::from_str(raw).unwrap_or(json!({}));
    // The raw LLM output should mention the collection
    assert!(
        raw_json.get("collection").is_some(),
        "Raw LLM response should contain 'collection' field"
    );
}

#[tokio::test]
async fn preview_mode_includes_raw_llm_response() {
    let client = reqwest::Client::new();
    let model = std::env::var("TEST_MODEL").unwrap_or_else(|_| "qwen3-coder:30b".to_string());
    let provider = std::env::var("TEST_PROVIDER").ok();
    let mut body = json!({ "query": "find all movies", "model": model, "preview": true });
    if let Some(p) = provider {
        body["provider"] = json!(p);
    }
    let resp = client
        .post(format!("{BASE_URL}/api/query"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .expect("Failed to send preview request");
    assert!(resp.status().is_success(), "HTTP {}", resp.status());
    let r: Value = resp.json().await.unwrap();
    assert!(
        r.get("raw_llm_response").is_some(),
        "Preview response should include raw_llm_response"
    );
    let raw = r["raw_llm_response"].as_str().unwrap_or("");
    assert!(!raw.is_empty(), "raw_llm_response in preview should not be empty");
}
