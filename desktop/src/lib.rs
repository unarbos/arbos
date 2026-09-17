//! Arbos desktop shell — Arbos UI over the Rust kernel (`crates/arbos-kernel`).

// The driver reports the whole of what the window believes as one `json!`, and
// that macro recurses once per key. The default 128 was reached as the state
// grew; a larger number costs nothing but lets the driver keep answering in one
// object rather than being split into shapes no test asked for.
#![recursion_limit = "512"]

pub mod agent;
pub mod assets;
pub mod boardhub;
pub mod build;
pub mod data;
pub mod driver;
pub mod feedback;
pub mod fonts;
pub mod kernel;
pub mod markup;
pub mod memory;
pub mod model;
pub mod notify_os;
pub mod permissions;
pub mod reading;
pub mod update;
pub mod view;
pub mod voice;
pub mod voice_ws;
