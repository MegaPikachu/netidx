#![feature(proc_macro, proc_macro_non_items, generators)]
#![recursion_limit="2048"]

extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate chrono;
extern crate tokio;
extern crate tokio_io;
extern crate futures_await as futures;
extern crate rand;
#[macro_use]
extern crate error_chain;

mod line_writer;
pub mod utils;
pub mod error;
pub mod path;
pub mod resolver_client;
pub mod resolver_server;
pub mod publisher;
pub mod subscriber;

#[cfg(test)]
extern crate tokio_timer;

#[cfg(test)]
mod tests;
