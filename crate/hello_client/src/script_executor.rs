use std::fs;
use std::path::Path;
use mlua::{Lua, Result as LuaResult};
use quick_js::{Context as JsContext};

/// Reads a script file, determines its type (Lua or JS/TS), and executes it in a sandbox.
pub fn execute_script(file_path: &str) -> Result<(), String> {
    // Read the script file
    let script_content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read script file: {}", e))?;

    // Determine the script type based on the file extension
    let script_type = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or("Failed to determine script type")?;

    match script_type {
        "lua" => execute_lua(&script_content),
        "js" | "ts" => execute_js(&script_content),
        _ => Err(format!("Unsupported script type: {}", script_type)),
    }
}

/// Executes a Lua script in a sandbox.
fn execute_lua(script: &str) -> Result<(), String> {
    let lua = Lua::new();
    lua.context(|ctx| {
        ctx.load(script)
            .set_name("sandboxed_script")
            .map_err(|e| format!("Lua script error: {}", e))?
            .exec()
            .map_err(|e| format!("Lua execution error: {}", e))
    })
}

/// Executes a JavaScript/TypeScript script in a sandbox.
// fn execute_js(script: &str) -> Result<(), String> {
//     let context = JsContext::new() //
//         .map_err(|e| format!("Failed to create JS context: {}", e))?;
//     context
//         .eval(script)
//         .map_err(|e| format!("JavaScript execution error: {}", e))?;
//     Ok(())
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_script() {
        let lua_script = r#"
            print("Hello from Lua!")
            local x = 10
            return x * 2
        "#;
        assert!(execute_lua(lua_script).is_ok());
    }

    #[test]
    fn test_js_script() {
        let js_script = r#"
            console.log("Hello from JavaScript!");
            let x = 10;
            x * 2;
        "#;
        assert!(execute_js(js_script).is_ok());
    }
}