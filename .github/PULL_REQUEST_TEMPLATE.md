## Summary

Describe the user-facing outcome and why the change is needed.

## Verification

- [ ] I added or updated regression tests before changing behavior.
- [ ] `npm test` passes.
- [ ] `npm run container:check` passes, or I explained why it could not run.
- [ ] Production builds contain no fixture adapter or synthetic chat data.
- [ ] Everyone-scoped deletion never falls back to self-only deletion.
- [ ] Destructive-action and deletion-scope copy remains explicit.
- [ ] Fixtures and screenshots contain only synthetic, privacy-safe data.
- [ ] I did not add credentials, sessions, databases, chat exports, or private conversation content.
- [ ] I updated documentation, lockfiles, and the screenshot when applicable.

## Risk and limitations

Describe Telegram capability assumptions, migrations, and anything reviewers should test manually.
