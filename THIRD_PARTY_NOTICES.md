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

Discord identity/deletion HTTPS uses `reqwest` 0.12.28 with rustls rather than native TLS. Development-only isolated Chromium/Firefox protocol capture uses `tokio-tungstenite` 0.30.0. The native ZIP picker uses `tauri-plugin-dialog` 2.8.4 and `rfd` 0.17.2; Unix regular-file opening uses `libc` 0.2.191. Their declared licenses are MIT, Apache-2.0, or the stated MIT/Apache-2.0 alternatives. The exact registry checksums are frozen in `src-tauri/Cargo.lock`:

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| reqwest | 0.12.28 | `eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147` |
| tokio-tungstenite | 0.30.0 | `17a073bfed563fa236697a068031408a93cd9522e08abf9933ead3e73411bd71` |
| tauri-plugin-dialog | 2.8.4 | `ce0fb9c2b6d34a9720627ed930ebc8af26f61e648695aecaeb03d5c33c5896f5` |
| rfd | 0.17.2 | `20dafead71c16a34e1ff357ddefc8afc11e7d51d6d2b9fbd07eaa48e3e540220` |
| libc | 0.2.191 | `65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a` |

`rfd` includes platform-specific file-dialog implementations and a Windows `windows-sys` 0.61.2 dependency; the default Linux backend uses GTK3. The asynchronous runtime uses Tokio 1.54.1 (MIT), registry checksum `b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3`.

No browser binary is bundled. Release builds use manual token entry and disable browser-debug capture. The development compatibility harness discovers an installed browser and uses a fresh temporary profile with synthetic credentials.

Frontend icons use `lucide-react` 1.71.0 (ISC). Development tools include `@vitejs/plugin-react` 6.3.0, Vite 8.3.7 and Vitest 5.1.0 (MIT). Their resolved graph includes Rolldown 1.2.18 and its optional platform bindings, `@oxc-project/types` 0.156.0, PostCSS 8.5.32 and aligned Vitest mocker/spy packages. Exact tarball URLs and integrity digests are frozen in `package-lock.json`; dependency lifecycle scripts are disabled during the container installation.

Other dependencies and their resolved versions are recorded in `package-lock.json` and the Cargo lockfiles. Their licenses remain the property of their respective authors.

## ZIP reader 9.1.2

`crates/discord-archive` pins `zip = "=9.1.2"` (MIT), with defaults disabled and only `deflate-flate2-zlib-rs` enabled. The resolved codec chain is `zip` → `flate2` → `zlib-rs` (Rust allocator); stored entries require no codec. The application permits only stored (0) and deflate (8). AES/legacy encryption and bzip2, deflate64, LZMA, PPMd, XZ and Zstandard features are not enabled. Encryption flags/metadata are rejected independently of feature selection. Retract reads entries without calling extraction APIs; the ZIP dependency also includes Unix extraction support through `nix` and Windows filesystem support through `windows-sys`, which Retract does not invoke. `getrandom` 0.4.3 is a mandatory ZIP dependency used for random extraction-staging directory names; indexed archive reads do not call that extraction path. The lockfile also records its target-specific EFI and wasm/JavaScript dependencies.

Resolved Cargo package checksums for the complete ZIP dependency subtree across all lockfile targets (including build-time proc macros and target-specific packages):

