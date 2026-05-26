# Postman Compatibility Notes

hello_client includes compatibility support for Postman-style scripting via the
`sandbox:pm` module (provided by `PmPack` in hello_sandbox).

---

## sandbox:pm — The pm.\* API

`PmPack` installs `sandbox:pm` which implements the `pm.*` surface used in
Postman test scripts:

```js
import { pm } from "sandbox:pm";

pm.test("Status is 200", () => {
  pm.expect(pm.response.code).to.equal(200);
});

pm.environment.set("token", pm.response.json().access_token);
pm.environment.get("base_url");
```

This allows existing Postman post-request scripts to run in hello_client with
minimal or no changes. See [PmPack](../sandbox/sdk/pm.md) for the full API
surface.

---

## Shared Helper Functions in hello_client

hello_client does not require workarounds to share helper functions across
requests. Use these native approaches:

**Local JS module imports (recommended):**

```http
GET https://{{base_url}}/users/1

> ./helpers.js
```

`helpers.js` is a local file imported as a script reference. It can export
shared utilities used by multiple post-scripts.

**Collection-level pre-scripts:** Register a pre-script on the `HttpTestRunner`
that runs before every test in the collection and writes shared functions or
setup data to the KV store.

**Custom `sandbox:*` modules:** Register shared logic once via
`HttpTestRunner::builder()`:

```rust
.module("sandbox:helpers", include_str!("helpers.js"))
```

Then import it in any script: `import { myHelper } from "sandbox:helpers";`

---

## Postman Native Approach (Reference)

The following describes how Postman handles helper function sharing natively.
This is provided as context for migrating existing Postman collections.

**Collection-Level Scripts:** Postman allows defining functions in the
Pre-request Script or Tests tab of a collection or folder. These scripts run
before or after every request within that scope.

**Global Helper Functions (Postman workaround):** A common community pattern in
Postman is to store a function string in a collection variable and use `eval()`
to execute it:

```js
// Define in a collection Pre-request Script:
pm.collectionVariables.set(
  "myHelper",
  "function(name) { return 'Hello ' + name; }"
);

// Use in any request:
const myHelper = eval(pm.collectionVariables.get("myHelper"));
console.log(myHelper("User"));
```

This workaround is not needed in hello_client — use local module imports or
`sandbox:*` modules instead.

**Postman Sandbox API (`pm.*`):**

- `pm.environment.set("key", "value")` — save data between requests
- `pm.sendRequest()` — make side-requests for setup or cleanup
- `pm.expect()` — write test assertions

hello_client implements this surface via `PmPack` (`sandbox:pm`). See the
[Adapters](../client/adapters.md) documentation for importing Postman
collections directly into hello_client.
