//! KV pack — per-slot key-value store with a pluggable backend.
//!
//! The default backend is [`InMemoryKvBackend`] (an in-process `HashMap`).
//! Pass a custom [`KvBackend`] impl to [`KvPack::with_backend`] to plug in
//! Redis, SQLite, or any other store without changing the JS `kv.*` API.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use deno_core::{op2, OpDecl, OpState};
use deno_error::JsErrorBox;
use serde_json::Value;

use crate::runtime::RunState;
use crate::sdk::SdkExtension;

// ─── KvBackend trait ─────────────────────────────────────────────────────────

/// Pluggable storage backend for [`KvPack`].
///
/// Implement this trait to use a persistent or distributed store
/// (Redis, SQLite, DynamoDB, …) while keeping the same `kv.*` JS API.
///
/// All methods receive `&self` — implementations must use interior mutability
/// (e.g. `Arc<Mutex<…>>` or a connection pool) for writes.
///
/// # Example — custom backend
///
/// ```rust,ignore
/// use hello_sandbox::KvBackend;
/// use serde_json::Value;
///
/// struct MyBackend;
///
/// impl KvBackend for MyBackend {
///     async fn get(&self, key: &str) -> Option<Value> { None }
///     async fn set(&self, key: &str, value: Value) {}
///     async fn delete(&self, key: &str) {}
///     async fn list(&self, prefix: &str) -> Vec<String> { vec![] }
/// }
/// ```
pub trait KvBackend: Send + Sync + 'static {
    /// Retrieve the value stored under `key`, or `None` if absent.
    fn get(
        &self,
        key: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send>>;

    /// Store `value` under `key`, replacing any existing entry.
    fn set(
        &self,
        key: &str,
        value: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

    /// Remove the entry for `key` (no-op if absent).
    fn delete(&self, key: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

    /// Return all keys whose prefix matches `prefix`.
    fn list(
        &self,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send>>;
}

// ─── InMemoryKvBackend ────────────────────────────────────────────────────────

/// Default [`KvBackend`] — an in-process `HashMap` behind a `Mutex`.
///
/// State persists across runs on the same pool slot and is cleared when the
/// slot is recycled.
pub struct InMemoryKvBackend {
    data: Mutex<HashMap<String, Value>>,
}

impl InMemoryKvBackend {
    /// Create a new, empty in-memory backend.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryKvBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KvBackend for InMemoryKvBackend {
    fn get(
        &self,
        key: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send>> {
        let val = self.data.lock().unwrap().get(key).cloned();
        Box::pin(async move { val })
    }

    fn set(
        &self,
        key: &str,
        value: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.data.lock().unwrap().insert(key.to_string(), value);
        Box::pin(async move {})
    }

    fn delete(&self, key: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.data.lock().unwrap().remove(key);
        Box::pin(async move {})
    }

    fn list(
        &self,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send>> {
        let keys: Vec<String> =
            self.data.lock().unwrap().keys().filter(|k| k.starts_with(prefix)).cloned().collect();
        Box::pin(async move { keys })
    }
}

// ─── Op state ────────────────────────────────────────────────────────────────

/// Per-slot KV state: a shared handle to the configured backend.
///
/// The `Arc` allows shared-storage mode (same backend instance across multiple
/// pool slots) when created externally; the default `KvPack::new()` creates
/// one `Arc<InMemoryKvBackend>` per slot (isolated storage).
pub struct KvStore {
    pub backend: Arc<dyn KvBackend>,
}

// ─── Capability + rate-limit helper ──────────────────────────────────────────

/// Check per-run KV capabilities and increment the operation counter.
///
/// Must be called **synchronously** (before any `await`) in each kv op.
///
/// Mutates `key` in-place to prepend the namespace prefix (if configured in
/// `RunCapabilities::kv_key_prefix`).
///
/// Returns `Ok(Option<String>)` where the inner `Option` is the namespace
/// prefix that was prepended (if any), so `op_kv_list` can strip it from
/// the result keys.
fn kv_check_capabilities(
    state: &Rc<RefCell<OpState>>,
    key: &mut String,
) -> Result<Option<String>, JsErrorBox> {
    let mut s = state.borrow_mut();
    let run_state = s.borrow_mut::<RunState>();

    // Pack-level enable/disable (capability overrides always win).
    if run_state.capabilities.kv_enabled == Some(false) {
        return Err(JsErrorBox::generic("capability denied: kv"));
    }

    // Per-run rate limit: capability override takes precedence over pool limit.
    let limit = run_state.capabilities.kv_ops_limit.or(run_state.rate_limits.kv_ops_per_run);
    run_state.kv_ops += 1;
    if let Some(lim) = limit {
        if run_state.kv_ops > lim {
            run_state.rate_limit_exceeded = Some(("kv".to_string(), lim));
            return Err(JsErrorBox::generic(format!("rate limit exceeded: kv (limit: {lim})")));
        }
    }

    // Namespace prefix injection: rewrite the caller's key in-place.
    let ns = if let Some(prefix) = &run_state.capabilities.kv_key_prefix {
        let namespaced = format!("{prefix}{key}");
        *key = namespaced;
        Some(prefix.clone())
    } else {
        None
    };

    Ok(ns)
}

// ─── Ops ─────────────────────────────────────────────────────────────────────

#[op2(async(deferred), fast)]
#[serde]
async fn op_kv_get(
    state: Rc<RefCell<OpState>>,
    #[string] mut key: String,
) -> Result<serde_json::Value, JsErrorBox> {
    kv_check_capabilities(&state, &mut key)?;
    let backend = state.borrow().borrow::<KvStore>().backend.clone();
    Ok(backend.get(&key).await.unwrap_or(serde_json::Value::Null))
}

#[op2(async(deferred), fast)]
async fn op_kv_set(
    state: Rc<RefCell<OpState>>,
    #[string] mut key: String,
    #[string] value_json: String,
) -> Result<(), JsErrorBox> {
    kv_check_capabilities(&state, &mut key)?;
    let v: serde_json::Value = serde_json::from_str(&value_json).unwrap_or(serde_json::Value::Null);
    let backend = state.borrow().borrow::<KvStore>().backend.clone();
    backend.set(&key, v).await;
    Ok(())
}

#[op2(async(deferred), fast)]
async fn op_kv_delete(
    state: Rc<RefCell<OpState>>,
    #[string] mut key: String,
) -> Result<(), JsErrorBox> {
    kv_check_capabilities(&state, &mut key)?;
    let backend = state.borrow().borrow::<KvStore>().backend.clone();
    backend.delete(&key).await;
    Ok(())
}

#[op2(async(deferred), fast)]
#[serde]
async fn op_kv_list(
    state: Rc<RefCell<OpState>>,
    #[string] mut prefix: String,
) -> Result<Vec<String>, JsErrorBox> {
    let ns = kv_check_capabilities(&state, &mut prefix)?;
    let backend = state.borrow().borrow::<KvStore>().backend.clone();
    let mut keys = backend.list(&prefix).await;
    // Strip the namespace prefix from returned keys so scripts only see the
    // user-visible (un-namespaced) portion of each key.
    if let Some(ns_str) = ns {
        keys =
            keys.into_iter().map(|k| k.strip_prefix(&ns_str).unwrap_or(&k).to_string()).collect();
    }
    Ok(keys)
}

// ─── Pack ────────────────────────────────────────────────────────────────────

/// KV SDK pack — per-slot key/value store (`get`, `set`, `delete`, `list`).
///
/// Uses [`InMemoryKvBackend`] by default.  Pass a custom backend via
/// [`KvPack::with_backend`] to plug in a persistent or distributed store.
///
/// # Examples
///
/// ```rust,ignore
/// // Default in-memory backend:
/// SandboxBuilder::new().sdk(KvPack::new())
///
/// // Custom backend:
/// SandboxBuilder::new().sdk(KvPack::with_backend(MyRedisBackend::new()))
/// ```
pub struct KvPack {
    backend_factory: Arc<dyn Fn() -> Arc<dyn KvBackend> + Send + Sync>,
}

impl KvPack {
    /// Create a `KvPack` with the default [`InMemoryKvBackend`].
    ///
    /// Each pool slot gets its own independent in-memory store.
    pub fn new() -> Self {
        Self {
            backend_factory: Arc::new(|| Arc::new(InMemoryKvBackend::new())),
        }
    }

    /// Create a `KvPack` where every slot shares the provided backend instance.
    ///
    /// Use this for shared-storage mode (e.g. all slots reading/writing to the
    /// same Redis or SQLite backend).
    pub fn with_backend(backend: impl KvBackend) -> Self {
        let arc: Arc<dyn KvBackend> = Arc::new(backend);
        Self {
            backend_factory: Arc::new(move || arc.clone()),
        }
    }
}

impl Default for KvPack {
    fn default() -> Self {
        Self::new()
    }
}

impl SdkExtension for KvPack {
    fn name(&self) -> &'static str {
        "kv"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_kv_get(), op_kv_set(), op_kv_delete(), op_kv_list()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:kv", include_str!("../../sdk-ts/src/kv.js"))]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/kv.d.ts")
    }

    fn inject_op_state(&self, op_state: &mut deno_core::OpState) {
        op_state.put(KvStore {
            backend: (self.backend_factory)(),
        });
    }
}
