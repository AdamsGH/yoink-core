# Inbox Telegram delivery

The shared Telegram adapter supplies trusted actors, destinations, entity URLs, and current identity policy to Inbox.

Commands, HTTP ingestion, classification, and notifications use the same native database. Telegram delivery does not start a separate Inbox process.

## Destination authority

Private commands require the caller's authorized private context. Group/topic operations require the shared group policy and current access.

A request body cannot select another actor or manufacture a trusted Telegram target.

## Text transport

Text delivery uses a bounded request deadline and bounded receipt parsing. The response must prove the expected sender, destination, and text effect.

The client disables redirects and automatic retry middleware. A timeout or lost response can mean Telegram accepted the message.

Inbox rendering uses conservative UTF-16 limits. Telegram entity offsets must remain valid after escaping and chunking.

## Durable recovery

The text journal records an effect before send. Valid receipts confirm it. Unknown outcomes cannot automatically resend.

Classification notifications retain an immutable resolved payload. Confirmed save receipts can bind before or after classification completes.

Recovery uses current authority and durable ledger proof. The absence of a receipt does not prove rejection.

## Lifecycle

The runtime owns notification dispatch and recovery tasks. Shutdown closes admission and drains admitted bounded attempts before database close.

Forced adapter abort remains a failure boundary. Handler ownership must survive long enough to record a valid late receipt.

See [Inbox contracts](contracts.md), [runtime ownership](runtime.md), and [persistence](../../architecture/persistence.md).
