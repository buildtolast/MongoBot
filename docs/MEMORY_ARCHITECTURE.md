# Memory & State Architecture

> This document explains how the system maintains "memory" through two primary mechanisms: **Runtime Schema Memory** (for query accuracy) and **Persistent Learning Memory** (for continuous improvement).

---

## 1. Overview

The Slack-Mongo Query Bot manages two distinct types of state to ensure high-performance querying and long-term model improvement:

| Memory Type | Scope | Purpose | Storage |
| :--- | :--- | :--- | :--- |
| **Runtime Schema Memory** | Transient | Provides LLM with context on field names/types for accurate query generation. | `DashMap` in `AppState` (In-memory) |
| **Learning Memory (DPO)** | Persistent | Captures user corrections to build a dataset for model fine-tuning. | `query_feedback` collection (MongoDB) |

---

## 2. Runtime Schema Memory (Schema Discovery & Caching)

To generate accurate MongoDB queries, the LLM must know the structure of the target collections (field names, data types, etc.).

### **Mechanism: Runtime Introspection**
Instead of requiring manual schema definitions, the system uses **runtime introspection**:
1.  **Sampling:** The system samples up to 50 documents from each collection.
2.  **Inference:** It recursively traverses documents to discover nested fields (using dot notation) and infers BSON types (e.g., `objectId`, `string`, `date`, `number`).
3.  **Array Introspection:** For array fields, it dives into the first element to discover the schema of the objects within the array, enabling queries like `transactions.amount`.

### **Storage: The `DashMap` Cache**
*   **Location:** `AppState.schemas` (`DashMap<String, CollectionSchema>`).
*   **Concurrency:** Uses `DashMap` to provide lock-free, high-concurrency reads, ensuring that schema lookups do not become a bottleneck for the query pipeline.
*   **Lifecycle:**
    *   **Startup:** Initial discovery of all collections in the configured database.
    *   **Hot Reload:** Users can trigger a refresh via `POST /api/schemas/refresh` to detect new collections or schema changes without a restart.
    *   **Enrichment:** The raw introspected schema is merged with human-readable descriptions from YAML files in the `schemas/` directory.

---

## 3. Persistent Learning Memory (The DPO Loop)

The system is designed to implement **Direct Preference Optimization (DPO)**. By capturing how users correct the LLM's initial output, we create a high-quality "Chosen vs. Rejected" dataset.

### **The Data Model: `QueryFeedback`**
Every feedback entry in the `query_feedback` collection contains:
*   **`nl_query`**: The original natural language input.
*   **`parsed_query`**: The LLM's initial (potentially incorrect) structured output.
*   **`user_edited_query`**: The version the user manually corrected and approved.
*   **`mongo_query`**: The actual BSON command that was successfully executed.
*   **`feedback_type`**: 
    *   `Corrected`: User edited the query (Critical for DPO).
    *   `Liked`: User approved the LLM output as-is.
    *   `Disliked`: User rejected the output.
    *   `Abandoned`: User closed the query without action.

### **Capture Points**
1.  **Implicit Capture (The Correction Flow):** 
    When a user enters **Preview Mode**, they can edit the `ParsedQuery` JSON. If they then click "Execute," the system automatically creates a `Corrected` feedback record, pairing the `parsed_query` (Rejected) with the `user_edited_query` (Chosen).
2.  **Explicit Capture (The Rating Flow):** 
    Users can click "Like" or "Dislike" icons in the Web UI to provide direct feedback on the LLM's performance.

### **Usage in Machine Learning**
The resulting dataset can be exported (via `/api/export?format=dpo-jsonl`) to train a model using DPO, teaching the LLM to prefer the user's corrected query structure over its own initial hallucination.
