# MongoDB Query Reference for Validation & Optimization

Comprehensive reference for building a query validation/optimization skill. Covers operators, aggregation stages, indexing, anti-patterns, and best practices with practical examples.

---

## Table of Contents

1. [Comparison Operators](#1-comparison-operators)
2. [Logical Operators](#2-logical-operators)
3. [Element Operators](#3-element-operators)
4. [Evaluation Operators](#4-evaluation-operators)
5. [Array Operators](#5-array-operators)
6. [Aggregation Pipeline Stages](#6-aggregation-pipeline-stages)
7. [Index Best Practices](#7-index-best-practices)
8. [Query Optimization](#8-query-optimization)
9. [Write Operation Safety](#9-write-operation-safety)
10. [Date Query Patterns](#10-date-query-patterns)
11. [Array Query Patterns](#11-array-query-patterns)
12. [Text Search](#12-text-search)
13. [Common Mistakes](#13-common-mistakes)
14. [Validation Rules Summary](#14-validation-rules-summary)

---

## 1. Comparison Operators

### $eq — Equals

```javascript
// Explicit
db.users.find({ status: { $eq: "active" } })

// Implicit (preferred shorthand)
db.users.find({ status: "active" })
```

**Validation note:** `{ field: { $eq: value } }` is equivalent to `{ field: value }`. Prefer the implicit form for readability.

### $ne — Not Equal

```javascript
db.users.find({ status: { $ne: "deleted" } })
```

**Warning:** `$ne` cannot use indexes efficiently. It performs an index scan of all values except the negated one, plus a collection scan for documents missing the field. Prefer positive matches when possible.

### $gt, $gte, $lt, $lte — Range Comparisons

```javascript
// Greater than
db.orders.find({ total: { $gt: 100 } })

// Greater than or equal
db.orders.find({ total: { $gte: 100 } })

// Less than
db.orders.find({ total: { $lt: 500 } })

// Less than or equal
db.orders.find({ total: { $lte: 500 } })

// Range (combined)
db.orders.find({ total: { $gte: 100, $lte: 500 } })
```

**Index usage:** Range operators use indexes but scan a range of the B-tree. Place range fields last in compound indexes (see ESR rule).

### $in — Matches Any Value in Array

```javascript
db.users.find({ role: { $in: ["admin", "moderator"] } })
```

**Performance notes:**
- `$in` uses indexes efficiently for small arrays (typically < 200 values).
- Large `$in` arrays (1000+ values) degrade to near-collection-scan performance.
- Each value in `$in` creates a separate index lookup, so very large arrays are expensive.
- For large sets, consider restructuring data or using `$lookup` in aggregation.

### $nin — Not In

```javascript
db.users.find({ status: { $nin: ["deleted", "banned"] } })
```

**Warning:** Like `$ne`, `$nin` cannot use indexes efficiently. It scans all index entries not in the list. Avoid on large collections.

---

## 2. Logical Operators

### $and — Logical AND

```javascript
// Explicit $and
db.users.find({
  $and: [
    { age: { $gte: 18 } },
    { status: "active" }
  ]
})

// Implicit AND (preferred when fields differ)
db.users.find({ age: { $gte: 18 }, status: "active" })

// Explicit $and required: same field, multiple conditions
db.products.find({
  $and: [
    { price: { $gt: 10 } },
    { price: { $lt: 100 } }
  ]
})
// OR simply:
db.products.find({ price: { $gt: 10, $lt: 100 } })
```

**When explicit `$and` is required:** When you need multiple conditions on the same field using the same operator, or when combining `$or` expressions.

### $or — Logical OR

```javascript
db.users.find({
  $or: [
    { status: "active" },
    { role: "admin" }
  ]
})
```

**Index usage:** Each clause in `$or` can use a different index. MongoDB performs an index scan for each clause and merges results (deduplication). For best performance, ensure every `$or` clause is indexed.

**Optimization tip:** If all `$or` clauses query the same field, use `$in` instead:

```javascript
// Suboptimal
db.users.find({ $or: [{ status: "active" }, { status: "pending" }] })

// Better
db.users.find({ status: { $in: ["active", "pending"] } })
```

### $not — Negation

```javascript
db.products.find({ price: { $not: { $gt: 100 } } })
```

**Warning:** `$not` with `$regex` cannot use indexes. Rewrite if possible.

### $nor — Not OR

```javascript
db.users.find({
  $nor: [
    { status: "deleted" },
    { banned: true }
  ]
})
```

**Warning:** `$nor` generally cannot use indexes. Each clause is negated, forcing scans. Avoid on large collections.

---

## 3. Element Operators

### $exists — Field Existence

```javascript
// Documents where email field exists
db.users.find({ email: { $exists: true } })

// Documents where email field does not exist
db.users.find({ email: { $exists: false } })
```

**Index usage:** `$exists: true` can use a sparse index on the field. `$exists: false` requires scanning documents without the field, which is expensive.

**Optimization:** If you frequently query `$exists: true`, consider a partial index:

```javascript
db.users.createIndex(
  { email: 1 },
  { partialFilterExpression: { email: { $exists: true } } }
)
```

### $type — BSON Type Check

```javascript
// Find documents where age is a number (any numeric BSON type)
db.users.find({ age: { $type: "number" } })

// Find documents where field is a string
db.users.find({ name: { $type: "string" } })

// Multiple types
db.users.find({ value: { $type: ["string", "int"] } })
```

**Common BSON type identifiers:** `"double"` (1), `"string"` (2), `"object"` (3), `"array"` (4), `"binData"` (5), `"objectId"` (7), `"bool"` (8), `"date"` (9), `"null"` (10), `"regex"` (11), `"int"` (16), `"long"` (18), `"decimal"` (19), `"number"` (alias for int, long, double, decimal).

---

## 4. Evaluation Operators

### $regex — Regular Expression

```javascript
// Case-sensitive prefix match (uses index)
db.users.find({ name: { $regex: /^John/ } })

// Case-insensitive (cannot use index efficiently)
db.users.find({ name: { $regex: /john/i } })

// With $options
db.users.find({ name: { $regex: "^John", $options: "i" } })
```

**Index rules for $regex:**
- Anchored prefix (`/^prefix/`) without `i` flag: uses index efficiently (prefix range scan).
- Anchored prefix with `i` flag: full index scan (cannot leverage prefix optimization).
- Unanchored pattern (`/pattern/`): full index/collection scan. Very expensive.
- Never use `$not` with `$regex` -- always causes full collection scan.

**Recommendation:** For case-insensitive searches on Atlas, use `$search` (Atlas Search) instead of `$regex` with the `i` flag.

### $expr — Aggregation Expressions in Queries

```javascript
// Compare two fields in the same document
db.budgets.find({
  $expr: { $gt: ["$spent", "$budget"] }
})

// Use with $cond for conditional logic
db.orders.find({
  $expr: {
    $lt: [
      { $cond: {
        if: { $gte: ["$quantity", 100] },
        then: { $multiply: ["$price", 0.9] },
        else: "$price"
      }},
      50
    ]
  }
})

// Use in $match stage (allows aggregation operators)
db.orders.aggregate([
  { $match: {
    $expr: { $gt: ["$total", "$discount"] }
  }}
])
```

**Performance:** `$expr` with aggregation operators (non-`$function`, non-`$accumulator`) is faster than `$where` and is the preferred alternative. `$expr` can use indexes when comparing a field to a constant but not when comparing two fields.

### $where — JavaScript Expression (AVOID)

```javascript
// DANGEROUS AND SLOW -- avoid in production
db.users.find({
  $where: function() { return this.age > this.minAge }
})
```

**Why to avoid `$where`:**
- Executes JavaScript for every document -- no index usage, full collection scan.
- Security risk: JavaScript injection if user input reaches `$where`.
- 10x-100x slower than equivalent `$expr` queries.
- Can cause CPU saturation (denial of service).
- Disable JavaScript execution in mongod.conf (`security.javascriptEnabled: false`) when not needed.

**Always rewrite `$where` as `$expr`:**

```javascript
// BAD: $where
db.users.find({ $where: "this.spent > this.budget" })

// GOOD: $expr
db.users.find({ $expr: { $gt: ["$spent", "$budget"] } })
```

---

## 5. Array Operators

### $elemMatch — Match Array Element

```javascript
// Match array element satisfying ALL conditions
db.scores.find({
  results: { $elemMatch: { $gte: 80, $lt: 90 } }
})

// With embedded documents
db.survey.find({
  answers: {
    $elemMatch: { question: "Q1", score: { $gte: 8 } }
  }
})
```

### $all — Match All Values

```javascript
// Documents where tags contains both "mongodb" and "database"
db.articles.find({
  tags: { $all: ["mongodb", "database"] }
})
```

**Equivalent to:** `{ $and: [{ tags: "mongodb" }, { tags: "database" }] }`

### $size — Exact Array Length

```javascript
// Documents where tags array has exactly 3 elements
db.articles.find({ tags: { $size: 3 } })
```

**Limitations:**
- `$size` does not accept ranges. You cannot do `{ $size: { $gt: 3 } }`.
- `$size` cannot use indexes.
- For range queries on array length, add a computed `arrayLength` field or use `$expr`:

```javascript
// Range on array length using $expr
db.articles.find({
  $expr: { $gt: [{ $size: "$tags" }, 3] }
})
```

---

## 6. Aggregation Pipeline Stages

### Stage Reference

| Stage | Purpose | Index Usage | Blocking? |
|-------|---------|-------------|-----------|
| `$match` | Filter documents | Yes (if first or after other index-compatible stages) | No |
| `$project` / `$addFields` / `$set` | Reshape/add fields | No | No |
| `$unset` | Remove fields | No | No |
| `$group` | Aggregate/summarize | No | Yes (blocking) |
| `$sort` | Order results | Yes (if matches index) | Yes (blocking, 100MB memory limit) |
| `$limit` | Cap result count | No | No |
| `$skip` | Skip N documents | No | No |
| `$unwind` | Flatten arrays | No | No |
| `$lookup` | Join collections | Yes (on foreign collection) | No |
| `$facet` | Parallel sub-pipelines | No | Yes |
| `$bucket` / `$bucketAuto` | Bucket by ranges | No | Yes |
| `$out` | Write to collection | N/A | Yes (must be last) |
| `$merge` | Merge to collection | N/A | Yes (must be last) |

### $match

```javascript
db.orders.aggregate([
  { $match: { status: "completed", total: { $gte: 100 } } }
])
```

**Best practice:** Always place `$match` as early as possible. A `$match` at the start of the pipeline can use indexes. After a `$group` or `$unwind`, it cannot.

### $group

```javascript
db.orders.aggregate([
  { $group: {
    _id: "$customerId",
    totalSpent: { $sum: "$total" },
    orderCount: { $sum: 1 },
    avgOrder: { $avg: "$total" },
    lastOrder: { $max: "$createdAt" },
    items: { $push: "$item" }
  }}
])
```

**Accumulators:** `$sum`, `$avg`, `$min`, `$max`, `$push`, `$addToSet`, `$first`, `$last`, `$stdDevPop`, `$stdDevSamp`, `$mergeObjects`, `$count`.

### $sort

```javascript
db.orders.aggregate([
  { $sort: { createdAt: -1 } }      // Descending
])

// Sort on multiple fields
db.orders.aggregate([
  { $sort: { status: 1, createdAt: -1 } }
])
```

**Memory limit:** `$sort` has a 100MB memory limit. For larger sorts, use `{ allowDiskUse: true }` or add an index that covers the sort.

### $project / $addFields / $set / $unset

```javascript
// $project: include/exclude fields
db.users.aggregate([
  { $project: { name: 1, email: 1, _id: 0 } }
])

// $addFields / $set (aliases): add computed fields
db.orders.aggregate([
  { $addFields: {
    totalWithTax: { $multiply: ["$total", 1.1] },
    year: { $year: "$createdAt" }
  }}
])

// $unset: remove fields
db.users.aggregate([
  { $unset: ["password", "ssn", "internalNotes"] }
])
```

**Optimization note:** Do not place `$project` early in the pipeline to reduce fields. MongoDB's query optimizer does this automatically. Place `$project` last to shape the final output.

### $limit / $skip

```javascript
// Pagination pattern
db.products.aggregate([
  { $match: { category: "electronics" } },
  { $sort: { price: 1 } },
  { $skip: 20 },
  { $limit: 10 }
])
```

**Warning:** `$skip` is expensive on large offsets because MongoDB still processes all skipped documents. For large datasets, use range-based pagination with a cursor:

```javascript
// Range-based pagination (much faster)
db.products.find({
  category: "electronics",
  _id: { $gt: ObjectId("last_seen_id") }
}).sort({ _id: 1 }).limit(10)
```

### $unwind

```javascript
db.orders.aggregate([
  { $unwind: "$items" },
  { $group: { _id: "$items.product", totalSold: { $sum: "$items.qty" } } }
])

// Preserve null/empty arrays
db.orders.aggregate([
  { $unwind: { path: "$items", preserveNullAndEmptyArrays: true } }
])
```

**Anti-pattern:** Do not use `$unwind` + `$group` to transform array elements within the same document. Use array operators (`$map`, `$filter`, `$reduce`) instead, as they avoid the blocking `$group` stage.

### $lookup — Join

```javascript
// Basic join
db.orders.aggregate([
  { $lookup: {
    from: "customers",
    localField: "customerId",
    foreignField: "_id",
    as: "customer"
  }},
  { $unwind: "$customer" }  // Convert array to single doc
])

// Pipeline lookup (more flexible)
db.orders.aggregate([
  { $lookup: {
    from: "products",
    let: { productIds: "$items.productId" },
    pipeline: [
      { $match: { $expr: { $in: ["$_id", "$$productIds"] } } },
      { $project: { name: 1, price: 1 } }
    ],
    as: "productDetails"
  }}
])
```

**Performance:** Ensure the `foreignField` is indexed. Without an index on the foreign collection's join field, every `$lookup` does a collection scan.

### $facet — Parallel Sub-pipelines

```javascript
db.products.aggregate([
  { $facet: {
    priceRanges: [
      { $bucket: { groupBy: "$price", boundaries: [0, 50, 100, 500], default: "500+" } }
    ],
    topRated: [
      { $sort: { rating: -1 } },
      { $limit: 5 }
    ],
    totalCount: [
      { $count: "count" }
    ]
  }}
])
```

### $bucket

```javascript
db.transactions.aggregate([
  { $bucket: {
    groupBy: "$amount",
    boundaries: [0, 100, 500, 1000, 5000],
    default: "5000+",
    output: {
      count: { $sum: 1 },
      total: { $sum: "$amount" }
    }
  }}
])
```

---

## 7. Index Best Practices

### The ESR Rule (Equality, Sort, Range)

The ESR rule defines the optimal field order for compound indexes:

1. **E**quality fields first -- fields queried with `$eq` or implicit equality
2. **S**ort fields second -- fields used in `.sort()`
3. **R**ange fields last -- fields queried with `$gt`, `$gte`, `$lt`, `$lte`, `$in`

```javascript
// Query pattern:
db.orders.find({
  status: "shipped",          // Equality
  total: { $gte: 100 }       // Range
}).sort({ createdAt: -1 })    // Sort

// Optimal index (ESR):
db.orders.createIndex({ status: 1, createdAt: -1, total: 1 })
//                       ^Equality    ^Sort          ^Range
```

**Why ESR works:**
- Equality narrows to exact values (most selective).
- Sort on the narrowed set avoids in-memory sorting.
- Range scan happens on the smallest possible set.

### Covered Queries

A covered query is satisfied entirely from the index without accessing documents.

```javascript
// Index
db.users.createIndex({ email: 1, name: 1, age: 1 })

// Covered query (all fields in index, _id excluded)
db.users.find(
  { email: "user@example.com" },
  { email: 1, name: 1, age: 1, _id: 0 }
)
```

**Requirements for covered queries:**
- All queried fields must be in the index.
- All projected fields must be in the index.
- `_id: 0` must be set in projection (unless `_id` is in the index).
- No array fields in the index (multikey indexes cannot cover queries).

### Compound Index Design

```javascript
// Supports queries on: {a}, {a, b}, {a, b, c}
// Does NOT efficiently support: {b}, {c}, {b, c}
db.collection.createIndex({ a: 1, b: 1, c: 1 })
```

**Prefix rule:** A compound index supports queries on any prefix of its fields, from left to right.

### Index Selectivity

Place the most selective field (highest cardinality) first among equality fields:

```javascript
// If userId has 100,000 distinct values and status has 5:
// Good: userId first (more selective)
db.orders.createIndex({ userId: 1, status: 1 })

// Less optimal: status first
db.orders.createIndex({ status: 1, userId: 1 })
```

### Partial Indexes

```javascript
// Only index active users (saves space and write overhead)
db.users.createIndex(
  { email: 1 },
  { partialFilterExpression: { status: "active" } }
)

// Query MUST include the partial filter to use this index
db.users.find({ email: "x@y.com", status: "active" })  // Uses index
db.users.find({ email: "x@y.com" })                     // Does NOT use index
```

### TTL Indexes

```javascript
// Auto-delete documents 30 days after createdAt
db.sessions.createIndex(
  { createdAt: 1 },
  { expireAfterSeconds: 2592000 }
)
```

### Wildcard Indexes

```javascript
// Index all fields in metadata subdocument
db.logs.createIndex({ "metadata.$**": 1 })

// Query any field under metadata
db.logs.find({ "metadata.region": "us-east-1" })
```

---

## 8. Query Optimization

### Use Explain Plans

```javascript
// Basic explain
db.orders.find({ status: "active" }).explain()

// Detailed execution stats
db.orders.find({ status: "active" }).explain("executionStats")

// All plans considered
db.orders.find({ status: "active" }).explain("allPlansExecution")
```

**Key fields to check:**
- `winningPlan.stage`: Should be `IXSCAN` (not `COLLSCAN`).
- `totalDocsExamined` vs `nReturned`: Ratio close to 1:1 is ideal.
- `totalKeysExamined` vs `nReturned`: High ratio suggests index inefficiency.
- `executionTimeMillis`: Actual query time.

### Projections

```javascript
// BAD: fetch entire document when you need 2 fields
db.users.find({ status: "active" })

// GOOD: project only needed fields
db.users.find(
  { status: "active" },
  { name: 1, email: 1, _id: 0 }
)
```

**Impact:** Projections reduce document size transferred over the network and can enable covered queries.

### Hint for Index Selection

```javascript
// Force a specific index
db.orders.find({ status: "active", createdAt: { $gte: lastWeek } })
  .hint({ status: 1, createdAt: -1 })

// Force collection scan (for testing)
db.orders.find({ status: "active" }).hint({ $natural: 1 })
```

**Use sparingly.** The query optimizer usually chooses well. Use `hint()` only after `explain()` confirms a suboptimal plan.

### Avoid $not with $regex

```javascript
// BAD: full collection scan, no index usage
db.users.find({ name: { $not: /^admin/ } })

// BETTER: use $expr or restructure
db.users.find({
  $and: [
    { name: { $exists: true } },
    { name: { $not: { $eq: null } } },
    // Application-level filtering for negated regex
  ]
})
```

### Limit and BatchSize

```javascript
// Always limit results for user-facing queries
db.logs.find({ level: "error" }).sort({ ts: -1 }).limit(100)

// Control batch size for large result sets
db.events.find({}).batchSize(500)
```

---

## 9. Write Operation Safety

### bulkWrite vs Loops

```javascript
// BAD: individual writes in a loop (N network round-trips)
for (const doc of documents) {
  await db.orders.insertOne(doc);
}

// GOOD: bulkWrite (single round-trip, batched)
await db.orders.bulkWrite([
  { insertOne: { document: { item: "A", qty: 10 } } },
  { insertOne: { document: { item: "B", qty: 20 } } },
  { updateOne: {
    filter: { item: "C" },
    update: { $set: { qty: 30 } }
  }},
  { deleteOne: { filter: { item: "D" } } }
]);

// Unordered for better performance (continues on error)
await db.orders.bulkWrite(operations, { ordered: false });
```

**Ordered vs unordered:**
- `ordered: true` (default): stops on first error, sequential execution.
- `ordered: false`: continues after errors, may execute in parallel on sharded clusters.

### Write Concern

```javascript
// Default: w:1 (acknowledged by primary)
db.orders.insertOne({ item: "A" })

// Majority write concern (safest for replica sets)
db.orders.insertOne(
  { item: "A" },
  { writeConcern: { w: "majority", wtimeout: 5000 } }
)

// w:0 fire-and-forget (fastest, no error detection)
db.logs.insertOne(
  { message: "debug info" },
  { writeConcern: { w: 0 } }
)
```

**Guidance:**
- Use `w: "majority"` for critical data (financial transactions, user accounts).
- Use `w: 1` for most application data (default, good balance).
- Use `w: 0` only for non-critical telemetry/logs where loss is acceptable.

### Dangerous updateMany Patterns

```javascript
// DANGEROUS: empty filter updates ALL documents
db.users.updateMany({}, { $set: { role: "basic" } })

// DANGEROUS: accidental filter match-all
db.users.updateMany(
  { status: { $ne: null } },  // Matches almost everything
  { $set: { verified: false } }
)

// SAFER: always use specific filters
db.users.updateMany(
  { role: "trial", createdAt: { $lt: cutoffDate } },
  { $set: { status: "expired" } }
)
```

**Validation rules for write safety:**
- Flag `updateMany({}, ...)` -- empty filter is almost always a mistake.
- Flag `deleteMany({}, ...)` -- empty filter deletes everything.
- Flag `updateMany` / `deleteMany` where the filter is likely to match all documents.
- Suggest `limit` or confirm intent for broad updates.

### Replace vs Update

```javascript
// replaceOne: overwrites the ENTIRE document (dangerous)
db.users.replaceOne(
  { _id: userId },
  { name: "New Name" }  // All other fields are LOST
)

// updateOne with $set: modifies specific fields (safe)
db.users.updateOne(
  { _id: userId },
  { $set: { name: "New Name" } }  // Other fields preserved
)
```

---

## 10. Date Query Patterns

### Storing Dates

```javascript
// Always store as native BSON Date (not strings)
db.events.insertOne({
  name: "Deploy",
  createdAt: new Date(),                          // Current time
  scheduledAt: ISODate("2026-03-15T14:30:00Z")    // Specific time
})
```

**Rule:** Never store dates as strings. BSON Date supports indexing, range queries, and aggregation date operators. Strings do not sort correctly and cannot use date math.

### Date Range Queries

```javascript
// Events today
const startOfDay = new Date(); startOfDay.setHours(0,0,0,0);
const endOfDay = new Date(); endOfDay.setHours(23,59,59,999);

db.events.find({
  createdAt: { $gte: startOfDay, $lte: endOfDay }
})

// Last 7 days
const sevenDaysAgo = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000);
db.events.find({
  createdAt: { $gte: sevenDaysAgo }
})

// Specific month
db.events.find({
  createdAt: {
    $gte: ISODate("2026-03-01T00:00:00Z"),
    $lt: ISODate("2026-04-01T00:00:00Z")    // Exclusive upper bound
  }
})
```

**Best practice:** Use `$gte` + `$lt` (exclusive upper bound) for date ranges to avoid boundary issues with millisecond precision.

### Aggregation Date Operators

```javascript
db.orders.aggregate([
  { $addFields: {
    year: { $year: "$createdAt" },
    month: { $month: "$createdAt" },
    dayOfWeek: { $dayOfWeek: "$createdAt" },
    formatted: {
      $dateToString: {
        format: "%Y-%m-%d",
        date: "$createdAt",
        timezone: "America/New_York"
      }
    }
  }}
])

// Group by day
db.orders.aggregate([
  { $group: {
    _id: {
      $dateToString: { format: "%Y-%m-%d", date: "$createdAt" }
    },
    count: { $sum: 1 },
    total: { $sum: "$amount" }
  }},
  { $sort: { _id: -1 } }
])
```

**Date operators:** `$year`, `$month`, `$dayOfMonth`, `$hour`, `$minute`, `$second`, `$millisecond`, `$dayOfWeek`, `$dayOfYear`, `$week`, `$dateToString`, `$dateFromString`, `$dateFromParts`, `$dateToParts`, `$dateAdd`, `$dateSubtract`, `$dateDiff`.

---

## 11. Array Query Patterns

### Querying Arrays

```javascript
// Match any element in array
db.articles.find({ tags: "mongodb" })

// Match exact array (order matters)
db.articles.find({ tags: ["mongodb", "database"] })

// Match array containing all values (any order)
db.articles.find({ tags: { $all: ["mongodb", "database"] } })
```

### $elemMatch vs Dot Notation

**Critical difference:** Dot notation matches conditions across different array elements. `$elemMatch` requires all conditions on the same element.

```javascript
// Data: { scores: [ { subject: "math", grade: 90 }, { subject: "english", grade: 70 } ] }

// Dot notation: matches if ANY element has subject "math" AND ANY element has grade >= 80
// These can be DIFFERENT elements -- potential false positive
db.students.find({
  "scores.subject": "math",
  "scores.grade": { $gte: 80 }
})
// Returns the document (math=90 satisfies grade>=80, separately)

// $elemMatch: requires SAME element to satisfy both conditions
db.students.find({
  scores: {
    $elemMatch: { subject: "math", grade: { $gte: 80 } }
  }
})
// Returns the document (math element has grade 90, both conditions met)
```

**When to use each:**
- **Dot notation:** Single condition on an array field, or when cross-element matching is acceptable.
- **`$elemMatch`:** Multiple conditions that must apply to the same array element.

### Positional Operators for Updates

```javascript
// $ -- update first matched element
db.students.updateOne(
  { _id: studentId, "scores.subject": "math" },
  { $set: { "scores.$.grade": 95 } }
)

// $[] -- update ALL elements
db.students.updateOne(
  { _id: studentId },
  { $inc: { "scores.$[].grade": 5 } }  // Add 5 to every grade
)

// $[identifier] -- update elements matching a filter
db.students.updateOne(
  { _id: studentId },
  { $set: { "scores.$[elem].grade": 100 } },
  { arrayFilters: [{ "elem.subject": "math" }] }
)
```

### Array Aggregation Operators

```javascript
// $filter -- filter array elements
db.orders.aggregate([
  { $project: {
    expensiveItems: {
      $filter: {
        input: "$items",
        as: "item",
        cond: { $gte: ["$$item.price", 100] }
      }
    }
  }}
])

// $map -- transform array elements
db.orders.aggregate([
  { $project: {
    itemNames: {
      $map: {
        input: "$items",
        as: "item",
        in: "$$item.name"
      }
    }
  }}
])

// $reduce -- reduce array to single value
db.orders.aggregate([
  { $project: {
    totalQty: {
      $reduce: {
        input: "$items",
        initialValue: 0,
        in: { $add: ["$$value", "$$this.qty"] }
      }
    }
  }}
])
```

---

## 12. Text Search

### Text Index Creation

```javascript
// Single field
db.articles.createIndex({ content: "text" })

// Multiple fields with weights
db.articles.createIndex(
  { title: "text", content: "text", tags: "text" },
  { weights: { title: 10, content: 5, tags: 1 } }
)
```

**Constraint:** A collection can have at most ONE text index (but it can cover multiple fields).

### $text Queries

```javascript
// Search for any of the terms (OR logic)
db.articles.find({ $text: { $search: "mongodb database" } })

// Exact phrase
db.articles.find({ $text: { $search: "\"mongodb database\"" } })

// Negation (exclude term)
db.articles.find({ $text: { $search: "mongodb -deprecated" } })

// Sort by relevance score
db.articles.find(
  { $text: { $search: "mongodb tutorial" } },
  { score: { $meta: "textScore" } }
).sort({ score: { $meta: "textScore" } })

// Case-insensitive and diacritic-insensitive by default
// Opt out:
db.articles.find({
  $text: { $search: "cafe", $caseSensitive: true, $diacriticSensitive: true }
})
```

### Text Search Limitations

- One text index per collection.
- No partial/substring matching (not a LIKE replacement).
- Stemming is language-dependent (English by default).
- Stop words are automatically excluded (a, an, the, is, etc.).
- Cannot do fuzzy matching or autocomplete.
- For advanced search (fuzzy, autocomplete, facets, synonyms), use Atlas Search (`$search` stage) instead of `$text`.

---

## 13. Common Mistakes

### Mistake 1: Missing Limit on Unbounded Queries

```javascript
// BAD: returns ALL matching documents (could be millions)
db.logs.find({ level: "info" })

// GOOD: always limit
db.logs.find({ level: "info" }).limit(1000)
```

**Validation rule:** Flag any `find()` without `.limit()` on collections known to be large.

### Mistake 2: Collection Scan on Large Collections

```javascript
// BAD: no index on 'email', causes COLLSCAN
db.users.find({ email: "user@example.com" })

// FIX: create index
db.users.createIndex({ email: 1 })
```

**Validation rule:** Check `explain()` output for `COLLSCAN` stage.

### Mistake 3: Using $regex Without Anchor

```javascript
// BAD: scans entire index or collection
db.users.find({ name: { $regex: /smith/ } })

// BETTER: anchored prefix
db.users.find({ name: { $regex: /^Smith/ } })

// BEST: use text index or Atlas Search for general text matching
```

### Mistake 4: Large $in Arrays

```javascript
// BAD: thousands of values in $in
db.orders.find({ productId: { $in: arrayOf5000Ids } })

// BETTER: use $lookup or restructure
db.orders.aggregate([
  { $lookup: {
    from: "targetProducts",
    localField: "productId",
    foreignField: "_id",
    as: "matched"
  }},
  { $match: { matched: { $ne: [] } } }
])
```

### Mistake 5: Using $where Instead of $expr

```javascript
// BAD: JavaScript execution, no index, security risk
db.orders.find({ $where: "this.total > this.budget" })

// GOOD: native operator, faster, safer
db.orders.find({ $expr: { $gt: ["$total", "$budget"] } })
```

### Mistake 6: Fetching Entire Documents

```javascript
// BAD: fetches all fields when you only need name
const users = await db.users.find({ status: "active" }).toArray()
users.map(u => u.name)

// GOOD: project at the database level
const names = await db.users.find(
  { status: "active" },
  { projection: { name: 1, _id: 0 } }
).toArray()
```

### Mistake 7: Wrong Field Order in Compound Index

```javascript
// Query:
db.orders.find({ customerId: "abc", total: { $gt: 100 } }).sort({ createdAt: -1 })

// BAD index order (range before sort):
db.orders.createIndex({ customerId: 1, total: 1, createdAt: -1 })
// Causes in-memory sort

// GOOD index order (ESR):
db.orders.createIndex({ customerId: 1, createdAt: -1, total: 1 })
// Index provides both filtering and sort order
```

### Mistake 8: Dot Notation When $elemMatch Is Needed

```javascript
// Data: { items: [{ name: "A", qty: 5 }, { name: "B", qty: 15 }] }

// BAD: dot notation matches across different array elements
db.orders.find({ "items.name": "A", "items.qty": { $gte: 10 } })
// Returns this document! (name "A" exists, qty >= 10 exists on element B)

// GOOD: $elemMatch ensures same element
db.orders.find({
  items: { $elemMatch: { name: "A", qty: { $gte: 10 } } }
})
// Does NOT return this document (element A has qty 5)
```

### Mistake 9: updateMany/deleteMany Without Specific Filter

```javascript
// DANGEROUS: empty filter
db.users.updateMany({}, { $set: { role: "user" } })
db.orders.deleteMany({})

// SAFER: explicit filter
db.users.updateMany(
  { role: { $exists: false } },
  { $set: { role: "user" } }
)
```

### Mistake 10: Storing Dates as Strings

```javascript
// BAD: string dates don't sort correctly, no date math
db.events.insertOne({ date: "2026-03-15" })
db.events.find({ date: { $gt: "2026-03-01" } })  // Lexicographic, not chronological

// GOOD: native BSON Date
db.events.insertOne({ date: new Date("2026-03-15") })
db.events.find({ date: { $gt: new Date("2026-03-01") } })
```

### Mistake 11: $unwind + $group Instead of Array Operators

```javascript
// BAD: blocking pipeline, wastes memory
db.orders.aggregate([
  { $unwind: "$items" },
  { $match: { "items.price": { $gte: 100 } } },
  { $group: { _id: "$_id", expensiveItems: { $push: "$items" } } }
])

// GOOD: non-blocking, in-document transformation
db.orders.aggregate([
  { $addFields: {
    expensiveItems: {
      $filter: {
        input: "$items",
        cond: { $gte: ["$$this.price", 100] }
      }
    }
  }}
])
```

### Mistake 12: Not Using bulkWrite for Batch Operations

```javascript
// BAD: N round-trips
for (const id of ids) {
  await collection.updateOne({ _id: id }, { $set: { processed: true } });
}

// GOOD: single round-trip
await collection.bulkWrite(
  ids.map(id => ({
    updateOne: {
      filter: { _id: id },
      update: { $set: { processed: true } }
    }
  })),
  { ordered: false }
);
```

### Mistake 13: Ignoring Write Concern for Critical Data

```javascript
// BAD: default w:1 for financial transaction
await db.transactions.insertOne({ amount: 500, type: "transfer" })

// GOOD: majority write concern
await db.transactions.insertOne(
  { amount: 500, type: "transfer" },
  { writeConcern: { w: "majority" } }
)
```

### Mistake 14: Using replaceOne When updateOne Is Intended

```javascript
// BAD: replaces entire document, losing all fields except _id and name
db.users.replaceOne({ _id: id }, { name: "Updated" })

// GOOD: updates only the name field
db.users.updateOne({ _id: id }, { $set: { name: "Updated" } })
```

### Mistake 15: Negation Operators on Unindexed Fields

```javascript
// BAD: $ne, $nin, $not, $nor cannot use indexes efficiently
db.users.find({ status: { $ne: "deleted" } })  // Near-full scan

// BETTER: query for the values you want
db.users.find({ status: { $in: ["active", "pending", "suspended"] } })
```

---

## 14. Validation Rules Summary

Quick reference for building automated query validation:

| Rule ID | Severity | Description |
|---------|----------|-------------|
| `NO_LIMIT` | warning | `find()` without `.limit()` on potentially large collections |
| `COLLSCAN` | error | Query causes full collection scan (no matching index) |
| `REGEX_NO_ANCHOR` | warning | `$regex` without `^` anchor -- full index/collection scan |
| `REGEX_CASE_INSENSITIVE` | info | `$regex` with `i` flag cannot use index prefix optimization |
| `NOT_WITH_REGEX` | error | `$not` combined with `$regex` -- always full collection scan |
| `WHERE_USAGE` | error | `$where` detected -- replace with `$expr` |
| `LARGE_IN_ARRAY` | warning | `$in` with > 200 values -- consider restructuring |
| `NEGATION_OPERATOR` | info | `$ne`, `$nin`, `$nor` -- poor index utilization |
| `EMPTY_FILTER_WRITE` | error | `updateMany({})` or `deleteMany({})` with empty filter |
| `MISSING_PROJECTION` | info | Query fetches all fields -- add projection |
| `DATE_AS_STRING` | warning | Date comparisons on string fields |
| `DOT_NOTATION_ARRAY` | warning | Multiple dot-notation conditions on array -- consider `$elemMatch` |
| `UNWIND_GROUP_ANTI` | info | `$unwind` followed by `$group` on same doc -- use array operators |
| `SORT_NO_INDEX` | warning | `$sort` stage without matching index (in-memory sort) |
| `LOOKUP_NO_INDEX` | warning | `$lookup` foreign field not indexed |
| `ESR_VIOLATION` | info | Compound index field order does not follow ESR rule |
| `REPLACE_VS_UPDATE` | warning | `replaceOne` used -- verify intent (not `updateOne` with `$set`) |
| `WRITE_LOOP` | warning | Sequential single writes detected -- suggest `bulkWrite` |
| `SKIP_LARGE_OFFSET` | info | `$skip` with large value -- suggest range-based pagination |
| `UNBOUNDED_ARRAY` | warning | Schema contains potentially unbounded arrays |
| `W0_CRITICAL_DATA` | warning | Write concern `w:0` on collections with critical data |
| `PROJECT_EARLY` | info | `$project` placed early in pipeline -- let optimizer handle it |
| `MATCH_NOT_FIRST` | warning | `$match` not at beginning of pipeline -- may miss index usage |
| `SIZE_RANGE` | info | `$size` used with comparison -- `$size` only supports exact match |
