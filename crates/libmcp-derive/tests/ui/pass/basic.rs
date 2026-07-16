//! A downstream projection needs no direct serde or serde_json dependency.

use libmcp::{StructuredProjection as _, ToolProjection};

#[derive(ToolProjection)]
#[libmcp(kind = "list")]
struct Output {
    slug: String,
    r#type: String,
    #[libmcp(skip_none)]
    summary: Option<String>,
    #[libmcp(full_only)]
    body: String,
}

fn main() {
    let output = Output {
        slug: "needle".to_owned(),
        r#type: "result".to_owned(),
        summary: None,
        body: "full only".to_owned(),
    };
    let concise = output.concise_projection().expect("projection must serialize");
    assert_eq!(concise["slug"], "needle");
    assert_eq!(concise["type"], "result");
    assert!(concise.get("body").is_none());
}
