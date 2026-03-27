FROM python:3.12-alpine AS builder-core

RUN apk add --no-cache gcc musl-dev libffi-dev openssl-dev postgresql-dev

COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

WORKDIR /app

COPY pyproject.toml uv.lock ./
COPY src/ ./src/

RUN uv pip install --system --no-cache -e .

FROM builder-core AS builder-plugins

RUN apk add --no-cache gcc musl-dev libffi-dev

COPY plugins/yoink-dl/pyproject.toml /app/plugins/yoink-dl/
COPY plugins/yoink-dl/src/ /app/plugins/yoink-dl/src/

COPY plugins/yoink-stats/pyproject.toml /app/plugins/yoink-stats/
COPY plugins/yoink-stats/src/ /app/plugins/yoink-stats/src/

COPY plugins/yoink-music/pyproject.toml /app/plugins/yoink-music/
COPY plugins/yoink-music/src/ /app/plugins/yoink-music/src/

COPY plugins/yoink-insight/pyproject.toml /app/plugins/yoink-insight/
COPY plugins/yoink-insight/src/ /app/plugins/yoink-insight/src/

RUN uv pip install --system --no-cache \
    -e /app/plugins/yoink-dl \
    -e /app/plugins/yoink-stats \
    -e /app/plugins/yoink-music \
    -e /app/plugins/yoink-insight

FROM python:3.12-alpine

ARG TZ=Europe/Moscow
ENV TZ="$TZ" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

RUN apk add --no-cache \
    ffmpeg \
    mediainfo \
    yt-dlp \
    ca-certificates \
    tzdata \
    libpq \
 && cp /usr/share/zoneinfo/${TZ} /etc/localtime \
 && echo "${TZ}" > /etc/timezone \
 && apk del tzdata

COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv
COPY --from=builder-plugins /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages
COPY --from=builder-plugins /usr/local/bin/yoink* /usr/local/bin/
COPY --from=builder-plugins /usr/local/bin/alembic /usr/local/bin/alembic
COPY --from=builder-plugins /usr/local/bin/gallery-dl /usr/local/bin/gallery-dl

WORKDIR /app

COPY src/ ./src/
COPY plugins/yoink-dl/src/ /app/plugins/yoink-dl/src/
COPY plugins/yoink-stats/src/ /app/plugins/yoink-stats/src/
COPY plugins/yoink-music/src/ /app/plugins/yoink-music/src/
COPY plugins/yoink-insight/src/ /app/plugins/yoink-insight/src/
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

VOLUME ["/app/data"]

EXPOSE 8000

ENTRYPOINT ["/entrypoint.sh"]
CMD ["yoink-combined"]
