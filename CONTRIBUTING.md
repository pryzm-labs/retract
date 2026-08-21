# Contributing to Retract

Thank you for helping make Telegram cleanup safer and clearer.

## Before opening a change

- Search existing issues and keep each change narrowly scoped.
- Use synthetic fixtures. Never commit or upload chat exports, credentials, session files, TDLib databases, unredacted conversations, or real-account screenshots.
- Never test destructive behavior on a valuable account or existing conversation. Use Telegram's test DC where possible, or a new isolated chat with informed participants.
- Preserve the core invariant: an everyone-scoped deletion must never silently fall back to a self-only deletion.

## Development

Retract pins Node in `.nvmrc`, Rust in `rust-toolchain.toml`, dependencies in lockfiles, and TDLib in `vendor/tdlib-dist`.

```sh
npm ci --ignore-scripts
npm test
npm run build
```

The preferred full check uses Docker BuildKit and leaves no runnable image:

```sh
npm run container:check
```

Native macOS development uses:

```sh
npm run tauri dev
```

Telegram credentials are entered in the app. Environment variables in `.env.example` are optional developer/CI overrides and must never be committed.

## Pull requests

Add regression tests before changing behavior. Update deletion-scope wording, documentation, lockfiles, and synthetic screenshots when affected. Describe what you verified and any limitations. By contributing, you agree that your contribution is licensed under the [MIT License](LICENSE).

Report security issues privately as described in [SECURITY.md](SECURITY.md).
