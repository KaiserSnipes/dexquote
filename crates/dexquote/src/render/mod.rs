//! Output rendering. Two modes:
//!
//!   - `human` — hand-rolled Unicode table matching the target mockup, with
//!     colors and a `★ best` marker pinned tight against the winning row.
//!   - `json`  — scripting output, one object per backend, always plain.
//!
//! The streaming renderer in `stream.rs` reuses the row formatter from
//! `table.rs` so in-flight and finished rows look identical.

pub mod benchmark;
pub mod depth;
pub mod json;
pub mod minimal;
pub mod route;
pub mod stream;
pub mod table;

pub use json::render_json;
pub use minimal::render_minimal;
pub use table::{render_human, render_token_list, RenderInput};
