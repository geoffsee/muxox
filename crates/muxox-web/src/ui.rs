use dioxus::prelude::*;

#[component]
pub fn App(name: String) -> Element {
    rsx! {
        head {
            title { "{name}" }
            meta { charset: "utf-8" }
            meta { name: "viewport", content: "width=device-width, initial-scale=1" }
            style { dangerous_inner_html: include_str!("web_style.css") }
        }
        body {
            header {
                div { class: "logo", "{name}" }
                div { id: "connection-status", "Connecting\u{2026}" }
            }
            div { id: "update-notice", role: "status", hidden: true }
            div { id: "container" }
            script { dangerous_inner_html: include_str!("web_script.js") }
        }
    }
}

pub fn render_index(name: &str) -> String {
    let name = name.to_string();
    dioxus::ssr::render_element(rsx! { App { name } })
}
