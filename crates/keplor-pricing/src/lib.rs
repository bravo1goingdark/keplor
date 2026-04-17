//! # keplor-pricing
//!
//! Cost accounting for LLM traffic.  Loads the bundled LiteLLM
//! `model_prices_and_context_window.json` catalogue and computes cost in
//! int64 nanodollars, correctly attributing cached / reasoning / batch /
//! tier / geo multipliers.
//!
//! ## Quick start
//!
//! ```
//! use keplor_core::{Cost, Provider, Usage};
//! use keplor_pricing::{Catalog, ModelKey};
//! use keplor_pricing::compute::{compute_cost, CostOpts};
//!
//! let catalog = Catalog::load_bundled().expect("bundled catalog");
//! let key = ModelKey::new("gpt-4o");
//! let pricing = catalog.lookup(&key).expect("model found");
//!
//! let usage = Usage {
//!     input_tokens: 1_000,
//!     output_tokens: 500,
//!     ..Usage::default()
//! };
//!
//! let cost = compute_cost(&Provider::OpenAI, pricing, &usage, &CostOpts::default());
//! assert!(cost > Cost::ZERO);
//! ```

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod catalog;
pub mod compute;
pub mod error;
pub mod model;

pub use catalog::{Catalog, ModelKey, PRICING_CATALOG_DATE, PRICING_CATALOG_VERSION};
pub use error::PricingError;
