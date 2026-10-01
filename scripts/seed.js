// Seed the `movies` database with synthetic data matching schemas/*.yaml.
//
// Runs automatically on `docker compose up` via the `seed` service
// (scripts/seed-if-empty.sh). To run by hand against the target database:
//   mongosh mongodb://localhost:27017/movies --quiet --file scripts/seed.js
//
// Requires the `movies` collection to be loaded first (sample_mflix/movies.json):
// actors and reviews reference real movie titles and cast names from it.
// Drops and recreates ONLY: users, orders, reviews, actors, error_logs.
// Deterministic: a fixed PRNG seed produces the same data on every run.

const COUNTS = { users: 200, orders: 1000, reviews: 1500, actors: 150, error_logs: 500 };
const NOW = new Date("2026-09-30T00:00:00Z");
const DAY_MS = 24 * 60 * 60 * 1000;

let state = 42;
function rand() {
  // mulberry32
  state = (state + 0x6d2b79f5) | 0;
  let t = Math.imul(state ^ (state >>> 15), 1 | state);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const randInt = (min, max) => min + Math.floor(rand() * (max - min + 1));
const pick = (arr) => arr[Math.floor(rand() * arr.length)];
const weighted = (pairs) => {
  const r = rand() * pairs.reduce((s, [, w]) => s + w, 0);
  let acc = 0;
  for (const [v, w] of pairs) {
    acc += w;
    if (r < acc) return v;
  }
  return pairs[pairs.length - 1][0];
};
const daysAgo = (min, max) => new Date(NOW.getTime() - randInt(min, max) * DAY_MS - randInt(0, DAY_MS - 1));
const round2 = (n) => Math.round(n * 100) / 100;

const FIRST = ["Alice", "Bob", "Carlos", "Diana", "Ethan", "Fatima", "George", "Hana", "Ivan", "Julia",
  "Kenji", "Laura", "Mateo", "Nina", "Omar", "Priya", "Quinn", "Rosa", "Sam", "Tara", "Umar", "Vera",
  "Wei", "Ximena", "Yusuf", "Zoe"];
const LAST = ["Chen", "Martinez", "Smith", "Okafor", "Kim", "Novak", "Patel", "Rossi", "Silva", "Nguyen",
  "Johnson", "Dubois", "Tanaka", "Murphy", "Schmidt", "Garcia", "Haddad", "Larsen"];

const movies = db.movies;
if (movies.estimatedDocumentCount() === 0) {
  throw new Error("movies collection is empty - import sample_mflix/movies.json first");
}

for (const name of Object.keys(COUNTS)) db[name].drop();

// ── users ────────────────────────────────────────────────────────────────────
const users = Array.from({ length: COUNTS.users }, (_, i) => {
  const first = pick(FIRST);
  const last = pick(LAST);
  const status = weighted([["active", 80], ["suspended", 12], ["deleted", 8]]);
  const created = daysAgo(30, 900);
  const lastLogin = new Date(Math.min(NOW.getTime(), created.getTime() + randInt(0, 900) * DAY_MS));
  return {
    _id: new ObjectId(),
    email: `${first}.${last}.${i}@example.com`.toLowerCase(),
    name: `${first} ${last}`,
    status,
    role: weighted([["user", 85], ["moderator", 10], ["admin", 5]]),
    created_at: created,
    last_login: lastLogin,
    login_count: status === "deleted" ? randInt(0, 5) : randInt(1, 400),
  };
});
db.users.insertMany(users);

// ── orders ───────────────────────────────────────────────────────────────────
const orders = Array.from({ length: COUNTS.orders }, () => {
  const status = weighted([["delivered", 45], ["shipped", 20], ["processing", 12], ["pending", 13], ["cancelled", 10]]);
  const created = daysAgo(0, 365);
  const shipped = status === "shipped" || status === "delivered"
    ? new Date(created.getTime() + randInt(1, 5) * DAY_MS)
    : null;
  const itemsCount = randInt(1, 8);
  return {
    _id: new ObjectId(),
    user_id: pick(users)._id,
    status,
    total: round2(itemsCount * (5 + rand() * 120)),
    items_count: itemsCount,
    created_at: created,
    shipped_at: shipped,
    payment_method: weighted([["credit_card", 60], ["paypal", 30], ["bank_transfer", 10]]),
  };
});
db.orders.insertMany(orders);

// ── reviews (real mflix titles, skewed toward well-rated films) ─────────────
const reviewable = movies
  .find({ "imdb.rating": { $type: "number" }, "imdb.votes": { $gte: 50000 } }, { title: 1, "imdb.rating": 1 })
  .toArray();
const PRAISE = ["A masterpiece of pacing and tone.", "Great performances all around.",
  "Visually stunning and emotionally resonant.", "Holds up on a rewatch.", "Tight script, no wasted scenes."];
const MIXED = ["Strong start but the third act drags.", "Good ideas, uneven execution.",
  "Worth watching once.", "The lead carries a thin story."];
// Named critics give NL queries like "reviews by Roger Ebert" real matches
// (tests/integration_tests.rs relies on this).
const CRITICS = ["Roger Ebert", "Pauline Kael", "Gene Siskel", "Mark Kermode", "Peter Travers"];
const PAN = ["Overlong and predictable.", "Could not connect with any of the characters.",
  "Style over substance.", "The trailer was better than the film."];
const reviews = Array.from({ length: COUNTS.reviews }, () => {
  const m = pick(reviewable);
  // Bias the 1-5 score by the film's IMDb rating, with noise.
  const score = Math.max(1, Math.min(5, Math.round(m.imdb.rating / 2 + (rand() - 0.5) * 2)));
  const text = score >= 4 ? pick(PRAISE) : score === 3 ? pick(MIXED) : pick(PAN);
  return {
    _id: new ObjectId(),
    movie: m.title,
    reviewer: rand() < 0.15 ? pick(CRITICS) : `${pick(FIRST)} ${pick(LAST)}`,
    score,
    text,
    date: daysAgo(0, 1500),
    helpful_votes: Math.floor(rand() * rand() * 500),
  };
});
db.reviews.insertMany(reviews);

// ── actors (most frequent cast members, with their real top-rated films) ────
const topCast = movies.aggregate([
  { $match: { "imdb.rating": { $type: "number" } } },
  { $unwind: "$cast" },
  { $sort: { "imdb.rating": -1 } },
  { $group: { _id: "$cast", films: { $sum: 1 }, titles: { $push: "$title" }, firstYear: { $min: "$year" } } },
  { $sort: { films: -1 } },
  { $limit: COUNTS.actors },
]).toArray();
const NATIONALITIES = ["American", "British", "Korean", "Canadian", "Australian", "French"];
const actors = topCast.map((a) => {
  const firstYear = typeof a.firstYear === "number" ? a.firstYear : 1980;
  return {
    _id: new ObjectId(),
    name: a._id,
    birth_year: firstYear - randInt(18, 30),
    nationality: weighted(NATIONALITIES.map((n, i) => [n, i === 0 ? 50 : 10])),
    gender: pick(["Male", "Female"]),
    active: firstYear > 1975 ? rand() < 0.8 : rand() < 0.1,
    oscar_wins: weighted([[0, 85], [1, 11], [2, 3], [3, 1]]),
    notable_movies: [...new Set(a.titles)].slice(0, 5),
  };
});
db.actors.insertMany(actors);

// ── error_logs ───────────────────────────────────────────────────────────────
const SERVICES = ["order-service", "payment-service", "auth-service", "search-service", "notification-service"];
const ERRORS = [
  ["Payment gateway timeout after 30s", "PaymentClient.charge"],
  ["Database connection pool exhausted", "PoolManager.acquire"],
  ["JWT signature verification failed", "TokenVerifier.verify"],
  ["Upstream returned 503 Service Unavailable", "HttpClient.send"],
  ["Null reference while building response", "ResponseBuilder.build"],
  ["Rate limit exceeded for tenant", "RateLimiter.check"],
  ["Failed to deserialize message payload", "MessageDecoder.decode"],
];
const errorLogs = Array.from({ length: COUNTS.error_logs }, () => {
  const level = weighted([["warning", 55], ["error", 38], ["critical", 7]]);
  const [message, frame] = pick(ERRORS);
  const service = pick(SERVICES);
  const timestamp = daysAgo(0, 90);
  return {
    _id: new ObjectId(),
    level,
    message,
    service,
    stack_trace: level === "warning"
      ? ""
      : `Error: ${message}\n    at ${frame} (${service}/src/handler.js:${randInt(10, 400)})\n    at process.processTicksAndRejections (node:internal/process/task_queues:95:5)`,
    timestamp,
    // Older issues are more likely to have been resolved.
    resolved: rand() < Math.min(0.95, (NOW - timestamp) / (90 * DAY_MS) + 0.1),
  };
});
db.error_logs.insertMany(errorLogs);

// ── indexes for the common filter / sort fields ─────────────────────────────
db.users.createIndex({ email: 1 }, { unique: true });
db.users.createIndex({ status: 1 });
db.orders.createIndex({ user_id: 1 });
db.orders.createIndex({ status: 1, created_at: -1 });
db.reviews.createIndex({ movie: 1 });
db.reviews.createIndex({ score: -1 });
db.actors.createIndex({ name: 1 });
db.error_logs.createIndex({ level: 1, timestamp: -1 });
db.error_logs.createIndex({ service: 1 });

for (const name of Object.keys(COUNTS)) print(`${name}: ${db[name].countDocuments()}`);
