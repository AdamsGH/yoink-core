# Installation

The supplied stack targets Linux amd64. It runs the native backend, frontend, nginx edge, local Bot API, proxy facade, and combined YouTube service.

## Prerequisites

- Docker with Compose 2.24.4 or newer and Buildx.
- `just`, Node.js 22 or newer, Git, and host ACL utilities.
- The Rust toolchain from [rust-toolchain.toml](../../backend/rust-toolchain.toml) for host-side checks.
- A kernel with Landlock ABI 6 or newer for confined workers.
- A Telegram bot token, owner user ID, and Telegram API credentials for the local Bot API.
- An HTTPS hostname and reverse proxy for the Mini App.
- A reachable SOCKS5 proxy, or reviewed replacement proxy configuration.

The default proxy facade connects to the host on port 1080. Copying the example configuration does not provision that proxy.

## Configure the instance

1. Clone Yoink with its submodules.
2. Copy `.env.example` to `.env`.
3. Set `bot_token`, `owner_id`, `api_id`, `api_hash`, and `yoink_domain`.
4. Generate `api_secret_key` with `openssl rand -hex 32`.
5. Configure native product credentials with their `YOINK_RUST_*` names.
6. Review proxy addresses and the [configuration guide](configuration.md).
7. Run `just data-dirs`.

`just data-dirs` needs noninteractive sudo for private directory ownership and Bot API ACLs. Runtime state belongs to UID/GID 10001. The Bot API directory belongs to UID/GID 101.

The example file retains legacy settings. The Rust binary does not interpret those names. Compose explicitly maps shared credentials and enables the native products.

## Provision the Bot API dependency

The stack expects a local `yoink/tg-bot-api:latest` image. Release images do not include this dependency.

On a fresh host, `just build tg` builds the retained TDLight compatibility server. This is a separate source build. Neither `just build all` nor `just pull-images` performs it.

A different Bot API image needs compatible Compose configuration. Do not assume an arbitrary upstream image implements the optional user-session methods.

## Build from source

```sh
just build all
just build backup
docker compose run --rm --no-deps yoink migrate
just up
just ps
```

The first runtime build initializes private release state and updates the runtime image pin. It does not start the service. An existing container without release state requires explicit adoption, described in [local releases](releases.md).

## Use published images

Check out the published `vMAJOR.MINOR.PATCH` tag before downloading its matching images:

```sh
just pull-images
just build yoink
docker compose run --rm --no-deps yoink migrate
just up
```

`just build yoink` validates and registers the downloaded backend without recompiling it. `just pull-images` also downloads the operations image, so a separate backup-image build is unnecessary.

Images use `ghcr.io/adamsgh/yoink-core/NAME:MAJOR.MINOR.PATCH`. The [publication guide](publication.md) lists the image names. Registry authentication is necessary if a package remains private.

Both startup sequences run an explicit migration before starting the database owner. Do not run this command against an already running instance.

## Check startup

```sh
curl --fail http://localhost:8003/health/ready
curl --fail http://localhost:8003/api/v1/system
```

Check the public HTTPS route and authenticate through Telegram. Readiness alone does not prove provider access or successful delivery.

The example domain does not create DNS records or certificates. Configure the external edge before exposing the Mini App.
