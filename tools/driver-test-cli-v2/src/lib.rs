// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! # driver-test-cli-v2
//!
//! A single-command CLI that orchestrates PowerShell scripts to smoketest a
//! pre-built Windows driver package on a local Hyper-V VM.

pub mod capture;
pub mod error;
pub mod pipeline;
pub mod ps_runner;
pub mod report;