| Package | Version | Cargo registry checksum (SHA-256) |
| --- | --- | --- |
| bitflags | 2.13.2 | `3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06` |
| bumpalo | 3.20.3 | `72f5acc6cb2ba439de613abc23857ec3d78374d8ed5ac84e9d11336e87da8649` |
| cfg-if | 1.0.4 | `9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801` |
| cfg_aliases | 0.2.2 | `f079e83a288787bcd14a6aea84cee5c87a67c5a3e660c30f557a3d24761b3527` |
| crc32fast | 1.5.1 | `8498c871161e1742aaa9d52551b2d6ebdd4c3d45a3be423e3728f33b955be550` |
| equivalent | 1.0.2 | `877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f` |
| flate2 | 1.1.10 | `6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb` |
| getrandom | 0.4.3 | `300e883d756b2e4ec94e02791f39b04b522276138852cfc41d9fb7e904106099` |
| hashbrown | 0.17.1 | `ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a` |
| indexmap | 2.14.2 | `cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855` |
| js-sys | 0.3.107 | `8dc09d2154098bbf4ef54eab6b117a3c24cbbffe94046b16ee6d6d4510584d3a` |
| libc | 0.2.191 | `65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a` |
| memchr | 2.8.3 | `cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98` |
| nix | 0.31.3 | `cf20d2fde8ff38632c426f1165ed7436270b44f199fc55284c38276f9db47c3d` |
| proc-macro2 | 1.0.107 | `985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9` |
| quote | 1.0.47 | `1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001` |
| r-efi | 6.0.0 | `f8dcc9c7d52a811697d2151c701e0d08956f92b0e24136cf4cf27b57a6a0d9bf` |
| rustversion | 1.0.23 | `cf54715a573b99ac80df0bc206da022bcd442c974952c7b9720069370852e21f` |
| syn | 3.0.5 | `12df2e0110f65b775f769bb17ef989067a1d931b2eb822bd4346631eeada89f9` |
| typed-path | 0.13.0 | `9ce898823d8cd0f028662d2f349694b1af0a5cc9be83f1d213a00d3147445e1e` |
| unicode-ident | 1.0.24 | `e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75` |
| wasm-bindgen | 0.2.130 | `a0181c14ddd6c7b8b0b0132a5f1ea2f32be0d893cd3ea97b6e3b28886b8a3d1c` |
| wasm-bindgen-macro | 0.2.130 | `6e931a02a39abdbf66966fc35902b5ed58344750a70885bed4ebb3d8eaf28ed9` |
| wasm-bindgen-macro-support | 0.2.130 | `2ca4b54280e455bd831e33a6b8e734baf432fe63fbc35c42ac14ce67a043a53a` |
| wasm-bindgen-shared | 0.2.130 | `3b1f9c3798725016c6ee5f2f57869ec0831e8bbc0ab0310a86e787e97f41389e` |
| windows-link | 0.2.1 | `f0805222e57f7521d6a62e36fa9163bc891acd422f971defe97d64e70d0a4fe5` |
| windows-sys | 0.61.2 | `ae137229bcbd6cdf0f7b80a31df61766145077ddf49416a728b02cb3921ff3fc` |
| zip | 9.1.2 | `ec2e3161de27091f18a831d9494e9c53378a34833af5c1dda35ad85849da18d6` |
| zlib-rs | 0.6.7 | `34b31d188d9d685a4f9c7b46d6e36631b07058d2cfe190267adce54dc230bf12` |

Registry package versions and checksums, including the other parser dependencies, are frozen in [`crates/discord-archive/Cargo.lock`](crates/discord-archive/Cargo.lock). Project checks run offline in Docker with `--locked`.

`ZipArchive::new` builds a central-directory index; its capacity checks are not application quotas. Our preflight checks the directory's location, length and count before constructing it. Construction uses a cached read-only view containing only the checked directory and one canonical EOCD, with virtual zero payload bytes and omitted archive comments; embedded end/ZIP64 markers in the directory fail closed. Thus fallback cannot reach another payload/comment directory or allocate from its declarations.

The library's local-header lookup checks the signature and computes payload offsets from local lengths; it does not establish all local/central agreement. Our reader checks names, versions, flags, methods, timestamps, CRC declarations, sizes and contiguous non-overlapping in-file ranges independently. Library CRC validation happens at clean EOF; abandoning a stream does not validate it. Our consumed-entry wrapper counts observed bytes, enforces ratio/pass quotas and cancellation, drains to EOF, checks the exact expanded count and only then marks the entry validated. Unselected payloads are not decompressed or CRC-certified.

This policy deliberately rejects ZIP64 (including contradictory sentinels/extra fields), data descriptors, prefixes/gaps, multi-disk archives, Unicode path overrides and non-ASCII entry names. ASCII case collisions, dot/space suffixes and file/directory conflicts fail closed. Unicode normalization support and additional ZIP variants require an explicit review after format evidence. ZIP64 rejection means classic ZIP's 65,534-entry representable policy limit can bind before the 100,000-entry ceiling. The supported JSON profile and its limits are recorded in the synthetic Discord fixture manifest.

The backend lockfile uses flate2 1.1.10, crc32fast 1.5.0 and indexmap 2.14.0 while the standalone parser lockfile resolves the versions above. Both use zip 9.1.2 and zlib-rs 0.6.7 without encryption or additional ZIP codecs. The backend also enables flate2's default Rust backend, resolving miniz_oxide 0.9.1 (MIT/Zlib/Apache-2.0), checksum `b63fbc4a50860e98e7b2aa7804ded1db5cbc3aff9193adaff57a6931bf7c4b4c`; PNG independently retains miniz_oxide 0.8.9. The ZIP subtree licenses are MIT, Apache-2.0/MIT alternatives, BSD-2-Clause for nix, and Zlib for zlib-rs; the full parser graph also includes the Unicode-3.0 component of unicode-ident's license.
