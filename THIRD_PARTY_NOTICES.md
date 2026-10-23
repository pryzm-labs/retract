# Third-party notices

Retract includes third-party software under licenses separate from Retract's MIT License.

## TDLib 1.8.64

The bundled Apple-silicon TDLib static archive is built from the official Telegram Database Library source revision recorded in [`vendor/tdlib-dist/build-stamp.txt`](vendor/tdlib-dist/build-stamp.txt). TDLib is distributed under the Boost Software License 1.0; the exact bundled notice is in [`vendor/tdlib-dist/TDLib-LICENSE_1_0.txt`](vendor/tdlib-dist/TDLib-LICENSE_1_0.txt).

## nanoid

Retract vendors a small JavaScript distribution of nanoid. Its MIT license is in [`vendor/nanoid/LICENSE`](vendor/nanoid/LICENSE).

## SQLCipher Community 4.14.0

The archive storage gate statically builds the SQLCipher Community amalgamation supplied by `libsqlite3-sys` 0.38.2. Its source identifiers, hashes, compiler flags, and Rust package checksums are recorded in [`vendor/sqlcipher/provenance.json`](vendor/sqlcipher/provenance.json). The SQLCipher license is in [`vendor/sqlcipher/LICENSE.txt`](vendor/sqlcipher/LICENSE.txt).

## OpenSSL 3.6.3

SQLCipher uses the statically linked OpenSSL source supplied by `openssl-src` 300.6.1+3.6.3. The applicable upstream Apache License 2.0 notice is in [`vendor/sqlcipher/OPENSSL-LICENSE.txt`](vendor/sqlcipher/OPENSSL-LICENSE.txt); exact source and package identifiers are recorded in [`vendor/sqlcipher/provenance.json`](vendor/sqlcipher/provenance.json).

## Discord remediation dependencies

Discord identity/deletion HTTPS uses `reqwest` 0.12.28 with rustls rather than native TLS. Development-only isolated Chromium/Firefox protocol capture uses `tokio-tungstenite` 0.30.0. The native ZIP picker uses `tauri-plugin-dialog` 2.8.3 and `rfd` 0.16.0; Unix regular-file opening uses `libc` 0.2.189. Their declared licenses are MIT, Apache-2.0, or the stated MIT/Apache-2.0 alternatives. The exact registry checksums are frozen in `src-tauri/Cargo.lock`:

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| reqwest | 0.12.28 | `eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147` |
| tokio-tungstenite | 0.30.0 | `17a073bfed563fa236697a068031408a93cd9522e08abf9933ead3e73411bd71` |
| tauri-plugin-dialog | 2.8.3 | `b7461aaacf1901915ea7b9f5cd74f8224a351ed96af0a62c07fce934003cb34b` |
| rfd | 0.16.0 | `a15ad77d9e70a92437d8f74c35d99b4e4691128df018833e99f90bcd36152672` |
| libc | 0.2.189 | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` |

No browser binary is bundled. Release builds use manual token entry and disable browser-debug capture. The development compatibility harness discovers an installed browser and uses a fresh temporary profile with synthetic credentials.

Other dependencies and their resolved versions are recorded in `package-lock.json` and the Cargo lockfiles. Their licenses remain the property of their respective authors.

## ZIP reader 9.0.5

`crates/discord-archive` pins `zip = "=9.0.5"` (MIT), with defaults disabled and only `deflate-flate2-zlib-rs` enabled. The resolved codec chain is `zip` → `flate2` → `zlib-rs` (Rust allocator); stored entries require no codec. The application permits only stored (0) and deflate (8). AES/legacy encryption and bzip2, deflate64, LZMA, PPMd, XZ and Zstandard features are not enabled. Encryption flags/metadata are rejected independently of feature selection. Retract reads entries without calling extraction APIs; the ZIP dependency also includes Unix extraction support through `nix`, which Retract does not invoke.

Resolved Cargo package checksums for the complete ZIP dependency subtree:

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| zip | 9.0.5 | `6e04976da629d3b04204ba8105b396a193af8ad2583b2c79bfb3ab8af580a11e` |
| flate2 | 1.1.10 | `6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb` |
| zlib-rs | 0.6.7 | `34b31d188d9d685a4f9c7b46d6e36631b07058d2cfe190267adce54dc230bf12` |
| crc32fast | 1.5.1 | `8498c871161e1742aaa9d52551b2d6ebdd4c3d45a3be423e3728f33b955be550` |
| cfg-if | 1.0.4 | `9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801` |
| indexmap | 2.14.2 | `cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855` |
| equivalent | 1.0.2 | `877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f` |
| hashbrown | 0.17.1 | `ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a` |
| memchr | 2.8.3 | `cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98` |
| typed-path | 0.12.3 | `8e28f89b80c87b8fb0cf04ab448d5dd0dd0ade2f8891bae878de66a75a28600e` |
| nix | 0.31.3 | `cf20d2fde8ff38632c426f1165ed7436270b44f199fc55284c38276f9db47c3d` |
| bitflags | 2.13.2 | `3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06` |
| cfg_aliases | 0.2.2 | `f079e83a288787bcd14a6aea84cee5c87a67c5a3e660c30f557a3d24761b3527` |
| libc | 0.2.189 | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` |

Registry package versions and checksums, including the other parser dependencies, are frozen in [`crates/discord-archive/Cargo.lock`](crates/discord-archive/Cargo.lock). Project checks run offline in Docker with `--locked`.

`ZipArchive::new` builds a central-directory index; its capacity checks are not application quotas. Our preflight checks the directory's location, length and count before constructing it. Construction uses a cached read-only view containing only the checked directory and one canonical EOCD, with virtual zero payload bytes and omitted archive comments; embedded end/ZIP64 markers in the directory fail closed. Thus fallback cannot reach another payload/comment directory or allocate from its declarations.

The library's local-header lookup checks the signature and computes payload offsets from local lengths; it does not establish all local/central agreement. Our reader checks names, versions, flags, methods, timestamps, CRC declarations, sizes and contiguous non-overlapping in-file ranges independently. Library CRC validation happens at clean EOF; abandoning a stream does not validate it. Our consumed-entry wrapper counts observed bytes, enforces ratio/pass quotas and cancellation, drains to EOF, checks the exact expanded count and only then marks the entry validated. Unselected payloads are not decompressed or CRC-certified.

This policy deliberately rejects ZIP64 (including contradictory sentinels/extra fields), data descriptors, prefixes/gaps, multi-disk archives, Unicode path overrides and non-ASCII entry names. ASCII case collisions, dot/space suffixes and file/directory conflicts fail closed. Unicode normalization support and additional ZIP variants require an explicit review after format evidence. ZIP64 rejection means classic ZIP's 65,534-entry representable policy limit can bind before the 100,000-entry ceiling. The supported JSON profile and its limits are recorded in the synthetic Discord fixture manifest.

The backend lockfile retains flate2 1.1.9, crc32fast 1.5.0 and indexmap 2.14.0 while the standalone parser lockfile resolves the versions above. Both use zip 9.0.5 and zlib-rs 0.6.7 without encryption or additional ZIP codecs. The ZIP subtree licenses are MIT, Apache-2.0/MIT alternatives, BSD-2-Clause for nix, and Zlib for zlib-rs; the full parser graph also includes the Unicode-3.0 component of unicode-ident's license.
