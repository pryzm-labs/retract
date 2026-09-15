# Third-party notices

Retract includes third-party software under licenses separate from Retract's MIT License.

## TDLib 1.8.64

The bundled Apple-silicon `libtdjson.dylib` is built from the official Telegram Database Library source revision recorded in [`vendor/tdlib-dist/build-stamp.txt`](vendor/tdlib-dist/build-stamp.txt). TDLib is distributed under the Boost Software License 1.0; the exact bundled notice is in [`vendor/tdlib-dist/TDLib-LICENSE_1_0.txt`](vendor/tdlib-dist/TDLib-LICENSE_1_0.txt).

## nanoid

Retract vendors a small JavaScript distribution of nanoid. Its MIT license is in [`vendor/nanoid/LICENSE`](vendor/nanoid/LICENSE).

## SQLCipher Community 4.14.0

The archive storage gate statically builds the SQLCipher Community amalgamation supplied by `libsqlite3-sys` 0.38.2. Its source identifiers, hashes, compiler flags, and Rust package checksums are recorded in [`vendor/sqlcipher/provenance.json`](vendor/sqlcipher/provenance.json). The SQLCipher license is in [`vendor/sqlcipher/LICENSE.txt`](vendor/sqlcipher/LICENSE.txt).

## OpenSSL 3.6.3

SQLCipher uses the statically linked OpenSSL source supplied by `openssl-src` 300.6.1+3.6.3. The applicable upstream Apache License 2.0 notice is in [`vendor/sqlcipher/OPENSSL-LICENSE.txt`](vendor/sqlcipher/OPENSSL-LICENSE.txt); exact source and package identifiers are recorded in [`vendor/sqlcipher/provenance.json`](vendor/sqlcipher/provenance.json).

## Discord remediation dependencies

Discord identity/deletion HTTPS uses `reqwest` 0.12.28 with rustls rather than native TLS. Isolated Chromium/Firefox protocol capture uses `tokio-tungstenite` 0.28.0. The native ZIP picker uses `tauri-plugin-dialog` 2.7.3 and `rfd` 0.16.0; Unix regular-file opening uses `libc` 0.2.189. Their declared licenses are MIT, Apache-2.0, or the stated MIT/Apache-2.0 alternatives. The exact registry checksums are frozen in `src-tauri/Cargo.lock`:

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| reqwest | 0.12.28 | `eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147` |
| tokio-tungstenite | 0.28.0 | `d25a406cddcc431a75d3d9afc6a7c0f7428d4891dd973e4d54c56b46127bf857` |
| tauri-plugin-dialog | 2.7.3 | `61854a36651aa48381e5e209f69a01273b77f3f9f91f0c430b1b98d33bd47229` |
| rfd | 0.16.0 | `a15ad77d9e70a92437d8f74c35d99b4e4691128df018833e99f90bcd36152672` |
| libc | 0.2.189 | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` |

No browser binary is bundled. Browser discovery targets an already installed supported browser, and automatic sign-in uses a fresh temporary profile rather than the user's existing profile.

Other dependencies and their resolved versions are recorded in `package-lock.json` and the Cargo lockfiles. Their licenses remain the property of their respective authors.

## ZIP reader 8.6.0 (Task 1 policy review, 2026-09-09)

`crates/discord-archive` pins `zip = "=8.6.0"` (MIT), with defaults disabled and only `deflate-flate2-zlib-rs` enabled. The resolved codec chain is `zip` → `flate2` 1.1.10 → `zlib-rs` 0.6.7 (Rust allocator); stored entries require no codec. The application permits only stored (0) and deflate (8). AES/legacy encryption and bzip2, deflate64, LZMA, PPMd, XZ and Zstandard features are not enabled. Encryption flags/metadata are rejected independently of feature selection. The lockfile also contains the explicitly requested Serde, JSON, SHA-256 and fixed-error dependencies; no network/runtime/storage dependency is present.

Resolved Cargo package checksums for the complete ZIP dependency subtree:

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| zip | 8.6.0 | `2d04a6b5381502aa6087c94c669499eb1602eb9c5e8198e534de571f7154809b` |
| flate2 | 1.1.10 | `6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb` |
| zlib-rs | 0.6.7 | `34b31d188d9d685a4f9c7b46d6e36631b07058d2cfe190267adce54dc230bf12` |
| crc32fast | 1.5.1 | `8498c871161e1742aaa9d52551b2d6ebdd4c3d45a3be423e3728f33b955be550` |
| cfg-if | 1.0.4 | `9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801` |
| indexmap | 2.14.2 | `cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855` |
| equivalent | 1.0.2 | `877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f` |
| hashbrown | 0.17.1 | `ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a` |
| memchr | 2.8.3 | `cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98` |
| typed-path | 0.12.3 | `8e28f89b80c87b8fb0cf04ab448d5dd0dd0ade2f8891bae878de66a75a28600e` |

All 31 registry package versions and checksums, including the other parser dependencies, are frozen in [`crates/discord-archive/Cargo.lock`](crates/discord-archive/Cargo.lock). Cargo generated that file in ARM64 Docker; project checks run offline with `--locked`.

Local source review covered `zip/src/read/zip_archive.rs`, `zip/src/read/readers.rs`, `zip/src/types.rs`, `zip/src/crc32.rs`, the crate manifest and the resolved feature tree. `ZipArchive::new` eagerly builds a central-directory index and ignores local headers. Its capacity checks are not application quotas, and its metadata search retries earlier EOCD candidates after decoding errors. Our preflight checks the directory's location, length and count before constructing it. Construction uses a cached read-only view containing only the checked directory and one canonical EOCD, with virtual zero payload bytes and omitted archive comments; embedded end/ZIP64 markers in the directory fail closed. Thus fallback cannot reach another payload/comment directory or allocate from its declarations.

The library's local-header lookup checks the signature and computes payload offsets from local lengths; it does not establish all local/central agreement. Our reader checks names, versions, flags, methods, timestamps, CRC declarations, sizes and contiguous non-overlapping in-file ranges independently. Library CRC validation happens at clean EOF; abandoning a stream does not validate it. Our consumed-entry wrapper counts observed bytes, enforces ratio/pass quotas and cancellation, drains to EOF, checks the exact expanded count and only then marks the entry validated. Unselected payloads are not decompressed or CRC-certified.

This policy deliberately rejects ZIP64 (including contradictory sentinels/extra fields), data descriptors, prefixes/gaps, multi-disk archives, Unicode path overrides and non-ASCII entry names. ASCII case collisions, dot/space suffixes and file/directory conflicts fail closed. Unicode normalization support and additional ZIP variants require an explicit review after format evidence. ZIP64 rejection means classic ZIP's 65,534-entry representable policy limit can bind before the design's 100,000-entry ceiling. Task 1 made no current-format claim; the subsequent frozen JSON profile and its limits are recorded in the synthetic Discord fixture manifest.

Task 7's offline feature/license audit preserved these parser pins and codecs. The backend lockfile reuses its existing flate2 1.1.9, crc32fast 1.5.0 and indexmap 2.14.0 versions while the standalone parser lockfile resolves the versions above. Both use zip 8.6.0 and zlib-rs 0.6.7 without encryption or additional ZIP codecs. The ZIP subtree licenses are MIT, Apache-2.0/MIT alternatives, and Zlib for zlib-rs; the full parser graph also includes the Unicode-3.0 component of unicode-ident's license. The generated benchmark adds no dependency or lockfile change.
