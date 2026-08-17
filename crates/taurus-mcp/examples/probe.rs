//! Connects to the MCP servers in a config file and lists what they expose.
//!
//! `cargo run -p taurus-mcp --example probe -- <path-to-mcp.json>`

use std::sync::Arc;

use taurus_mcp::McpManager;
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The same repair the desktop app makes at startup, so probing reproduces
    // what the app does rather than whatever the terminal happened to have.
    // This is the tool you reach for when a server will not start, and a PATH
    // that differs from the app's is the thing that makes that unreproducible.
    let env = taurus_tools::login_path::adopt();
    match &env.skipped {
        Some(reason) => println!("PATH: not read from the login shell ({reason})"),
        None if env.added.is_empty() => println!("PATH: already complete"),
        None => println!("PATH: added {} from the login shell", env.added.join(", ")),
    }

    let path = std::env::args().nth(1).expect("usage: probe <mcp.json>");
    let config = taurus_mcp::parse(&std::fs::read_to_string(&path)?)?;
    // Reported here as well as connected to, because an entry that will not
    // parse no longer takes its neighbours down with it and so would otherwise
    // pass in silence.
    for (name, reason) in &config.invalid {
        println!("server {name} — UNREADABLE: {reason}");
    }

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
        Arc::new(PermissionEngine::new(
            &workspace,
            workspace.join(".taurus"),
            Box::new(AllowAll),
        )),
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
