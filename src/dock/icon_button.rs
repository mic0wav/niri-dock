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
    title: String,
}

#[derive(Debug)]
pub enum Output {
    Focus(u64),
    Launch(String),
}

#[derive(Debug, Clone)]
pub enum Input {
    Clicked,
    SetFocused(bool),
    Update { icon_name: String, title: String },
}

#[relm4::factory(pub async)]
impl AsyncFactoryComponent for IconButtonModel {
    type Init = (String, Action, bool, String);
    type Input = Input;
    type Output = Output;
    type CommandOutput = ();
    type ParentWidget = gtk::Box;

    view! {
        #[root]
        gtk::Button {
            #[watch]
            set_class_active: ("active", self.focused),
            #[watch]
            set_tooltip_text: Some(&self.title),
            add_css_class: "app",
            set_valign: gtk::Align::Center,
            connect_clicked => Input::Clicked,
            gtk::Image {
                #[watch]
                set_icon_name: Some(&self.icon_name),
                set_icon_size: gtk::IconSize::Large,
            }
        }
    }

    async fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: AsyncFactorySender<Self>) -> Self {
        Self { icon_name: init.0, action: init.1, focused: init.2, title: init.3 }
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
            Input::SetFocused(focused) => {
                self.focused = focused;
            }
            Input::Update { icon_name, title } => {
                self.icon_name = icon_name;
                self.title = title;
            }
        }
    }
}

impl IconButtonModel {
    pub fn window_id(&self) -> Option<u64> {
        match self.action {
            Action::Focus(id) => Some(id),
            Action::Launch(_) => None,
        }
    }
}
