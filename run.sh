#!/usr/bin/env bash
# ============================================================================
# SlackMongoBot — run.sh
# ============================================================================
# Usage:
#   ./run.sh                      Run from source with cargo (default)
#   ./run.sh --docker             Run with Docker Compose (bot + mongo + ollama)
#   ./run.sh --docker-clean       Run with Docker Compose (full rebuild, no cache)
#   ./run.sh --build              Clean rebuild + run tests + build binary & Docker
#   ./run.sh --clean              Remove all build artifacts
#   ./run.sh --restart            Rebuild + restart (Docker or cargo)
#   ./run.sh --stop               Stop whatever was started
#   ./run.sh --help               Show this help
# ============================================================================

set -euo pipefail

# ── Configuration (env vars with defaults) ──────────────────────────────────
export MONGO_URI="${MONGO_URI:-mongodb://localhost:27017}"
export MONGO_DB_NAME="${MONGO_DB_NAME:-myapp}"
export OLLAMA_URL="${OLLAMA_URL:-http://localhost:11434}"
export OLLAMA_MODEL="${OLLAMA_MODEL:-qwen3-coder:30b}"
export LLM_PROVIDER="${LLM_PROVIDER:-ollama}"
export HOST="${HOST:-0.0.0.0}"
export PORT="${PORT:-3000}"
export RUST_LOG="${RUST_LOG:-info}"
export MAX_RESULT_ROWS="${MAX_RESULT_ROWS:-10000}"
export ALLOWED_OPERATIONS="${ALLOWED_OPERATIONS:-find,count,distinct,aggregate}"
export SLACK_BOT_TOKEN="${SLACK_BOT_TOKEN:-}"
export SLACK_SIGNING_SECRET="${SLACK_SIGNING_SECRET:-}"
export DASHBOARD_TOKEN="${DASHBOARD_TOKEN:-}"
export UNSAFE_ALLOW_ALL_OPERATIONS="${UNSAFE_ALLOW_ALL_OPERATIONS:-false}"
export QUERY_ONLY="${QUERY_ONLY:-false}"

APP_NAME="slack-mongo-bot"
DOCKER_CONTAINER="slack-mongo-bot"

# ── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Functions ───────────────────────────────────────────────────────────────

print_help() {
    echo ""
    echo -e "${CYAN}SlackMongoBot${NC} — NL-to-MongoDB query bot with Web UI + Slack"
    echo ""
    echo "Usage:"
    echo "  ./run.sh                Run from source with cargo (default)"
    echo "  ./run.sh --docker       Run with Docker Compose (bot + mongo + ollama)"
    echo "  ./run.sh --docker-clean Run with Docker Compose (full rebuild, no cache)"
    echo "  ./run.sh --build        Clean rebuild + run tests + build binary & Docker"
    echo "  ./run.sh --clean        Remove all build artifacts"
    echo "  ./run.sh --restart      Rebuild + restart (Docker or cargo)"
    echo "  ./run.sh --stop         Stop whatever was started"
    echo "  ./run.sh --help         Show this help"
    echo ""
    echo "Environment Variables:"
    echo "  MONGO_URI               MongoDB URI (default: mongodb://localhost:27017)"
    echo "  MONGO_DB_NAME           Database name (default: myapp)"
    echo "  OLLAMA_URL              Ollama URL (default: http://localhost:11434)"
    echo "  OLLAMA_MODEL            LLM model (default: qwen3-coder:30b)"
    echo "  LLM_PROVIDER            LLM provider: ollama, anthropic, openai (default: ollama)"
    echo "  HOST                    Server host (default: 0.0.0.0)"
    echo "  PORT                    Server port (default: 3000)"
    echo "  SLACK_BOT_TOKEN         Slack bot token (optional)"
    echo "  SLACK_SIGNING_SECRET    Slack signing secret (optional)"
    echo "  DASHBOARD_TOKEN         Dashboard auth token (optional)"
    echo "  MAX_RESULT_ROWS         Max export rows (default: 10000)"
    echo "  UNSAFE_ALLOW_ALL_OPERATIONS  Allow \$out/\$merge stages (default: false)"
    echo "  QUERY_ONLY              Parse queries without executing (default: false)"
    echo "  RUST_LOG                Log level (default: info)"
    echo ""
}

