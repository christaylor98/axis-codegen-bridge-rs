use std::fs;

use super::CoreBundle;

/// Reads a Core IR 0.5 bundle: the native `.axbi` canonical binary (AXCI header). Nothing else is accepted.
pub fn load_core_bundle(path: &str) -> Result<CoreBundle, String> {
    let bytes = fs::read(path).map_err(|e| format!("failed to open {}: {}", path, e))?;
    super::parse_axbi(&bytes).map_err(|e| format!("{}: {}", path, e))
}

pub fn load_core_bundle_from_bytes(bytes: &[u8]) -> Result<CoreBundle, String> {
    super::parse_axbi(bytes)
}
