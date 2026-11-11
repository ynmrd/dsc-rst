//! Discord-flavoured Markdown parser, mention/emoji helpers and message grouping
//!
//! Pure logic only: everything here is UI-agnostic and never panics on arbitrary input.

pub mod compose;
pub mod emoji;
pub mod grouping;
pub mod markdown;
pub mod timestamp;

mod util;
