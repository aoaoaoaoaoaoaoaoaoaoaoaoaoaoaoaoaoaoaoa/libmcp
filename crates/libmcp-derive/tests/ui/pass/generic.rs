//! Generic projection implementations carry generated field bounds.

use libmcp::{SelectorProjection as _, StructuredProjection as _};

#[derive(libmcp::ToolProjection)]
struct GenericOutput<T> {
    value: T,
}

#[derive(libmcp::SelectorProjection)]
struct GenericSelector<S, T> {
    #[libmcp(selector)]
    key: S,
    #[libmcp(title)]
    label: T,
}

fn main() {
    let output = GenericOutput { value: 7_u64 };
    assert!(output.concise_projection().is_ok());

    let selector = GenericSelector {
        key: String::from("needle"),
        label: String::from("Needle"),
    };
    assert_eq!(selector.selector_ref().slug, "needle");
}
