//! Optional Prometheus recorder/listener installation.
//!
//! No listener opens without the `prometheus` feature and a valid
//! `MEMORY_PROMETHEUS_LISTEN_ADDR`. 127.0.0.1:0 is supported for tests.

#![allow(dead_code)]
