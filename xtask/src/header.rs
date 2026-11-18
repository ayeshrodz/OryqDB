//! Header verification scaffold (expanded in W11 when `include/oryq.h` is generated).

use std::path::Path;

pub fn check_header(_root: &Path) -> Result<(), String> {
    println!("xtask header: no generated include/oryq.h yet (scheduled for W11).");
    Ok(())
}
