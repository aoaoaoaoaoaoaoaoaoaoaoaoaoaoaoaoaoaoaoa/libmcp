use libmcp::ToolProjection;

#[derive(ToolProjection)]
struct Output {
    #[libmcp(full_only, concise_only)]
    value: String,
}

fn main() {}
