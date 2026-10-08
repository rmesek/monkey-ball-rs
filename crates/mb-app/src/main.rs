//! Desktop entry point. The browser build enters through `mb-web` and Android through `mb-android`; there
//! this binary is an empty shell so `cargo check --target wasm32-unknown-unknown` still covers the crate.

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod desktop;

fn main() {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    desktop::main();
}
