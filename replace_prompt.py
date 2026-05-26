import sys

file_path = 'src/infra/llm.rs'

new_function_lines = [
    'pub(crate) fn build_prompt_with_examples(query: &str, schema: &str, examples: &[crate::domain::models::QueryFeedback]) -> String {',
    '    let mut examples_section = String::new();',
    '    if !examples.is_empty() {',
    '        examples_section.push_str("\\n## EXAMPLES OF SUCCESSFUL TRANSLATIONS\\n");',
    '        for (i, fb) in examples.iter().enumerate() {',
    '            let parsed_json = serde_json::to_string(&fb.parsed_query).unwrap_or_else(|_| "{}".to_string());',
    '            examples_section.push_str(&format!("\\n### Example {}\\n", i + 1));',
    '            examples_section.push_str(&format!("Input: \\"{}\\"\\n", fb.nl_query));',
    '            examples_section.push_str(&format!("Output: {}\\n", parsed_json));',
    '        }',
    '    }',
    '',
    '    format!(',
    '        r#\"# SYSTEM PROMPT',
    'You are a MongoDB query translator. Return ONLY valid JSON. No explanation.',
'',
'## COLLECTIONS SCHEMA',
'{schema}',
'',
'## RULES',
'- Use ONLY schema fields.',
'- Use dot notation for nested fields (e.g., "salary.bonus").',
'- Dates must be ISO 8601 ("YYYY-MM-DDT00:00:00Z").',
'- For "distinct" operations, include the field in the "projection".',
'- For "aggregate" operations, use the "group_by" field.',
'- For complex queries, use "raw_pipeline" or "raw_filter" with native MongoDB syntax.',
'- Forbidden: $out, $merge, $function, $accumulator, $where.',
'',
'## REQUIRED FIELDS',
'- "collection": (string)',
'- "operation": (string) "find" | "count" | "distinct" | "aggregate" | "insert" | "update" | "delete"',
'- "filters": (array)',
'- "limit": (number, default 100)',
'',
'## OPTIONAL FIELDS',
'- "projection": (array of strings)',
'- "sort": (object) {{"field": "string", "order": 1 | -1}}',
'- "or_filters": (array of filter arrays)',
'- "text_search": (string)',
'- "expr": (object) e.g., {{"$gt": ["$field", 10]}}',
'- "group_by": (object) {{"field": "string", "accumulators": [{{"output_field": "string", "operator": "$sum", "input_field": "string"}}]}}',
'- "lookup": (object) {{"from": "string", "local_field": "string", "foreign_field": "string", "as_field": "string"}}',
'- "unwind": (string)',
'- "bucket": (object) {{"group_by": "string", "boundaries": [number]}}',
'- "raw_pipeline": (array of objects)',
'- "raw_filter": (object)',
'- "document": (object) for insert',
'- "update_fields": (object) for update',
'- "update_many": (boolean)',
'- "delete_many": (boolean)',
'',
'{examples}',
'',
'## USER QUERY',
'{query}"#',
    '        schema = schema,',
    '        examples = examples_section,',
    '        query = query',
    ')',
'}'
]

with open(file_path, 'r') as f:
    lines = f.readlines()

# The function starts at line 411 (1-indexed)
# So index 410
start_idx = 410
# The function ends at line 441 (1-indexed)
# So end_idx is 441
end_idx = 441

# Verify start line
if not lines[start_idx].strip().startswith("pub(crate) fn build_prompt_with_examples"):
    print(f"Error: Start line mismatch. Found: {lines[start_idx].strip()}")
    sys.exit(1)

# Perform replacement
new_lines = lines[:start_idx] + [line + '\n' for line in new_function_lines] + lines[end_idx:]

with open(file_path, 'w') as f:
    f.writelines(new_lines)

print("Success: Function replaced.")
