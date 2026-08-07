//! Filing producer bugs in Jira.
//!
//! The analysis panel already knows what is wrong with an event stream, how
//! much traffic it affects and which producer sends it. This module is the
//! short walk from knowing that to the owning team knowing it too.
//!
//! See `docs/jira-integration.md` for the decisions behind the shape of this.

pub mod client;
pub mod issues;
pub mod oauth;
pub mod routing;
pub mod ticket;
pub mod tokens;
