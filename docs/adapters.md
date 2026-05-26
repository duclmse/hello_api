# Adapter Reference — Index

`hello_client` provides bidirectional import/export adapters for Postman, Bruno,
curl, OpenCollection, and OpenAPI. All adapters live in the `adapters` module
and are re-exported from the crate root.

```rust
use hello_client::{PostmanAdapter, PostmanCollection, PostmanError};
use hello_client::{BrunoAdapter, BrunoError};
use hello_client::{CurlAdapter, CurlError};
use hello_client::{OpenCollectionAdapter, OpenCollection, OpenCollectionError};
use hello_client::{OpenApiAdapter, OpenApiCollection, OpenApiError};
```

---

## Per-Adapter Documentation

| Adapter                          | Reference                                             |
| -------------------------------- | ----------------------------------------------------- |
| Postman (Collection v2.0 / v2.1) | [docs/client/postman.md](client/postman.md)           |
| Bruno (`.bru` file format)       | [docs/client/bruno.md](client/bruno.md)               |
| curl (command import / export)   | [docs/client/curl.md](client/curl.md)                 |
| sandbox:pm scripting library     | [docs/client/pm-scripting.md](client/pm-scripting.md) |
| OpenCollection + OpenAPI         | [docs/client/adapters.md](client/adapters.md)         |

