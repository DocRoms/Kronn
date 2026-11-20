//! Tests of `crate::api_tests` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api_tests::*;

#[path = "../api_tests_tests_up.rs"]
pub(crate) mod tests;
