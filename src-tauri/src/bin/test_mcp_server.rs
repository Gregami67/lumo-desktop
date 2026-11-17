use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars::JsonSchema;
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct EchoArgs {
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct AddArgs {
    a: f64,
    b: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SlowArgs {
    seconds: f64,
}

#[derive(Clone)]
struct TestServer {
    tool_router: ToolRouter<Self>,
}

impl TestServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl TestServer {
    #[tool(description = "Return the text you send.")]
    async fn echo(&self, Parameters(args): Parameters<EchoArgs>) -> String {
        args.text
    }

    #[tool(description = "Add two numbers.")]
    async fn add(&self, Parameters(args): Parameters<AddArgs>) -> String {
        (args.a + args.b).to_string()
    }

    #[tool(description = "Sleep for the given seconds, then reply.")]
    async fn slow(&self, Parameters(args): Parameters<SlowArgs>) -> String {
        tokio::time::sleep(Duration::from_secs_f64(args.seconds)).await;
        "awake".to_string()
    }

    #[tool(description = "Always fails.")]
    async fn boom(&self) -> Result<String, ErrorData> {
        Err(ErrorData::internal_error("intentional failure", None))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TestServer {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[test-mcp-server] starting");
    let service = TestServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
