mod icon_button;
mod indicator;
mod layer_shell;

use std::{fs::File, io::Read};

use gtk::prelude::*;
use relm4::prelude::*;

use icon_button::Action;
use crate::niri_ipc::NiriEvent;

#[derive(serde::Deserialize, Clone)]
struct Launchables {
    icons: Vec<String>,
    commands: Vec<String>,
}

#[tracker::track]
pub struct DockModel {
    enabled: bool,
    visible: bool,
    #[tracker::do_not_track]
    apps: AsyncFactoryVecDeque<icon_button::IconButtonModel>,
    #[tracker::do_not_track]
    launchables: AsyncFactoryVecDeque<icon_button::IconButtonModel>,
    #[tracker::do_not_track]
    indicator: Controller<indicator::IndicatorModel>,
    #[tracker::do_not_track]
    windows: Vec<crate::niri_ipc::WindowInfo>,
    apps_count: usize,
}

#[derive(Debug)]
pub enum Input {
    Enter,
    Leave,
    Focus(u64),
    Launch(String),
    NiriEvent(NiriEvent),
}

#[derive(Debug)]
pub enum Output {
    Focus(u64),
    Launch(String),
}

#[relm4::component(pub)]
impl SimpleComponent for DockModel {
    type Init = ();
    type Input = Input;
    type Output = Output;

    view! {
        #[name = "window"]
        gtk::Window {
            #[track = "model.changed_visible() || model.changed_enabled()"]
            set_visible: model.visible && model.enabled,

            gtk::Box {
                set_margin_all: 8,
                set_spacing: 8,
                add_controller = gtk::EventControllerMotion {
                    connect_leave => Input::Leave,
                },

                #[local_ref]
                launchables_box -> gtk::Box {
                    add_css_class: "dock",
                    set_spacing: 8,
                },

                #[local_ref]
                apps_box -> gtk::Box {
                    #[track = "model.changed_apps_count()"]
                    set_visible: model.apps_count > 0,
                    set_spacing: 8,
                    add_css_class: "dock",
                },
            }
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let apps = AsyncFactoryVecDeque::builder()
            .launch(gtk::Box::default())
            .forward(sender.input_sender(), |msg| match msg {
                icon_button::Output::Focus(x) => Input::Focus(x),
                icon_button::Output::Launch(x) => Input::Launch(x),
            });

        let mut launchables = AsyncFactoryVecDeque::builder()
            .launch(gtk::Box::default())
            .forward(sender.input_sender(), |msg| match msg {
                icon_button::Output::Focus(x) => Input::Focus(x),
                icon_button::Output::Launch(x) => Input::Launch(x),
            });
        if let Some(x) = load_launchables() {
            for (i, y) in x.icons.iter().enumerate() {
                launchables.guard().push_back((
                    y.to_owned(),
                    Action::Launch(x.commands[i].clone()),
                    false,
                    y.to_owned(),
                ));
            }
        }

        let indicator_builder = indicator::IndicatorModel::builder();
        relm4::main_application().add_window(&indicator_builder.root);
        let indicator =
            indicator_builder
                .launch(())
                .forward(sender.input_sender(), |msg| match msg {
                    indicator::Output::Enter => Input::Enter,
                });

        let model = DockModel {
            enabled: true,
            visible: false,
            apps,
            launchables,
            indicator,
            windows: vec![],
            apps_count: 0,
            tracker: 0,
        };

        let apps_box = model.apps.widget();
        let launchables_box = model.launchables.widget();
        let widgets = view_output!();

        layer_shell::anchor_bottom(&widgets.window);

        // One task owns the actual socket and turns raw niri events into a
        // Rust channel; a second forwards those into this component's own
        // input queue. If the socket ever drops (niri restarted, etc.) it
        // waits a couple seconds and reconnects rather than giving up.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::runtime().spawn(async move {
            loop {
                if let Err(e) = crate::niri_ipc::event_stream(tx.clone()).await {
                    log::error!("Niri event stream ended: {e}, retrying in 2s");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });

        let sender_clone = sender.clone();
        crate::runtime().spawn(async move {
            while let Some(event) = rx.recv().await {
                sender_clone.input(Input::NiriEvent(event));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        self.reset();

        match msg {
            Input::Launch(x) => {
                sender.output(Output::Launch(x)).unwrap();
            }
            Input::Focus(x) => {
                sender.output(Output::Focus(x)).unwrap();
            }
            Input::Enter => {
                self.set_visible(true);
            }
            Input::Leave => {
                self.set_visible(false);
                self.indicator.emit(indicator::Input::Leave);
            }
            Input::NiriEvent(event) => {
                match event {
                    NiriEvent::WindowsChanged(windows) => {
                        self.windows = windows;
                    }
                    NiriEvent::WindowOpenedOrChanged(w) => {
                        if let Some(existing) = self.windows.iter_mut().find(|x| x.id == w.id) {
                            *existing = w;
                        } else {
                            self.windows.push(w);
                        }
                    }
                    NiriEvent::WindowClosed(id) => {
                        self.windows.retain(|w| w.id != id);
                    }
                    NiriEvent::WindowFocusChanged(focused_id) => {
                        for w in self.windows.iter_mut() {
                            w.focused = Some(w.id) == focused_id;
                        }
                    }
                }

                self.set_apps_count(self.windows.len());
                let mut guard = self.apps.guard();
                guard.clear();
                for w in &self.windows {
                    guard.push_back((
                        icon_name_for_app_id(&w.app_id),
                        Action::Focus(w.id),
                        w.focused,
                        w.title.clone(),
                    ));
                }
            }
        }
    }
}

fn load_launchables() -> Option<Launchables> {
    let Some(dir) = crate::config::dir() else {
        log::error!("Failed to find config directory.");
        return None;
    };
    let path = dir.join("config.toml");

    let mut file = match File::open(&path) {
        Ok(x) => x,
        Err(e) => {
            log::error!("Failed to read config: {e}");
            return None;
        }
    };

    let mut buf = String::new();
    match file.read_to_string(&mut buf) {
        Ok(_) => (),
        Err(e) => {
            log::error!("Failed to read config: {e}");
            return None;
        }
    }

    match toml::from_str(&buf) {
        Ok(x) => Some(x),
        Err(e) => {
            log::error!("Failed to parse config: {e}");
            None
        }
    }
}

fn icon_name_for_app_id(app_id: &str) -> String {
    let candidates = [
        format!("{app_id}.desktop"),
        format!("{}.desktop", app_id.to_lowercase()),
    ];
    for desktop_id in candidates {
        if let Some(info) = gtk::gio::DesktopAppInfo::new(&desktop_id) {
            if let Some(icon) = info.icon() {
                if let Some(name) = icon.to_string() {
                    return name.to_string();
                }
            }
        }
    }
    app_id.to_string()
}
