//! mpp-rs — a small, partial Rust port of MPXJ's MPP reader for WebAssembly.
//!
//! Native API:   `parse_to_json(&[u8]) -> Result<String, String>`
//! WASM/Typst:   exported `parse_mpp(bytes) -> json bytes` (see README).
//!
//! Scope: reads tasks (UID, ID, Name, OutlineLevel, Start, Finish, % complete)
//! from MPP14 (Project 2010+) files. Not a full MPXJ — see README for the
//! deliberately-unported long tail and the LGPL-2.1 provenance.

mod fixed;
mod model;
mod mpp14;
mod util;
mod var;

/// Parse MPP bytes and return the project as a JSON string.
pub fn parse_to_json(bytes: &[u8]) -> Result<String, String> {
    let project = mpp14::parse(bytes)?;
    serde_json::to_string(&project).map_err(|e| format!("serialize: {e}"))
}

// ---------------------------------------------------------------------------
// Typst plugin ABI. Typst calls `plugin("mpp_rs.wasm").parse_mpp(bytes)`; the
// function receives the file bytes and returns JSON bytes. Errors are returned
// via the protocol's error channel (a returned Err becomes a Typst panic with
// the message), so the Typst side gets a readable diagnostic.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_minimal_protocol::*;

    initiate_protocol!();

    #[wasm_func]
    pub fn parse_mpp(bytes: &[u8]) -> Result<Vec<u8>, String> {
        crate::parse_to_json(bytes).map(String::into_bytes)
    }
}
