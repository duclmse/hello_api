//! Import/export adapters for external HTTP collection formats.
//!
//! # Supported formats
//! - [`postman`] — Postman Collection v2.0 and v2.1 (JSON)
//! - [`bruno`] — Bruno `.bru` file format
//! - [`curl`] — curl command import/export
//! - [`opencollection`] — OpenCollection v1.0.0 (JSON/YAML)
//! - [`openapi`] — OpenAPI 3.x / Swagger 2.0 (YAML/JSON)

pub mod bru_parser;
pub mod bruno;
pub mod curl;
pub mod openapi;
pub mod opencollection;
pub mod postman;

pub use bruno::{BrunoAdapter, BrunoError};
pub use curl::{CurlAdapter, CurlError};
pub use openapi::{OpenApiAdapter, OpenApiCollection, OpenApiError};
pub use opencollection::{OpenCollection, OpenCollectionAdapter, OpenCollectionError};
pub use postman::{PostmanAdapter, PostmanCollection, PostmanError};
