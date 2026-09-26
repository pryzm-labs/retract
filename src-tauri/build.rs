fn main() {
    if std::env::var("TARGET").is_ok_and(|target| target.ends_with("apple-darwin")) {
        use std::io::Read;

        use flate2::read::GzDecoder;
        use sha2::{Digest, Sha256};

        const EXPECTED_COMPRESSED_SHA256: [u8; 32] = [
            0xe9, 0x3e, 0x21, 0x34, 0xe9, 0xfb, 0x57, 0xf7, 0xd0, 0x19, 0xa8, 0x80, 0x2f, 0x85,
            0x18, 0xb9, 0xc0, 0x33, 0x98, 0x06, 0x22, 0x4a, 0xf1, 0x7d, 0xf2, 0x1f, 0xa9, 0x65,
            0x78, 0x5a, 0xfb, 0x95,
        ];
        const EXPECTED_ARCHIVE_SHA256: [u8; 32] = [
            0xaa, 0xb5, 0x73, 0x6f, 0x73, 0x73, 0x19, 0xa1, 0x3b, 0xcb, 0x87, 0x1a, 0xa2, 0xb8,
            0xa7, 0xa9, 0x0a, 0x33, 0xe2, 0x8e, 0xc9, 0x70, 0x8f, 0xee, 0x53, 0xdb, 0xd3, 0x87,
            0xa2, 0x4b, 0x98, 0xe4,
        ];
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory is set"),
        );
        let tdlib = manifest_dir.join("../vendor/tdlib-dist/libtdjson_static.a.gz");
        let compressed = std::fs::read(&tdlib).expect("read pinned TDLib static dependency");
        assert_eq!(
            Sha256::digest(&compressed).as_ref(),
            EXPECTED_COMPRESSED_SHA256,
            "compressed TDLib dependency does not match Retract's reviewed SHA-256"
        );
        let mut archive = Vec::new();
        GzDecoder::new(compressed.as_slice())
            .read_to_end(&mut archive)
            .expect("decompress pinned TDLib static dependency");
        assert_eq!(
            Sha256::digest(&archive).as_ref(),
            EXPECTED_ARCHIVE_SHA256,
            "TDLib static archive does not match Retract's reviewed SHA-256"
        );
        println!("cargo:rerun-if-changed={}", tdlib.display());
        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo output directory is set"),
        );
        std::fs::write(out_dir.join("libtdjson_retract.a"), &archive)
            .expect("write verified TDLib static linker input");
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=tdjson_retract");
        println!("cargo:rustc-link-lib=dylib=z");
        println!("cargo:rustc-link-lib=dylib=c++");
    }
    tauri_build::build()
}
