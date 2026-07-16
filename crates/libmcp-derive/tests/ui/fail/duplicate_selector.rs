use libmcp::SelectorProjection;

#[derive(SelectorProjection)]
struct Selector {
    #[libmcp(selector)]
    first: String,
    #[libmcp(selector)]
    second: String,
}

fn main() {}
