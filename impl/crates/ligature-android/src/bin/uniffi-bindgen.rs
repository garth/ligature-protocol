//! Generates the Kotlin bindings from the compiled library:
//!
//! ```sh
//! cargo build -p ligature-android
//! cargo run -p ligature-android --features bindgen --bin uniffi-bindgen -- \
//!   generate --library target/debug/libligature_android.so --language kotlin --out-dir out/
//! ```

fn main() {
    uniffi::uniffi_bindgen_main();
}
