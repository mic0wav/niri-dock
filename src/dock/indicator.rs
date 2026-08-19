use gtk::prelude::*;
use relm4::prelude::*;

use super::layer_shell;

pub struct IndicatorModel {
    visible: bool,
}

#[derive(Debug)]
pub enum Input {
    Enter,
    Leave,
}

#[derive(Debug)]
pub enum Output {
    Enter,
}

#[relm4::component(pub)]
impl SimpleComponent for IndicatorModel {
    type Init = ();
    type Input = Input;
    type Output = Output;

    view! {
        #[name = "window"]
        gtk::Window {
            #[watch]
            set_visible: model.visible,

            gtk::Box {
                add_css_class: "indicator",
                set_margin_all: 2,
                add_controller = gtk::EventControllerMotion {
                    connect_enter[sender] => move |_, _, _| {
                        sender.input(Input::Enter)
                    }
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = IndicatorModel { visible: true };
        let widgets = view_output!();

        layer_shell::anchor_bottom(&widgets.window);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            Input::Enter => {
                self.visible = false;
                sender.output(Output::Enter).unwrap();
            }
            Input::Leave => {
                self.visible = true;
            }
        }
    }
}
