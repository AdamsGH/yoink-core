#!/usr/bin/env bash
# tg-bot-api user-mode session management.
#
# Usage: scripts/tg-login.sh <action> [phone]
#   login +<phone>  start interactive login (code + optional 2FA + registration)
#   status          show current token / authorisation state
#   logout          revoke server-side session and remove the local token file
#
# Why the script is structured around watch-token + a backgrounded curl:
# tg-bot-api emits the new bearer token into user_db.binlog BEFORE its
# /userLogin handler returns. We snapshot the binlog, fire /userLogin in
# the background, then poll the binlog diff for the new token. As soon as
# we see it, we attach the SOCKS5 proxy via /user<token>/proxyadd. This
# way the very first MTProto connection that tg-bot-api opens (still
# inside /userLogin) already goes through the proxy. If we waited for
# /userLogin to finish, MTProto would resolve over a clearnet IP first
# and the session would be flagged.
#
# Token is persisted at data/tg-bot-api/user.token (0600); the file is
# the source of truth for `status` and `logout`.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

TG_URL="${TG_URL:-http://127.0.0.1:8082}"
TG_NOPROXY="${TG_NOPROXY:-127.0.0.1,localhost}"
TOKEN_FILE="${TOKEN_FILE:-data/tg-bot-api/user.token}"
PY="docker/tg-login.py"

export no_proxy="$TG_NOPROXY"
export NO_PROXY="$TG_NOPROXY"

action="${1:-status}"
phone="${2:-}"

case "$action" in
  login)
    if [ -z "$phone" ]; then
        echo "Usage: just tg login +79001234567" >&2
        exit 1
    fi
    mkdir -p data/tg-bot-api
    chmod 700 data/tg-bot-api
    PHONE_DIGITS="${phone//+/}"
    DB_PATH="data/tg-bot-api/user_db.binlog"
    KNOWN_FILE=$(mktemp)
    RESP_FILE=$(mktemp)
    trap 'rm -f "$RESP_FILE" "$KNOWN_FILE"' EXIT

    sudo python3 "$PY" watch-token "$DB_PATH" "$PHONE_DIGITS" snap 2>/dev/null > "$KNOWN_FILE" || true
    echo "[tg] Requesting code for $phone ..."
    curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/userLogin" \
        -d "phone_number=$phone" > "$RESP_FILE" 2>/dev/null &
    LOGIN_PID=$!

    echo "[tg] Waiting for session token in user_db.binlog..."
    new_token=$(sudo python3 "$PY" watch-token "$DB_PATH" "$PHONE_DIGITS" "$KNOWN_FILE" 2>/dev/null)
    if [ -n "$new_token" ]; then
        echo "[tg] Got token, configuring SOCKS5 proxy..."
        python3 "$PY" proxy-add "$TG_URL" "$new_token" "host.docker.internal" "1080"
    else
        echo "[tg] WARNING: could not detect token early, proxy may not be set"
    fi
    wait "$LOGIN_PID" || true

    resp=$(cat "$RESP_FILE")
    state=$(echo "$resp" | python3 "$PY" state)
    token=$(echo "$resp" | python3 "$PY" token)

    if [ "$state" = "ready" ]; then
        echo "[tg] Already authorized."
        printf '%s' "$token" > "$TOKEN_FILE"
        chmod 600 "$TOKEN_FILE"
        echo "[tg] Token saved to $TOKEN_FILE"
        exit 0
    fi
    if [ "$state" != "wait_code" ]; then
        echo "[tg] Unexpected state: $state"
        echo "$resp"
        exit 1
    fi

    read -rp "[tg] Enter the code from Telegram: " code
    resp=$(curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/user${token}/authcode" -d "code=${code}" || true)
    state=$(echo "$resp" | python3 "$PY" state)

    if [ "$state" = "wait_password" ]; then
        read -rsp "[tg] Enter your 2FA password: " pwd; echo
        resp=$(curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/user${token}/authpassword" -d "password=${pwd}" || true)
        state=$(echo "$resp" | python3 "$PY" state)
    fi
    if [ "$state" = "wait_registration" ]; then
        read -rp "[tg] First name: " fname
        read -rp "[tg] Last name:  " lname
        resp=$(curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/user${token}/registeruser" \
            -d "first_name=${fname}" -d "last_name=${lname}" || true)
        state=$(echo "$resp" | python3 "$PY" state)
    fi

    if [ "$state" = "ready" ]; then
        printf '%s' "$token" > "$TOKEN_FILE"
        chmod 600 "$TOKEN_FILE"
        echo "[tg] Authorized. Token saved to $TOKEN_FILE"
    else
        echo "[tg] Authorization failed. State: $state"
        echo "$resp"
        exit 1
    fi
    ;;

  status)
    if [ ! -f "$TOKEN_FILE" ]; then
        echo "[tg] No token file. Run: just tg login +<phone>"
        exit 1
    fi
    token=$(cat "$TOKEN_FILE")
    echo "[tg] Token: ${token:0:20}...(redacted)"
    resp=$(curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/user${token}/getMe" || echo '{"ok":false}')
    python3 "$PY" status <<< "$resp"
    ;;

  logout)
    if [ ! -f "$TOKEN_FILE" ]; then
        echo "[tg] No token file."
        exit 0
    fi
    token=$(cat "$TOKEN_FILE")
    curl -sf --noproxy "$TG_NOPROXY" "${TG_URL}/user${token}/logout" > /dev/null && echo "[tg] Logged out."
    rm -f "$TOKEN_FILE"
    echo "[tg] Token file removed."
    ;;

  *)
    echo "Usage: $0 [login +<phone>|status|logout]" >&2
    exit 1
    ;;
esac
