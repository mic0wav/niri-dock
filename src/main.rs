mod backoff;
mod config;
mod dock;
mod niri_ipc;

use std::sync::OnceLock;
use tokio::runtime::Runtime;

pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("failed to build tokio runtime"))
}

use env_logger::Env;

use gtk::prelude::*;
use relm4::{prelude::*, set_global_css};

struct AppModel {
    #[allow(dead_code)]
    dock: Controller<dock::DockModel>,
}

#[derive(Debug)]
pub enum Input {
    FocusWindow(u64),
    LaunchApp(String),
}

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = ();
    type Input = Input;
    type Output = ();

    view! {
        gtk::Window {
            set_visible: false
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let app = relm4::main_application();
        let dock_builder = dock::DockModel::builder();
        app.add_window(&dock_builder.root);

        let dock = dock_builder
            .launch(())
            .forward(sender.input_sender(), |msg| match msg {
                dock::Output::Focus(x) => Input::FocusWindow(x),
                dock::Output::Launch(x) => Input::LaunchApp(x),
            });

        let model = AppModel { dock };
        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            Input::FocusWindow(id) => {
                crate::runtime().spawn(async move {
                    if let Err(e) = niri_ipc::focus_window(id).await {
                        log::error!("Failed to focus window: {e}");
                    }
                });
            }
            Input::LaunchApp(cmd) => {
                crate::runtime().spawn(async move {
                    if let Err(e) = niri_ipc::spawn(cmd).await {
                        log::error!("Failed to launch app: {e}");
                    }
                });
            }
        }
    }
}

fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();
    let app = RelmApp::new(niri_ipc::APP_ID);

    set_global_css(&load_css());
    app.run::<AppModel>(());
}

fn load_css() -> String {
    const DEFAULT_CSS: &str = include_str!("../resources/dock.css");
    config::read_or_seed_default("dock.css", DEFAULT_CSS)
}
