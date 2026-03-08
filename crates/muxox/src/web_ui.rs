use dioxus::prelude::*;

#[allow(non_snake_case)]
pub fn App() -> Element {
    rsx! {
        head {
            title { "muxox" }
            meta { charset: "utf-8" }
            meta { name: "viewport", content: "width=device-width, initial-scale=1" }
            style { dangerous_inner_html: include_str!("web_style.css") }
        }
        body {
            header {
                div { class: "logo", "muxox" }
                div { id: "connection-status", "Connecting\u{2026}" }
            }
            div { id: "container" }
            script { dangerous_inner_html: include_str!("web_script.js") }
        }
    }
}

pub fn render_index() -> String {
    dioxus::ssr::render_element(rsx! { App {} })
}