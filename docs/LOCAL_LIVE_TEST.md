# Local setup and first live deletion test

This guide verifies Retract without exposing valuable history. Run the automated synthetic checks first, then use Telegram’s test environment where practical. Telegram recommends validating authorization in test DCs before production.

## 1. Verify the local toolchain

From Terminal:

```sh
cd retract
node --version
rustc --version
xcode-select -p
```

Install the Xcode Command Line Tools if the last command fails:

```sh
xcode-select --install
```

The project already includes the Apple-silicon TDLib library. CMake, gperf, and OpenSSL are needed only if that artifact is missing or you are rebuilding for a different architecture; the automated script reports the exact missing prerequisite.

## 2. Run the synthetic automated checks

```sh
npm ci --ignore-scripts
npm test
npm run verify:production-bundle
npm run container:check
```

These checks exercise keyword search, filters, albums, hidden selections, admin badges, review dialogs, deletion planning, the **No reply sent** and **Empty** shortlists, and privacy findings using synthetic fixtures. They also verify that the production bundle cannot import those fixtures. Nothing in this step reaches a Telegram account.

The fixture UI exists only in the automated tests and the dedicated `npm run screenshot:dev` documentation mode. The normal Tauri app always requires a real Telegram connection.

## 3. Verify the included TDLib library

```sh
npm run tdlib:ensure
```

The command should report `TDLib 1.8.64 is ready` and a verified SHA-256. The normal `npm run tauri dev`, `npm run tauri build`, and `npm run check` commands invoke this check automatically. Retract also asks the loaded library for its version and refuses anything other than 1.8.64.

## 4. Obtain Telegram application credentials

1. Sign in at <https://my.telegram.org>.
2. Open **API development tools**.
3. Create an application if necessary. Use **Retract** as the app title; do not use Telegram’s name or logo.
4. Copy the numeric `api_id` and the `api_hash`. Treat the hash as a secret.

Never put either value in frontend code, a `VITE_*` variable, a screenshot, or source control.

## 5A. Preferred: test-DC accounts

Telegram’s test environment is separate from normal Telegram. Create two disposable test accounts on the same test DC using phone numbers of the documented form `99966XYYYY`, where `X` is 1–3 and the sign-in code is `XXXXX`. Do not put private content there; Telegram warns that these accounts are trivially accessible and periodically wiped.

Retract signs in only to an existing account, so create/register the two test accounts with an official client configured for Telegram’s test environment before signing one into Retract.

Launch `npm run tauri dev`. In **Connect Telegram**, confirm **TDLib 1.8.64 included — READY** is visible. Enter the API ID and API hash, turn on **Use Telegram's test server**, then choose **Save settings**.

If Retract does not begin Telegram authorization, setup failed; stop instead of continuing.

## 5B. Controlled production smoke test

Only do this after the automated and preferably the test-DC checks. Open **Connection Settings**, turn off **Use Telegram's test server**, and choose **Save settings**. Leave the API-hash field blank to keep the value already stored in Keychain.

Retract keeps test-DC and production databases and job stores in separate app-data profiles.

### Optional environment overrides

Normal local testing does not require `RETRACT_*` environment variables. They remain available for CI and backend development; any active override is listed in the Settings screen and wins over the corresponding saved value. `RETRACT_TDLIB_PATH` is an advanced override for testing a custom library, not a setup requirement. Remove an override from the launching shell if you want the UI or bundled value to take effect.

## 6. Create an isolated verification group

A fresh private group is safer than an existing DM.

1. In an official Telegram client, create a private group named `Retract Verification — YYYY-MM-DD`.
2. Add only one consenting partner. Keep yourself as owner.
3. Ask the partner to keep the chat open on a separate device.
4. Send only harmless new fixtures from your account:
   - Text: `RETRACT-VERIFY-<random>-TEXT`
   - A non-sensitive screenshot with caption `RETRACT-VERIFY-<random>-PHOTO`
   - A small disposable text file named `retract-verify-<random>.txt`
5. Have the partner confirm all three are visible. Do not save or forward the media.

## 7. Delete only those fixtures

1. Open Retract and confirm the intended Telegram account is shown in the sidebar.
2. Select the new verification group in the left pane. Confirm it shows **Owner** and the expected capabilities.
3. Search for the unique `RETRACT-VERIFY-<random>` token.
4. Set the direction filter to **Mine**.
5. Confirm the result list contains only the three disposable fixtures.
6. Select each result. The right pane must report all three as **Delete for everyone**. Stop if any item says **Only removable for you** or **Cannot delete**.
7. Choose **Review deletion**. Confirm the frozen count, media types, and plan fingerprint.
8. Check the irreversible acknowledgement and choose **Delete for everyone**. Approve macOS authentication only after its native reason identifies the expected chat ID, message count, and plan token.
9. Ask the partner to confirm the text, screenshot message/caption, and file message disappear. Also refresh/reopen the chat on their side.
10. Record Retract’s completed job counters. Do not interpret a previously downloaded external copy as a failed message deletion.

Start with selected messages only. Do not test **Revoke history & remove chat** or **Delete group permanently** until this path succeeds and the partner confirms the result. Every destructive action requires macOS device-owner authentication; high-impact actions also require the exact group title.

Before testing the chat-wide action, verify that both people understand its scope: Retract requests revocation of the available history and removes the conversation from your own chat list. A later DM can recreate the conversation, and clearing a group’s history does not dissolve the group or remove its members.

## 8. Finish the isolated test

After both people confirm the expected result, use the official client to remove the disposable group or test Retract’s permanent group deletion as a separate critical-action case. Never reuse the group for real conversation.

For broader role, failure, cancellation, flood-wait, and restart coverage, continue with [TEST_PLAN.md](TEST_PLAN.md).
