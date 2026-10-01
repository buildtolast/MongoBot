#!/bin/sh
# One-shot seeder run by the `seed` service in docker-compose.yml on every
# `docker compose up`. Only writes when data is missing, so restarts never
# wipe existing data. Set FORCE_SEED=true to regenerate the seeded collections,
# or SEED=false to skip seeding entirely.
set -eu

if [ "${SEED:-true}" = "false" ]; then
    echo "seed: disabled (SEED=false), skipping"
    exit 0
fi

URI="${SEED_MONGO_URI:-mongodb://mongo:27017}"
DB="${MONGO_DB_NAME:-movies}"
MOVIES_JSON=/seed/movies.json

until mongosh "$URI" --quiet --eval 'db.adminCommand("ping").ok' >/dev/null 2>&1; do
    sleep 1
done

count() {
    mongosh "$URI/$DB" --quiet --eval "db.getCollection('$1').estimatedDocumentCount()"
}

if [ "$(count movies)" = "0" ]; then
    if [ ! -f "$MOVIES_JSON" ]; then
        echo "seed: $MOVIES_JSON not found; set MFLIX_DIR to the sample_mflix directory" >&2
        exit 1
    fi
    echo "seed: importing movies from sample_mflix"
    mongoimport --uri "$URI" --db "$DB" --collection movies --file "$MOVIES_JSON" --quiet
fi

if [ "${FORCE_SEED:-false}" = "true" ] || [ "$(count users)" = "0" ]; then
    echo "seed: generating users, orders, reviews, actors, error_logs"
    mongosh "$URI/$DB" --quiet --file /scripts/seed.js
else
    echo "seed: data present, skipping (FORCE_SEED=true to regenerate)"
fi
