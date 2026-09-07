//! LANline server library.
//!
//! Every module the `lanline-server` binary is built from lives here so that
//! sibling tools — notably the `lanline-hypervisor` launcher — can reuse the
//! pieces that are not tied to a running pipeline: `net` (port allocation,
//! address detection), `mdns` (service advertisement), `discovery` /
//! `model` (the beacon payload shape), `config`.
//!
//! The binary entry point is `src/main.rs`; it wires these together and owns
//! `async fn main`.

pub mod adsb;
pub mod ais;
pub mod analysis;
pub mod api;
pub mod apt;
pub mod aprs;
pub mod audio;
pub mod catalog;
pub mod config;
pub mod discovery;
pub mod error;
pub mod flight;
pub mod mdns;
pub mod media;
pub mod model;
pub mod net;
pub mod radio;
pub mod registry;
pub mod sessions;
pub mod state;
pub mod tls;
pub mod util;
pub mod vessel;
pub mod voice;
