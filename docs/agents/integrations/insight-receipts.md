# Insight receipt verification

- Keep the real Bot API acceptance boundary in [forced-receipt.mjs](../../../backend/scripts/insight-fixture/forced-receipt.mjs).
- Hold confirmation only after a genuine validated Bot API response. A JavaScript sleep or SQL mutation is not equivalent.
- Observe the exact effect token through the native authenticated fixture controller.
- Queue exhaustion, update acknowledgement, unchanged SQL, and a later `/start` response do not prove exact replay fencing.
- Preserve the original process's logs through shutdown and inspect its actual exit.
- Do not use SIGKILL, OOM, replacement signals, or changed budgets to manufacture the expected shutdown timeout.
- Confirm the late receipt before restart. Prove that replay creates no duplicate send.
- Keep controller, database, log, and signal operations bounded. A bounded polling loop cannot bound an unbounded callback.
- Report primary failures and hold-release cleanup failures separately.
- Node orchestration tests support diagnostics. Only the matching-image actual-process scenario establishes transport acceptance.
