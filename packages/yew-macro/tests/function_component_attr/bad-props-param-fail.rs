use yew::prelude::*;

#[derive(Clone, Properties, PartialEq)]
struct Props {
    a: usize,
}

#[component(Comp)]
fn comp(props: Props) -> Html {
    html! {
        <p>
            { props.a }
        </p>
    }
}

fn main() {}
