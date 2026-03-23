#!/bin/sh
set -e

WORK_DIR="${TELEGRAM_WORK_DIR:-/var/lib/telegram-bot-api}"
TEMP_DIR="${TELEGRAM_TEMP_DIR:-/tmp/telegram-bot-api}"
HTTP_PORT="${TELEGRAM_HTTP_PORT:-8081}"

ARGS="--http-port=${HTTP_PORT} --dir=${WORK_DIR} --temp-dir=${TEMP_DIR}"

[ -n "$TELEGRAM_LOCAL" ]                    && ARGS="${ARGS} --local"
[ -n "$TELEGRAM_STAT" ]                     && ARGS="${ARGS} --http-stat-port=8082"
[ -n "$TELEGRAM_STAT_HIDE_SENSIBLE_DATA" ]  && ARGS="${ARGS} --stats-hide-sensible-data"
[ -n "$TELEGRAM_FILTER" ]                   && ARGS="${ARGS} --filter=${TELEGRAM_FILTER}"
[ -n "$TELEGRAM_MAX_WEBHOOK_CONNECTIONS" ]  && ARGS="${ARGS} --max-webhook-connections=${TELEGRAM_MAX_WEBHOOK_CONNECTIONS}"
[ -n "$TELEGRAM_VERBOSITY" ]                && ARGS="${ARGS} --verbosity=${TELEGRAM_VERBOSITY}"
[ -n "$TELEGRAM_MAX_CONNECTIONS" ]          && ARGS="${ARGS} --max-connections=${TELEGRAM_MAX_CONNECTIONS}"
[ -n "$TELEGRAM_NO_FILE_LIMIT" ]            && ARGS="${ARGS} --no-file-limit"
[ -n "$TELEGRAM_ALLOW_USERS" ]              && ARGS="${ARGS} --allow-users"
[ -n "$TELEGRAM_ALLOW_USERS_REGISTRATION" ] && ARGS="${ARGS} --allow-users-registration"
[ -n "$TELEGRAM_INSECURE" ]                 && ARGS="${ARGS} --insecure"
[ -n "$TELEGRAM_RELATIVE" ]                 && ARGS="${ARGS} --relative"
[ -n "$TELEGRAM_MAX_BATCH" ]                && ARGS="${ARGS} --max-batch-operations=${TELEGRAM_MAX_BATCH}"
[ -n "$TELEGRAM_FILE_EXPIRATION_TIME" ]     && ARGS="${ARGS} --file-expiration-time=${TELEGRAM_FILE_EXPIRATION_TIME}"
[ -n "$TELEGRAM_HTTP_IDLE_TIMEOUT" ]        && ARGS="${ARGS} --http-idle-timeout=${TELEGRAM_HTTP_IDLE_TIMEOUT}"

if [ -n "$TELEGRAM_LOGS" ]; then
    ARGS="${ARGS} --log=${TELEGRAM_LOGS}"
else
    ARGS="${ARGS} --log=/proc/1/fd/1"
fi

echo "telegram-bot-api ${ARGS}"
exec telegram-bot-api ${ARGS}
