//! Desktop entry point. The browser build enters through `mb-web`; there this binary is an empty shell so
//! `cargo check --target wasm32-unknown-unknown` still covers the crate.

#[cfg(not(target_arch = "wasm32"))]
mod desktop;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    desktop::main();
}
