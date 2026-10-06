//! The stdio transport: what an agent spawns.

use std::sync::Arc;

use search::Search;
use search_mcp::serve_backend;

use crate::run::{build_backend, build_service};

/// Serve MCP over stdin/stdout until the client disconnects.
pub async fn serve(config_path: Option<String>) -> i32 {
    let service = match build_backend(config_path) {
        Ok(service) => service,
        Err(message) => {
            eprintln!("search: {message}");
            return 1;
        }
    };
    match serve_backend(service).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("search: stdio: {error}");
            1
        }
    }
}

/// Kept so the binary and tests share one construction path.
#[allow(dead_code)]
pub(crate) fn service_for_test(config_path: Option<String>) -> Result<Arc<Search>, String> {
    build_service(config_path, None, None)
}