print_config() {
    echo -e "${CYAN}Configuration:${NC}"
    echo "  MONGO_URI:       ${MONGO_URI}"
    echo "  MONGO_DB_NAME:   ${MONGO_DB_NAME}"
    echo "  OLLAMA_URL:      ${OLLAMA_URL}"
    echo "  OLLAMA_MODEL:    ${OLLAMA_MODEL}"
    echo "  LLM_PROVIDER:    ${LLM_PROVIDER}"
    echo "  HOST:            ${HOST}"
    echo "  PORT:            ${PORT}"
    echo "  SLACK:           ${SLACK_BOT_TOKEN:+configured}${SLACK_BOT_TOKEN:-not configured}"
    echo "  DASHBOARD_AUTH:  ${DASHBOARD_TOKEN:+enabled}${DASHBOARD_TOKEN:-disabled}"
    echo "  UNSAFE_OPS:      ${UNSAFE_ALLOW_ALL_OPERATIONS}"
    echo "  QUERY_ONLY:      ${QUERY_ONLY}"
    echo "  RUST_LOG:        ${RUST_LOG}"
    echo ""
}

check_mongo_db() {
    if [ -z "$MONGO_DB_NAME" ]; then
        echo -e "${RED}Error: MONGO_DB_NAME is required${NC}"
        echo ""
        echo "Set it via environment variable:"
        echo "  export MONGO_DB_NAME=myapp"
        echo ""
        exit 1
    fi
}

run_cargo() {
    check_mongo_db
    print_config

    echo -e "${GREEN}Starting from source with cargo...${NC}"
    echo -e "Dashboard will be at: ${CYAN}http://${HOST}:${PORT}${NC}"
    echo ""

    ARGS="--host ${HOST} --port ${PORT}"
    ARGS="${ARGS} --mongo-uri ${MONGO_URI}"
    ARGS="${ARGS} --mongo-db-name ${MONGO_DB_NAME}"
    ARGS="${ARGS} --ollama-url ${OLLAMA_URL}"
    ARGS="${ARGS} --ollama-model ${OLLAMA_MODEL}"
    ARGS="${ARGS} --llm-provider ${LLM_PROVIDER}"
    ARGS="${ARGS} --max-result-rows ${MAX_RESULT_ROWS}"
    ARGS="${ARGS} --allowed-operations ${ALLOWED_OPERATIONS}"

    if [ -n "${SLACK_BOT_TOKEN}" ]; then
        ARGS="${ARGS} --slack-bot-token ${SLACK_BOT_TOKEN}"
    fi
    if [ -n "${SLACK_SIGNING_SECRET}" ]; then
        ARGS="${ARGS} --slack-signing-secret ${SLACK_SIGNING_SECRET}"
    fi
    if [ -n "${DASHBOARD_TOKEN}" ]; then
        ARGS="${ARGS} --dashboard-token ${DASHBOARD_TOKEN}"
    fi
    if [ "${UNSAFE_ALLOW_ALL_OPERATIONS}" = "true" ]; then
        ARGS="${ARGS} --unsafe-allow-all-operations"
    fi
    if [ "${QUERY_ONLY}" = "true" ]; then
        ARGS="${ARGS} --query-only"
    fi

    # shellcheck disable=SC2086
    cargo run -- ${ARGS}
}

run_docker() {
    print_config

    echo -e "${GREEN}Starting with Docker Compose (bot + mongo)...${NC}"
    echo -e "${YELLOW}Note: Ollama must be running on the host machine${NC}"
    echo -e "Dashboard will be at: ${CYAN}http://localhost:${PORT}${NC}"
    echo ""

    docker compose up -d --build
    echo ""
    echo -e "${GREEN}Docker containers started.${NC}"
    echo ""

    echo -e "${GREEN}Services:${NC}"
    echo -e "  Bot:     ${CYAN}http://localhost:${PORT}${NC}"
    echo -e "  MongoDB: ${CYAN}mongodb://localhost:27017${NC} (Docker)"
    echo -e "  Ollama:  ${CYAN}http://localhost:11434${NC} (host machine)"
    echo ""
    echo -e "View logs: ${YELLOW}docker compose logs -f bot${NC}"

    verify_build
}

run_stop() {
    echo -e "${YELLOW}Stopping SlackMongoBot...${NC}"

    # Check Docker Compose
    if docker compose ps -q 2>/dev/null | grep -q .; then
        echo "Stopping Docker Compose services..."
        docker compose down
        echo -e "${GREEN}Docker services stopped.${NC}"
        return
    fi

    # Check cargo process
    if pgrep -f "target/debug/${APP_NAME}" > /dev/null 2>&1; then
        echo "Stopping cargo process..."
        pkill -f "target/debug/${APP_NAME}" || true
        echo -e "${GREEN}Process stopped.${NC}"
        return
    fi
    if pgrep -f "target/release/${APP_NAME}" > /dev/null 2>&1; then
        echo "Stopping release binary..."
        pkill -f "target/release/${APP_NAME}" || true
        echo -e "${GREEN}Process stopped.${NC}"
        return
    fi

    # Check port
    if lsof -ti ":${PORT}" > /dev/null 2>&1; then
        echo "Killing process on port ${PORT}..."
        lsof -ti ":${PORT}" | xargs kill -9 2>/dev/null || true
        echo -e "${GREEN}Process killed.${NC}"
        return
    fi

    echo -e "${YELLOW}No running instance found.${NC}"
}

