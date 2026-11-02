# Insight Telegram changes

- Keep dispatch, menus, trusted destination policy, localization, and actual sends in the shared Telegram adapter.
- Preserve handler ownership through cancellation and late receipt confirmation.
- Model output is untrusted text. Preserve the native renderer and entity handling instead of applying arbitrary HTML parse mode.
- A successful later command is not an ordering barrier for a concurrent Insight replay.
- Use exact durable effect identities to prove replay fencing.
- Read [receipt verification](insight-receipts.md) before changing forced-shutdown acceptance.
