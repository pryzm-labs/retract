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

Other dependencies and their resolved versions are recorded in `package-lock.json` and the Cargo lockfiles. Their licenses remain the property of their respective authors.
