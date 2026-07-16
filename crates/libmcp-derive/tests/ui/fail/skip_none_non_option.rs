use libmcp::ToolProjection;

#[derive(ToolProjection)]
struct Output {
    #[libmcp(skip_none)]
    value: String,
}

fn main() {}
