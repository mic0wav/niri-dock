use gtk4_layer_shell::{Edge, Layer, LayerShell};

pub fn anchor_bottom(window: &gtk4::Window) {
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    for (anchor, state) in [
        (Edge::Left, false),
        (Edge::Right, false),
        (Edge::Top, false),
        (Edge::Bottom, true),
    ] {
        window.set_anchor(anchor, state);
    }
}
