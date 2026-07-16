use libmcp::ToolProjection;

#[derive(ToolProjection)]
struct Output {
    #[libmcp(full_ony)]
    value: String,
}

fn main() {}
