//! A terminal explorer and editor for memory cards.
//!
//! Split so the rules can be tested without a terminal: [`model`] is what a card is and what may
//! be done to one, and [`app`] is what that looks like and what a key does. The binary is the
//! event loop between them.

pub mod app;
pub mod model;
