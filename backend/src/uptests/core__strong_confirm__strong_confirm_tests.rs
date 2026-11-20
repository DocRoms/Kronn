//! Tests of `crate::core::strong_confirm` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::strong_confirm::*;
use aes_gcm::aead::{rand_core::RngCore, OsRng};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
#[path = "../core/strong_confirm_tests.rs"]
pub(crate) mod strong_confirm_tests;
