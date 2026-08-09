//! Connects to the MCP servers in a config file and lists what they expose.
//!
//! `cargo run -p taurus-mcp --example probe -- <path-to-mcp.json>`

use std::sync::Arc;

use taurus_mcp::{McpConfig, McpManager};
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: probe <mcp.json>");
    let config: McpConfig = serde_json::from_str(&std::fs::read_to_string(&path)?)?;

    let manager = McpManager::new();
    let tools = manager.connect_all(&config).await;

    for status in manager.statuses().await {
        println!(
            "server {} — {} ({} tools){}",
            status.name,
            if status.connected {
                "connected"
            } else {
                "FAILED"
            },
            status.tool_count,
            status.error.map(|e| format!(": {e}")).unwrap_or_default()
        );
    }

    // Register them exactly as the app does, proving they are indistinguishable
    // from built-ins at the registry boundary.
    let mut registry = ToolRegistry::with_builtins();
    for tool in tools {
        println!("  tool {}", tool.name());
        registry.register(tool);
    }

    // Matches the directory the sample config grants the server.
    let workspace = std::path::Path::new("/tmp/mcp-probe").canonicalize()?;
    let ctx = ToolContext::new(
        workspace.clone(),
        Arc::new(PermissionEngine::new(&workspace, Box::new(AllowAll))),
        CancellationToken::new(),
    );

    // Call one through the registry, permission gate and all.
    if let Some(name) = registry
        .names()
        .find(|n| n.contains("list_directory"))
        .map(str::to_string)
    {
        println!("\ncalling {name}…");
        match registry
            .execute(
                &name,
                serde_json::json!({"path": workspace.to_str().unwrap()}),
                &ctx,
            )
            .await
        {
            Ok(out) => println!("{out}"),
            Err(e) => println!("error: {e}"),
        }
    }

    manager.shutdown().await;
    Ok(())
}
