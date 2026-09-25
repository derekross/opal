//! Runtime pieces shared by the Opal and Peridot daemons: how their own
//! features sign, how they talk to relays, and the local control socket.

pub mod ipc;
pub mod qr;
pub mod relays;
pub mod signer;

pub use signer::{EventSigner, SignError};
