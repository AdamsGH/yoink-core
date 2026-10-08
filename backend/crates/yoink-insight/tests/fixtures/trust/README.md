# Provider TLS fixtures

`root.der` is an isolated X.509 trust anchor. `localhost.pem` is its server
certificate with only DNS:localhost in its SAN (valid 2020-01-01 to 2040-01-01).
`localhost.key` is an intentionally public, unencrypted **test-only** private
key. Never use these credentials or trust anchor in production. The root signing
key was discarded after generation.

Feature-gated provider unit tests require `openssl` on PATH and start a disposable
loopback `openssl s_server`; no external network, containers, custom resolvers,
production CA settings, or TLS-verification bypasses are used. The same tests
build the real pinned BYOK client with public addresses and independently probe
its TLS trust over localhost. Private/mixed addresses remain rejected before
client construction; no private addresses are injected into DNS pinning.
