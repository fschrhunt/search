//! The stdio transport: what an agent spawns.

use crate::mcp::serve_client;

use crate::cli::run::build_client;

/// Serve MCP over stdin/stdout until the client disconnects.
pub async fn serve(config_path: Option<String>) -> i32 {
    let service = match build_client(config_path) {
        Ok(service) => service,
        Err(message) => {
            eprintln!("search: {message}");
            return 1;
        }
    };
    match serve_client(service).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("search: stdio: {error}");
            1
        }
    }
}