verify_build() {
    local url="http://localhost:${PORT}/api/health"
    local max_tries=10
    local try=1

    echo ""
    echo -e "${YELLOW}Verifying server health...${NC}"
    while [ $try -le $max_tries ]; do
        local response
        response=$(curl -s "$url" 2>/dev/null) || true
        if echo "$response" | grep -q '"status"'; then
            local status mongo
            status=$(echo "$response" | grep -o '"status":"[^"]*"' | cut -d'"' -f4)
            mongo=$(echo "$response" | grep -o '"mongo":[a-z]*' | cut -d: -f2)
            echo -e "${GREEN}Server is running:${NC}"
            echo -e "  Status:   ${CYAN}${status}${NC}"
            echo -e "  MongoDB:  ${CYAN}${mongo}${NC}"
            return 0
        fi
        sleep 2
        try=$((try + 1))
    done
    echo -e "${YELLOW}Could not verify server (may still be starting)${NC}"
    echo -e "  Check manually: ${CYAN}curl ${url}${NC}"
    return 1
}

run_restart() {
    echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
    echo -e "${CYAN}  SlackMongoBot — Restart${NC}"
    echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
    echo ""

    if docker compose ps -q 2>/dev/null | grep -q .; then
        echo -e "${YELLOW}Docker Compose detected — rebuilding...${NC}"
        docker compose up -d --build
        echo -e "${GREEN}Containers rebuilt and restarted.${NC}"
        verify_build
    elif pgrep -f "target/(debug|release)/${APP_NAME}" > /dev/null 2>&1; then
        echo -e "${YELLOW}Cargo process detected — stopping...${NC}"
        pkill -f "target/(debug|release)/${APP_NAME}" || true
        sleep 1
        echo -e "${YELLOW}Rebuilding and starting...${NC}"
        run_cargo
    else
        echo -e "${YELLOW}No running instance found. Starting fresh...${NC}"
        run_cargo
    fi
}

# ── Main ────────────────────────────────────────────────────────────────────

case "${1:-}" in
    --docker)
        run_docker
        ;;
    --docker-clean)
        print_config
        echo -e "${GREEN}Starting with Docker Compose (clean build, no cache)...${NC}"
        echo -e "${YELLOW}Note: Ollama must be running on the host machine${NC}"
        echo ""
        docker compose build --no-cache
        docker compose up -d
        echo ""
        echo -e "${GREEN}Docker containers started (clean build).${NC}"
        echo ""
        echo -e "${GREEN}Services:${NC}"
        echo -e "  Bot:     ${CYAN}http://localhost:${PORT}${NC}"
        echo -e "  MongoDB: ${CYAN}mongodb://localhost:27017${NC} (Docker)"
        echo -e "  Ollama:  ${CYAN}http://localhost:11434${NC} (host machine)"
        echo ""
        echo -e "View logs: ${YELLOW}docker compose logs -f bot${NC}"
        verify_build
        ;;
    --build)
        echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
        echo -e "${CYAN}  SlackMongoBot — Full Build${NC}"
        echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
        echo ""

        echo -e "${YELLOW}Cleaning build artifacts...${NC}"
        cargo clean
        echo -e "${GREEN}Clean complete.${NC}"
        echo ""

        echo -e "${YELLOW}Running tests...${NC}"
        if cargo test; then
            echo -e "${GREEN}All tests passed.${NC}"
        else
            echo -e "${RED}Tests failed. Aborting build.${NC}"
            exit 1
        fi
        echo ""

        echo -e "${YELLOW}Building release binary...${NC}"
        cargo build --release
        echo -e "${GREEN}Binary built: target/release/${APP_NAME}${NC}"
        echo ""

        echo -e "${YELLOW}Building Docker image...${NC}"
        docker compose build
        echo -e "${GREEN}Docker image built.${NC}"
        echo ""

        echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
        echo -e "${GREEN}  Build complete!${NC}"
        echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
        echo ""
        echo -e "  Binary:  ${CYAN}target/release/${APP_NAME}${NC}"
        echo -e "  Docker:  ${CYAN}./run.sh --docker${NC}"
        echo -e "  Cargo:   ${CYAN}./run.sh${NC}"
        ;;
    --clean)
        echo -e "${YELLOW}Cleaning all build artifacts...${NC}"
        cargo clean
        docker compose down --rmi local --volumes 2>/dev/null || true
        echo -e "${GREEN}All clean.${NC}"
        ;;
    --restart)
        run_restart
        ;;
    --stop)
        run_stop
        ;;
    --help|-h)
        print_help
        ;;
    "")
        run_cargo
        ;;
    *)
        echo -e "${RED}Unknown option: $1${NC}"
        print_help
        exit 1
        ;;
esac
