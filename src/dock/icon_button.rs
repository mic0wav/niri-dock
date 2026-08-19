use gtk::prelude::*;
use relm4::prelude::*;

#[derive(Debug, Clone)]
pub enum Action {
    Focus(u64),
    Launch(String),
}

pub struct IconButtonModel {
    icon_name: String,
    action: Action,
    focused: bool, 
}

#[derive(Debug)]
pub enum Output {
    Focus(u64),
    Launch(String),
}

#[derive(Debug, Clone)]
pub enum Input {
    Clicked,
}

#[relm4::factory(pub async)]
impl AsyncFactoryComponent for IconButtonModel {
    type Init = (String, Action, bool);
    type Input = Input;
    type Output = Output;
    type CommandOutput = ();
    type ParentWidget = gtk::Box;

    view! {
        #[root]
        gtk::Button {
            #[watch]
            set_class_active: ("active", self.focused),
            add_css_class: "app",
            set_valign: gtk::Align::Center,
            connect_clicked => Input::Clicked,
            gtk::Image {
                set_icon_name: Some(&self.icon_name),
                set_icon_size: gtk::IconSize::Large,
            }
        }
    }

    async fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: AsyncFactorySender<Self>) -> Self {
        Self { icon_name: init.0, action: init.1, focused: init.2 }
    }

    async fn update(&mut self, msg: Self::Input, sender: AsyncFactorySender<Self>) {
        match msg {
            Input::Clicked => {
                let out = match &self.action {
                    Action::Focus(id) => Output::Focus(*id),
                    Action::Launch(cmd) => Output::Launch(cmd.clone()),
                };
                sender.output(out).unwrap();
            }
        }
    }
}
