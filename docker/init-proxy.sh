#!/bin/sh
# Ensures SOCKS5 proxy is configured in tdlight-telegram-bot-api via TDLib addProxy API.
# Idempotent: checks existing proxies first, only adds if the target is not already present.
# TDLib persists proxy config per-session; runs for bot token and user token (if present).

set -e

BOT_API_URL="${BOT_API_URL:-http://yoink-tg-bot-api:8082}"
BOT_TOKEN="${bot_token}"
SOCKS5_HOST="${socks5_host:-host.docker.internal}"
SOCKS5_PORT="${socks5_port:-1080}"
USER_TOKEN_FILE="${USER_TOKEN_FILE:-/data/tg-bot-api/user.token}"

if [ -z "$BOT_TOKEN" ]; then
    echo "ERROR: bot_token is not set"
    exit 1
fi

api() {
    wget -q -O- -T 15 \
        --header="Content-Type: application/x-www-form-urlencoded" \
        "$@" 2>&1
}

configure_proxy() {
    local session_url="$1"
    local label="$2"

    # Trigger TDLib client initialization - tdlight lazy-inits on first request.
    # getMe may return 401/timeout but that is fine; it wakes the client so proxy
    # API calls (which need an active client) work immediately after.
    echo "[${label}] Waking TDLib client..."
    api "${session_url}/getMe" > /dev/null 2>&1 || true
    sleep 1

    EXISTING=$(api "${session_url}/getProxies")
    if echo "$EXISTING" | grep -q "\"server\":\"${SOCKS5_HOST}\"" && \
       echo "$EXISTING" | grep -q "\"port\":${SOCKS5_PORT}" && \
       echo "$EXISTING" | grep -q '"is_enabled":true'; then
        echo "[${label}] SOCKS5 ${SOCKS5_HOST}:${SOCKS5_PORT} already configured - skipping"
        return 0
    fi

    echo "[${label}] Configuring SOCKS5 proxy ${SOCKS5_HOST}:${SOCKS5_PORT}..."

    RESPONSE=$(api --post-data="server=${SOCKS5_HOST}&port=${SOCKS5_PORT}&type=socks5" \
        "${session_url}/addProxy")

    if ! echo "$RESPONSE" | grep -q '"ok":true'; then
        echo "[${label}] ERROR: addProxy failed: ${RESPONSE}"
        return 1
    fi

    PROXY_ID=$(echo "$RESPONSE" | sed 's/.*"id":\([0-9]*\).*/\1/')

    RESPONSE2=$(api --post-data="proxy_id=${PROXY_ID}" \
        "${session_url}/enableProxy")

    if ! echo "$RESPONSE2" | grep -q '"ok":true'; then
        echo "[${label}] ERROR: enableProxy failed: ${RESPONSE2}"
        return 1
    fi

    echo "[${label}] Proxy id=${PROXY_ID} configured and enabled"
}

configure_proxy "${BOT_API_URL}/bot${BOT_TOKEN}" "bot"

if [ -f "${USER_TOKEN_FILE}" ]; then
    USER_TOKEN=$(cat "${USER_TOKEN_FILE}" | tr -d '[:space:]')
    if [ -n "$USER_TOKEN" ]; then
        configure_proxy "${BOT_API_URL}/user${USER_TOKEN}" "user"
    fi
else
    echo "[user] No token file at ${USER_TOKEN_FILE} - skipping user session proxy setup"
fi
