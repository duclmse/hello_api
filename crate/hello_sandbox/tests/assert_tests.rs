//! Tests for the sandbox:assert module (F9).

use hello_sandbox::{AssertPack, PoolConfig, Sandbox, SandboxConfig};
use serde_json::json;
use tokio::task::LocalSet;

fn single_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..Default::default()
    }
}

fn make_assert_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(single_slot())
        .sdk(AssertPack)
        .build()
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn assert_equal_pass() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.equal(1 + 1, 2, "math works");
                    assert.equal("hello", "hello");
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 2);
            assert_eq!(result.metrics.assertions_failed, 0);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_equal_fail() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.equal(1, 2, "should fail");
                    assert.equal("a", "b");
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 0);
            assert_eq!(result.metrics.assertions_failed, 2);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_ok_truthy_and_falsy() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.ok(1);
                    assert.ok("non-empty");
                    assert.ok(0);  // falsy — fails
                    assert.ok("");  // falsy — fails
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 2);
            assert_eq!(result.metrics.assertions_failed, 2);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_contains_string_and_array() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.contains("hello world", "world");
                    assert.contains([1, 2, 3], 2);
                    assert.contains("abc", "xyz");  // fails
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 2);
            assert_eq!(result.metrics.assertions_failed, 1);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_not_equal() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.notEqual(1, 2);       // passes
                    assert.notEqual("x", "x");   // fails
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 1);
            assert_eq!(result.metrics.assertions_failed, 1);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_greater_less_than() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            let result = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.greaterThan(5, 3);   // passes
                    assert.lessThan(2, 10);     // passes
                    assert.greaterThan(3, 5);   // fails
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.metrics.assertions_passed, 2);
            assert_eq!(result.metrics.assertions_failed, 1);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_counts_do_not_leak_across_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_assert_sandbox();
            // Run 1: one pass.
            let r1 = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.equal(1, 1);
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(r1.metrics.assertions_passed, 1);
            assert_eq!(r1.metrics.assertions_failed, 0);

            // Run 2: one fail. Counts must be fresh (not accumulated).
            let r2 = sb
                .run(
                    r#"
                    import { assert } from "sandbox:assert";
                    assert.equal(1, 99);
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(r2.metrics.assertions_passed, 0);
            assert_eq!(r2.metrics.assertions_failed, 1);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn assert_metrics_zero_when_pack_not_registered() {
    LocalSet::new()
        .run_until(async {
            // Sandbox WITHOUT AssertPack.
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(single_slot())
                .build()
                .unwrap();
            let result = sb.run("return 42;").await.unwrap();
            assert_eq!(result.metrics.assertions_passed, 0);
            assert_eq!(result.metrics.assertions_failed, 0);
            assert_eq!(result.value, json!(42));
        })
        .await;
}
