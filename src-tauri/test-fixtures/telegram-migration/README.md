# Telegram migration parity fixtures

These content-free fixtures were captured from the pre-extraction codec at
commit `c2300593fcc0b94c710d6a535173799dd36da4f0` on 2026-09-06. The capture ran
inside the pinned Linux arm64 Docker toolchain and called the then-current
`TelegramCompatibilityProvider::bind_plan`,
`TelegramCompatibilityProvider::normalize_job`, and `FoundationStore` v3
writer directly.

The nine JSON files cover every native `PlanOperation`. UUIDs and timestamps
are fixed. `selected.json` includes the adjacent identifiers
`9007199254740992` and `9007199254740993`, negative chat IDs, and mixed
everyone/self-only/none reaches. The compound variants retain their ordered
steps. `leave-chat.json` belongs to the second synthetic account/source so the
encrypted store also freezes foreign-scope handling.

`store-v3.b64` is the exact authenticated `RTRCT03` byte stream emitted by the
base `FoundationStore`, encoded only to keep the fixture reviewable through
text tooling. Its key is the non-secret test value `[0x54; 32]`; its AAD uses
provider `telegram` and profile `telegram-migration-base`. The store contains
all nine envelopes plus fixed projected jobs, including authorized and
unauthorized starts and an absolute rate-limit retry deadline. Exact hashes,
fingerprints, IDs, scopes, and recovery expectations are in `manifest.json`.

No fixture contains real Telegram data, credentials, profile material, message
bodies, exports, or production key bytes.
