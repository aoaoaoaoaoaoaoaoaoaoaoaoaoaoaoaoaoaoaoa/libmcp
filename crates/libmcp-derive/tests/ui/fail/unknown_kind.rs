use libmcp::ToolProjection;

#[derive(ToolProjection)]
#[libmcp(kind = "transcendental")]
struct Output {
    value: String,
}

fn main() {}
