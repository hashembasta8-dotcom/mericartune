//! OBD-II protocol stack: ELM327 transport engine, SAE J1979 PIDs, DTC codec.
//!
//! This is a real protocol implementation, not a stub:
//! - [`transport`] — pluggable transports (mock for tests/demo, serial behind the
//!   `hardware` feature). The engine never touches a device directly.
//! - [`elm`] — ELM327 AT-command protocol: init sequence, echo/space handling,
//!   error frames (`NO DATA`, `CAN ERROR`, …), ISO-TP frame assembly.
//! - [`pid`] — SAE J1979 Mode 01 PID registry with the real scaling formulas.
//! - [`dtc`] — Mode 03/07/0A/04 diagnostic trouble codes (P/C/B/U encoding).

pub mod dtc;
pub mod elm;
pub mod pid;
pub mod sim;
pub mod transport;

pub use dtc::{Dtc, DtcKind};
pub use elm::{ElmClient, ObdError, ObdResponse};
pub use pid::{Pid, PidValue, PID_REGISTRY};
pub use transport::{MockTransport, Transport};
